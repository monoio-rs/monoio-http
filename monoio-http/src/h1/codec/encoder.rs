use std::{fmt::Write, io};

use bytes::{BufMut, BytesMut};
use http::{StatusCode, Version};
use monoio::{
    buf::IoBuf,
    io::{sink::Sink, AsyncWriteRent, AsyncWriteRentExt},
};
use monoio_codec::Encoder;
use thiserror::Error as ThisError;

use crate::{
    common::{
        body::{Body, StreamHint},
        error::HttpError,
        ext::Reason,
        request::{RequestHead, RequestHeadRef},
        response::{ResponseHead, ResponseHeadRef},
        IntoParts,
    },
    h1::payload::PayloadError,
};

const AVERAGE_HEADER_SIZE: usize = 30;

#[derive(ThisError, Debug)]
pub enum EncodeError {
    #[error("payload error")]
    Payload(#[from] PayloadError),
    #[error("io error {0}")]
    Io(#[from] io::Error),
    #[error("payload error debug")]
    InvalidPayload(String),
}

impl Clone for EncodeError {
    fn clone(&self) -> Self {
        match self {
            Self::Payload(e) => Self::InvalidPayload(e.to_string()),
            _ => self.clone(),
        }
    }
}

struct HeadEncoder(pub Length);

impl HeadEncoder {
    #[inline]
    fn write_length(&self, dst: &mut BytesMut, force: bool) {
        match self.0 {
            Length::None => {
                if force {
                    dst.extend_from_slice(b"content-length: 0\r\n");
                }
            }
            Length::ContentLength(l) => {
                dst.extend_from_slice(http::header::CONTENT_LENGTH.as_ref());
                dst.extend_from_slice(b": ");
                let _ = write!(dst, "{l}");
                dst.extend_from_slice(b"\r\n");
            }
            Length::Chunked => {
                dst.extend_from_slice(http::header::TRANSFER_ENCODING.as_ref());
                dst.extend_from_slice(b": chunked\r\n");
            }
        }
    }

    #[inline]
    fn write_headers(headers: &http::HeaderMap<http::HeaderValue>, dst: &mut BytesMut) {
        if !headers.contains_key(http::header::CONTENT_LENGTH)
            && !headers.contains_key(http::header::TRANSFER_ENCODING)
        {
            // fast path
            for (name, value) in headers.iter() {
                dst.extend_from_slice(name.as_ref());
                dst.extend_from_slice(b": ");
                dst.extend_from_slice(value.as_ref());
                dst.extend_from_slice(b"\r\n");
            }
        } else {
            for (name, value) in headers.iter().filter(|(name, _)| {
                *name != http::header::CONTENT_LENGTH && *name != http::header::TRANSFER_ENCODING
            }) {
                dst.extend_from_slice(name.as_ref());
                dst.extend_from_slice(b": ");
                dst.extend_from_slice(value.as_ref());
                dst.extend_from_slice(b"\r\n");
            }
        }
    }
}

impl Encoder<RequestHead> for HeadEncoder {
    type Error = io::Error;

    #[inline]
    fn encode(&mut self, item: RequestHead, dst: &mut BytesMut) -> Result<(), Self::Error> {
        self.encode((&item.method, &item.uri, item.version, &item.headers), dst)
    }
}

impl Encoder<&RequestHead> for HeadEncoder {
    type Error = io::Error;

    #[inline]
    fn encode(&mut self, item: &RequestHead, dst: &mut BytesMut) -> Result<(), Self::Error> {
        self.encode((&item.method, &item.uri, item.version, &item.headers), dst)
    }
}

impl<'a> Encoder<RequestHeadRef<'a>> for HeadEncoder {
    type Error = io::Error;

    #[inline]
    fn encode(&mut self, item: RequestHeadRef<'a>, dst: &mut BytesMut) -> Result<(), Self::Error> {
        self.encode((item.method, item.uri, item.version, item.headers), dst)
    }
}

impl<'a> Encoder<&RequestHeadRef<'a>> for HeadEncoder {
    type Error = io::Error;

    #[inline]
    fn encode(&mut self, item: &RequestHeadRef<'a>, dst: &mut BytesMut) -> Result<(), Self::Error> {
        self.encode((item.method, item.uri, item.version, item.headers), dst)
    }
}

impl Encoder<(&http::Method, &http::Uri, http::Version, &http::HeaderMap)> for HeadEncoder {
    type Error = io::Error;

    fn encode(
        &mut self,
        item: (&http::Method, &http::Uri, http::Version, &http::HeaderMap),
        dst: &mut BytesMut,
    ) -> Result<(), Self::Error> {
        let (method, uri, version, headers) = item;
        // TODO: magic number here
        dst.reserve(256 + headers.len() * AVERAGE_HEADER_SIZE);
        // put http method
        dst.extend_from_slice(method.as_str().as_bytes());
        dst.extend_from_slice(b" ");
        // put path
        dst.extend_from_slice(
            uri.path_and_query()
                .map(|u| u.as_str())
                .unwrap_or("/")
                .as_bytes(),
        );
        dst.extend_from_slice(b" ");
        // put version
        let ver = match version {
            Version::HTTP_09 => b"HTTP/0.9\r\n",
            Version::HTTP_10 => b"HTTP/1.0\r\n",
            Version::HTTP_11 => b"HTTP/1.1\r\n",
            Version::HTTP_2 => b"HTTP/2.0\r\n",
            Version::HTTP_3 => b"HTTP/3.0\r\n",
            _ => return Err(io::Error::new(io::ErrorKind::Other, "unsupported version")),
        };
        dst.extend_from_slice(ver);

        // put content length or transfor encoding
        // note: should remote these headers if cannot guarantee these 2 header not exist.
        let force = headers.contains_key(http::header::CONTENT_LENGTH)
            || headers.contains_key(http::header::TRANSFER_ENCODING);
        self.write_length(dst, force);
        // put headers
        Self::write_headers(headers, dst);
        dst.extend_from_slice(b"\r\n");
        Ok(())
    }
}

impl Encoder<ResponseHead> for HeadEncoder {
    type Error = io::Error;

    #[inline]
    fn encode(&mut self, item: ResponseHead, dst: &mut BytesMut) -> Result<(), Self::Error> {
        self.encode(
            (item.status, item.version, &item.headers, &item.extensions),
            dst,
        )
    }
}

impl Encoder<&ResponseHead> for HeadEncoder {
    type Error = io::Error;

    #[inline]
    fn encode(&mut self, item: &ResponseHead, dst: &mut BytesMut) -> Result<(), Self::Error> {
        self.encode(
            (item.status, item.version, &item.headers, &item.extensions),
            dst,
        )
    }
}

impl<'a> Encoder<ResponseHeadRef<'a>> for HeadEncoder {
    type Error = io::Error;

    #[inline]
    fn encode(&mut self, item: ResponseHeadRef<'a>, dst: &mut BytesMut) -> Result<(), Self::Error> {
        self.encode(
            (item.status, item.version, item.headers, item.extensions),
            dst,
        )
    }
}

impl<'a> Encoder<&ResponseHeadRef<'a>> for HeadEncoder {
    type Error = io::Error;

    #[inline]
    fn encode(
        &mut self,
        item: &ResponseHeadRef<'a>,
        dst: &mut BytesMut,
    ) -> Result<(), Self::Error> {
        self.encode(
            (item.status, item.version, item.headers, item.extensions),
            dst,
        )
    }
}

impl
    Encoder<(
        http::StatusCode,
        http::Version,
        &http::HeaderMap<http::HeaderValue>,
        &http::Extensions,
    )> for HeadEncoder
{
    type Error = io::Error;

    fn encode(
        &mut self,
        item: (
            http::StatusCode,
            http::Version,
            &http::HeaderMap<http::HeaderValue>,
            &http::Extensions,
        ),
        dst: &mut bytes::BytesMut,
    ) -> Result<(), Self::Error> {
        let (status, version, headers, extensions) = item;
        // TODO: magic number here
        dst.reserve(256 + headers.len() * AVERAGE_HEADER_SIZE);
        // put version
        if version == Version::HTTP_11
            && status == StatusCode::OK
            && extensions.get::<Reason>().is_none()
        {
            dst.extend_from_slice(b"HTTP/1.1 200 OK\r\n");
        } else {
            let ver = match version {
                Version::HTTP_11 => b"HTTP/1.1 ",
                Version::HTTP_10 => b"HTTP/1.0 ",
                Version::HTTP_09 => b"HTTP/0.9 ",
                _ => {
                    return Err(io::Error::new(
                        io::ErrorKind::Other,
                        "unexpected http version",
                    ));
                }
            };
            dst.extend_from_slice(ver);
            // put status code
            dst.extend_from_slice(status.as_str().as_bytes());
            dst.extend_from_slice(b" ");
            // put reason
            let reason = match extensions.get::<Reason>() {
                Some(reason) => reason.as_bytes(),
                None => status.canonical_reason().unwrap_or("<none>").as_bytes(),
            };
            dst.extend_from_slice(reason);
            dst.extend_from_slice(b"\r\n");
        }

        // put content length or transfor encoding
        // note: should remote these headers if cannot guarantee these 2 header not exist.
        self.write_length(dst, true);

        // put headers
        Self::write_headers(headers, dst);
        dst.extend_from_slice(b"\r\n");
        Ok(())
    }
}

struct FixedBodyEncoder;

impl Encoder<&[u8]> for FixedBodyEncoder {
    type Error = io::Error;

    // Note: for big body, flush the buffer and send it directly to avoid copy.
    fn encode(&mut self, item: &[u8], dst: &mut bytes::BytesMut) -> Result<(), Self::Error> {
        dst.extend_from_slice(item);
        Ok(())
    }
}

#[allow(dead_code)]
struct ChunkedBodyEncoder;

impl Encoder<Option<&[u8]>> for ChunkedBodyEncoder {
    type Error = io::Error;

    fn encode(
        &mut self,
        item: Option<&[u8]>,
        dst: &mut bytes::BytesMut,
    ) -> Result<(), Self::Error> {
        let data = match item {
            Some(d) => d,
            None => {
                dst.extend_from_slice(b"0\r\n\r\n");
                return Ok(());
            }
        };
        // 8 size + \r\n + data + \r\n = 12 + data.len()
        dst.reserve(12 + data.len());
        dst.write_fmt(format_args!("{:X}\r\n", data.len()))
            .expect("write Bytes failed");
        dst.extend_from_slice(data);
        dst.extend_from_slice(b"\r\n");
        Ok(())
    }
}

/// Encoder for Request or Response(in fact it is not a encoder literally,
/// it is with io, like FramedWrite).
pub struct GenericEncoder<T> {
    io: T,
    buf: BytesMut,
}

const INITIAL_CAPACITY: usize = 8 * 1024;
const BACKPRESSURE_BOUNDARY: usize = INITIAL_CAPACITY;

impl<T> GenericEncoder<T> {
    pub fn new(io: T) -> Self {
        Self {
            io,
            buf: BytesMut::with_capacity(INITIAL_CAPACITY),
        }
    }
}

#[allow(clippy::enum_variant_names)]
enum Length {
    None,
    ContentLength(usize),
    Chunked,
}

impl<T, R> Sink<R> for GenericEncoder<T>
where
    T: AsyncWriteRent,
    R: IntoParts,
    R::Body: Body,
    HeadEncoder: Encoder<R::Parts>,
    <HeadEncoder as Encoder<R::Parts>>::Error: Into<EncodeError>,
    HttpError: From<<<R as IntoParts>::Body as Body>::Error>,
{
    type Error = HttpError;

    async fn send(&mut self, item: R) -> Result<(), Self::Error> {
        let (head, mut payload) = item.into_parts();

        // if there is too much content in buffer, flush it first
        if self.buf.len() > BACKPRESSURE_BOUNDARY {
            Sink::<R>::flush(self).await?;
        }
        let payload_type = payload.stream_hint();

        match payload_type {
            StreamHint::None => {
                let mut encoder = HeadEncoder(Length::None);
                // encode head to buffer
                encoder.encode(head, &mut self.buf).map_err(Into::into)?;
            }
            StreamHint::Fixed => {
                // get data(to set content length and body)
                let data = payload
                    .next_data()
                    .await
                    .expect("empty data with fixed hint")?;
                let mut encoder = HeadEncoder(Length::ContentLength(data.bytes_init()));
                // encode head to buffer
                encoder.encode(head, &mut self.buf).map_err(Into::into)?;
                // flush
                if self.buf.len() + data.bytes_init() > BACKPRESSURE_BOUNDARY {
                    // if data to send is too long, we will flush the buffer
                    // first, and send Bytes directly.
                    Sink::<R>::flush(self).await?;
                    let (r, _) = self.io.write_all(data).await;
                    r?;
                } else {
                    // the data length is small, we copy it to avoid too many
                    // syscall(head and body will be sent together).
                    let slice =
                        unsafe { std::slice::from_raw_parts(data.read_ptr(), data.bytes_init()) };
                    FixedBodyEncoder.encode(slice, &mut self.buf)?;
                }
            }
            StreamHint::Stream => {
                let mut encoder = HeadEncoder(Length::Chunked);
                // encode head to buffer
                encoder.encode(head, &mut self.buf).map_err(Into::into)?;

                while let Some(data_res) = payload.next_data().await {
                    let data = data_res?;
                    write!(self.buf, "{:X}\r\n", data.bytes_init())
                        .expect("unable to format data length");
                    if self.buf.len() + data.bytes_init() > BACKPRESSURE_BOUNDARY {
                        // if data to send is too long, we will flush the buffer
                        // first, and send Bytes directly.
                        if !self.buf.is_empty() {
                            Sink::<R>::flush(self).await?;
                        }
                        let (r, _) = self.io.write_all(data).await;
                        r?;
                    } else {
                        // the data length is small, we copy it to avoid too many
                        // syscall.
                        let slice = unsafe {
                            std::slice::from_raw_parts(data.read_ptr(), data.bytes_init())
                        };
                        FixedBodyEncoder.encode(slice, &mut self.buf)?;
                    }
                    self.buf.put_slice(b"\r\n");
                }
                self.buf.put_slice(b"0\r\n\r\n");
            }
        }
        Ok(())
    }

    async fn flush(&mut self) -> Result<(), Self::Error> {
        if self.buf.is_empty() {
            return Ok(());
        }
        // This action does not allocate.
        let buf = std::mem::replace(&mut self.buf, BytesMut::new());
        let (result, buf) = self.io.write_all(buf).await;
        self.buf = buf;
        result?;
        self.buf.clear();
        self.io.flush().await?;
        Ok(())
    }

    // copied from monoio-codec
    async fn close(&mut self) -> Result<(), Self::Error> {
        Sink::<R>::flush(self).await?;
        self.io.shutdown().await?;
        Ok(())
    }
}
