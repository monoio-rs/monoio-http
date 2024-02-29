use std::io::{Cursor, Read};

use bytes::{Bytes, BytesMut};
use flate2::{
    read::{GzDecoder, GzEncoder, ZlibDecoder, ZlibEncoder},
    Compression,
};
use futures_core::Future;
use monoio::buf::IoBuf;
use smallvec::SmallVec;

use super::error::HttpError;
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

pub type Chunks = SmallVec<[Bytes; 16]>;

pub trait BodyExt: Body {
    /// Consumes body and return continous memory
    fn bytes(self) -> impl Future<Output = Result<Bytes, Self::Error>>;
    /// Return bytes array
    fn chunks(&mut self) -> impl Future<Output = Result<Chunks, Self::Error>>;
    /// Consumes body and returns decodec content
    fn decode_content(
        self,
        content_encoding: String,
    ) -> impl Future<Output = Result<Bytes, Self::Error>>;
    /// Consumes body and returns encoded content
    fn encode_content(
        self,
        accept_encoding: String,
    ) -> impl Future<Output = Result<Bytes, Self::Error>>;
}

impl<T> BodyExt for T
where
    T: Body<Data = Bytes> + FixedBody + Sized,
    T::Error: From<std::io::Error>,
{
    async fn bytes(mut self) -> Result<Bytes, Self::Error> {
        match self.stream_hint() {
            StreamHint::None => Ok(Bytes::new()),
            StreamHint::Fixed => self
                .next_data()
                .await
                .expect("unable to read chunk for fixed body"),
            StreamHint::Stream => {
                let mut data = BytesMut::new();
                while let Some(chunk) = self.next_data().await {
                    data.extend_from_slice(&chunk?);
                }
                Ok(data.freeze())
            }
        }
    }

    async fn chunks(&mut self) -> Result<Chunks, Self::Error> {
        match self.stream_hint() {
            StreamHint::None => Ok(Chunks::new()),
            StreamHint::Fixed => {
                let mut chunks = Chunks::new();
                let b = self
                    .next_data()
                    .await
                    .expect("unable to read chunk for fixed body")?;
                chunks.push(b);
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

    async fn decode_content(self, encoding: String) -> Result<Bytes, Self::Error> {
        let buf = self.bytes().await?;
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

    async fn encode_content(self, accept_encoding: String) -> Result<Bytes, Self::Error> {
        let buf = self.bytes().await?;
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
}

impl From<Payload> for HttpBody {
    fn from(p: Payload) -> Self {
        Self::H1(p)
    }
}

impl From<RecvStream> for HttpBody {
    fn from(p: RecvStream) -> Self {
        Self::H2(p)
    }
}

impl Default for HttpBody {
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
        }
    }

    fn stream_hint(&self) -> StreamHint {
        match self {
            Self::Ready(Some(_)) => StreamHint::Fixed,
            Self::Ready(None) => StreamHint::None,
            Self::H1(ref p) => p.stream_hint(),
            Self::H2(ref p) => p.stream_hint(),
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
