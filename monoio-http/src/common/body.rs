use std::{
    cell::UnsafeCell,
    io::{Cursor, Read},
    task::{ready, Poll},
};

use bytes::{Bytes, BytesMut};
use flate2::{
    read::{GzDecoder, GzEncoder, ZlibDecoder, ZlibEncoder},
    Compression,
};
use futures_core::Future;
use monoio::buf::IoBuf;
use monoio_compat::box_future::MaybeArmedBoxFuture;
use smallvec::SmallVec;

use super::error::{EncodeDecodeError, HttpError};
use crate::{
    common::{request::Request, response::Response},
    h1::payload::Payload,
    h2::RecvStream,
};

const SUPPORTED_ENCODINGS: [&str; 3] = ["gzip", "br", "deflate"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamHint {
    None,
    Fixed,
    Stream,
}

pub trait Body {
    type Data: IoBuf;
    type Error;

    fn next_data(&mut self) -> impl Future<Output = Option<Result<Self::Data, Self::Error>>>;
    fn stream_hint(&self) -> StreamHint;
}

impl<T: Body> Body for &mut T {
    type Data = T::Data;
    type Error = T::Error;

    #[inline]
    fn next_data(&mut self) -> impl Future<Output = Option<Result<Self::Data, Self::Error>>> {
        (**self).next_data()
    }

    #[inline]
    fn stream_hint(&self) -> StreamHint {
        (**self).stream_hint()
    }
}

pub type Chunks = SmallVec<[Bytes; 16]>;

pub trait BodyExt: Body {
    /// Consumes body and return continous memory
    fn bytes(self) -> impl Future<Output = Result<Bytes, Self::Error>>;
    /// Return bytes array
    fn chunks(self) -> impl Future<Output = Result<Chunks, Self::Error>>;
}

pub trait BodyEncodeExt: BodyExt {
    type EncodeDecodeError;

    /// Consumes body and returns decodec content
    fn decode_content(
        self,
        content_encoding: String,
    ) -> impl Future<Output = Result<Bytes, Self::EncodeDecodeError>>;
    /// Consumes body and returns encoded content
    fn encode_content(
        self,
        accept_encoding: String,
    ) -> impl Future<Output = Result<Bytes, Self::EncodeDecodeError>>;
}

impl<T> BodyExt for T
where
    T: Body<Data = Bytes>,
{
    async fn bytes(mut self) -> Result<Bytes, Self::Error> {
        match self.stream_hint() {
            StreamHint::None => Ok(Bytes::new()),
            StreamHint::Fixed => self.next_data().await.unwrap_or(Ok(Bytes::new())),
            StreamHint::Stream => {
                let mut data = BytesMut::new();
                while let Some(chunk) = self.next_data().await {
                    data.extend_from_slice(&chunk?);
                }
                Ok(data.freeze())
            }
        }
    }

    async fn chunks(mut self) -> Result<Chunks, Self::Error> {
        match self.stream_hint() {
            StreamHint::None => Ok(Chunks::new()),
            StreamHint::Fixed => {
                let mut chunks = Chunks::new();
                if let Some(b) = self.next_data().await {
                    chunks.push(b?);
                }
                Ok(chunks)
            }
            StreamHint::Stream => {
                let mut chunks = Chunks::new();
                while let Some(chunk) = self.next_data().await {
                    chunks.push(chunk?);
                }
                Ok(chunks)
            }
        }
    }
}

impl<T: BodyExt> BodyEncodeExt for T {
    type EncodeDecodeError = EncodeDecodeError<T::Error>;
    async fn decode_content(self, encoding: String) -> Result<Bytes, Self::EncodeDecodeError> {
        let buf = self.bytes().await.map_err(EncodeDecodeError::Http)?;
        match encoding.as_str() {
            "gzip" => {
                let mut decoder = GzDecoder::new(buf.as_ref());
                let mut decompressed_data = Vec::new();
                decoder.read_to_end(&mut decompressed_data)?;
                Ok(bytes::Bytes::from(decompressed_data))
            }
            "deflate" => {
                let mut decoder = ZlibDecoder::new(buf.as_ref());
                let mut decompressed_data = Vec::new();
                decoder.read_to_end(&mut decompressed_data)?;
                Ok(bytes::Bytes::from(decompressed_data))
            }
            "br" => {
                let mut decoder = brotli::Decompressor::new(buf.as_ref(), 4096);
                let mut decompressed_data = Vec::new();
                decoder.read_to_end(&mut decompressed_data)?;
                Ok(bytes::Bytes::from(decompressed_data))
            }
            _ => {
                // Unsupported or no encoding, return original data
                Ok(buf)
            }
        }
    }

    async fn encode_content(
        self,
        accept_encoding: String,
    ) -> Result<Bytes, Self::EncodeDecodeError> {
        let buf = self.bytes().await.map_err(EncodeDecodeError::Http)?;
        let accepted_encodings: Vec<String> = accept_encoding
            .split(',')
            .map(|s| s.trim().to_string())
            .collect();

        // Find the first supported encoding from the accepted encodings
        let selected_encoding = accepted_encodings
            .iter()
            .find(|&encoding| SUPPORTED_ENCODINGS.contains(&encoding.as_str()))
            .cloned()
            .unwrap_or_else(|| "identity".to_string());

        let encoded_data = match selected_encoding.as_str() {
            "gzip" => {
                let mut encoder = GzEncoder::new(buf.as_ref(), Compression::best());
                let mut compressed_data = Vec::new();
                encoder.read_to_end(&mut compressed_data)?;
                compressed_data
            }
            "deflate" => {
                let mut encoder = ZlibEncoder::new(buf.as_ref(), Compression::best());
                let mut compressed_data = Vec::new();
                encoder.read_to_end(&mut compressed_data)?;
                compressed_data
            }
            "br" => {
                let mut encoder = brotli::CompressorReader::new(Cursor::new(buf), 4096, 11, 22);
                let mut compressed_data = Vec::new();
                encoder.read_to_end(&mut compressed_data)?;
                compressed_data
            }
            _ => {
                // Unsupported or no encoding, return original data
                buf.to_vec()
            }
        };

        Ok(bytes::Bytes::from(encoded_data))
    }
}

#[derive(Debug)]
pub enum HttpBody {
    Ready(Option<Bytes>),
    H1(Payload),
    H2(RecvStream),
    Multipart(ParsedMuliPartForm),
}

impl HttpBody {
    pub async fn to_ready(&mut self) -> Result<Option<Bytes>, HttpError> {
        match self.stream_hint() {
            StreamHint::None => Ok(None),
            StreamHint::Fixed => match self.next_data().await {
                None => {
                    *self = Self::Ready(None);
                    Ok(None)
                }
                Some(chunk) => {
                    let bytes = chunk?;
                    *self = Self::Ready(Some(bytes.clone()));
                    Ok(Some(bytes))
                }
            },
            StreamHint::Stream => {
                let mut data = BytesMut::new();
                while let Some(chunk) = self.next_data().await {
                    data.extend_from_slice(&chunk?);
                }
                let bytes = data.freeze();
                *self = Self::Ready(Some(bytes.clone()));
                Ok(Some(bytes))
            }
        }
    }

    #[inline]
    pub fn is_ready(&self) -> bool {
        matches!(self, Self::Ready(_))
    }

    #[inline]
    pub fn ready_data(&self) -> Option<Bytes> {
        match self {
            Self::Ready(b) => b.clone(),
            _ => None,
        }
    }
}

impl From<Payload> for HttpBody {
    #[inline]
    fn from(p: Payload) -> Self {
        Self::H1(p)
    }
}

impl From<RecvStream> for HttpBody {
    #[inline]
    fn from(p: RecvStream) -> Self {
        Self::H2(p)
    }
}

impl Default for HttpBody {
    #[inline]
    fn default() -> Self {
        Self::Ready(None)
    }
}

impl HttpBody {
    pub fn request<T>(req: Request<T>) -> Request<Self>
    where
        Self: From<T>,
    {
        let (parts, body) = req.into_parts();
        #[cfg(feature = "logging")]
        tracing::debug!("Request {parts:?}");
        Request::from_parts(parts, body.into())
    }

    pub fn response<T>(req: Response<T>) -> Response<Self>
    where
        Self: From<T>,
    {
        let (parts, body) = req.into_parts();
        #[cfg(feature = "logging")]
        tracing::debug!("Response {parts:?}");
        Response::from_parts(parts, body.into())
    }
}

impl Body for HttpBody {
    type Data = Bytes;
    type Error = HttpError;

    async fn next_data(&mut self) -> Option<Result<Self::Data, Self::Error>> {
        match self {
            Self::Ready(b) => b.take().map(Result::Ok),
            Self::H1(ref mut p) => p.next_data().await,
            Self::H2(ref mut p) => p.next_data().await.map(|r| r.map_err(HttpError::from)),
            Self::Multipart(ref mut p) => p.next_data().await,
        }
    }

    fn stream_hint(&self) -> StreamHint {
        match self {
            Self::Ready(Some(_)) => StreamHint::Fixed,
            Self::Ready(None) => StreamHint::None,
            Self::H1(ref p) => p.stream_hint(),
            Self::H2(ref p) => p.stream_hint(),
            Self::Multipart(ref p) => p.stream_hint(),
        }
    }
}

pub trait FixedBody: Body {
    fn fixed_body(data: Option<Bytes>) -> Self;
}

impl FixedBody for HttpBody {
    fn fixed_body(data: Option<Bytes>) -> Self {
        Self::Ready(data)
    }
}

#[derive(Debug)]
pub struct HttpBodyStream {
    inner: UnsafeCell<HttpBody>,
    stream_fut: MaybeArmedBoxFuture<Option<Result<Bytes, HttpError>>>,
}

impl HttpBodyStream {
    pub fn new(inner: HttpBody) -> Self {
        let stream_fut = MaybeArmedBoxFuture::new(async move { Some(Ok(Bytes::new())) });
        Self {
            inner: UnsafeCell::new(inner),
            stream_fut,
        }
    }
}

unsafe impl Send for HttpBodyStream {}

impl From<HttpBody> for HttpBodyStream {
    fn from(b: HttpBody) -> Self {
        Self::new(b)
    }
}

impl futures_core::Stream for HttpBodyStream {
    type Item = Result<Bytes, HttpError>;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context,
    ) -> std::task::Poll<Option<Self::Item>> {
        if !self.stream_fut.armed() {
            let body_stream = unsafe { &mut *(self.inner.get()) };
            self.stream_fut.arm_future(body_stream.next_data());
        }

        Poll::Ready(ready!(self.stream_fut.poll(cx)))
    }
}
