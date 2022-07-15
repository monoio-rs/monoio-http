use std::{fmt::Write, future::Future, io};

use bytes::BytesMut;
use http::{HeaderMap, HeaderValue, Version};
use monoio::io::{sink::Sink, stream::Stream, AsyncWriteRent, AsyncWriteRentExt};
use monoio_codec::Encoder;
use thiserror::Error as ThisError;

use crate::{
    common::request::RequestHead,
    common::{ext::Reason, response::ResponseHead, IntoParts},
    h1::payload::{Payload, PayloadError},
    ParamMut,
};

const AVERAGE_HEADER_SIZE: usize = 30;

#[derive(ThisError, Debug)]
pub enum EncodeError {
    #[error("payload error")]
    Payload(#[from] PayloadError),
    #[error("io error {0}")]
    Io(#[from] io::Error),
}

struct HeadEncoder;

impl HeadEncoder {
    fn set_length_header(header_map: &mut HeaderMap, length: Length) {
        match length {
            Length::None => (),
            Length::ContentLength(l) => {
                header_map.insert(http::header::CONTENT_LENGTH, l.into());
            }
            Length::Chunked => {
                header_map.insert(
                    http::header::TRANSFER_ENCODING,
                    HeaderValue::from_static("chunked"),
                );
            }
        }
    }
}

impl Encoder<RequestHead> for HeadEncoder {
    type Error = io::Error;

    fn encode(&mut self, item: RequestHead, dst: &mut BytesMut) -> Result<(), Self::Error> {
        self.encode(&item, dst)
    }
}

impl Encoder<&RequestHead> for HeadEncoder {
    type Error = io::Error;

    fn encode(&mut self, item: &RequestHead, dst: &mut bytes::BytesMut) -> Result<(), Self::Error> {
        dst.reserve(256 + item.headers.len() * AVERAGE_HEADER_SIZE);
        // put http method
        dst.extend_from_slice(item.method.as_str().as_bytes());
        dst.extend_from_slice(b" ");
        // put path
        dst.extend_from_slice(
            item.uri
                .path_and_query()
                .map(|u| u.as_str())
                .unwrap_or("/")
                .as_bytes(),
        );
        dst.extend_from_slice(b" ");
        // put version
        let ver = match item.version {
            Version::HTTP_09 => "HTTP/0.9\r\n",
            Version::HTTP_10 => "HTTP/1.0\r\n",
            Version::HTTP_11 => "HTTP/1.1\r\n",
            Version::HTTP_2 => "HTTP/2.0\r\n",
            Version::HTTP_3 => "HTTP/3.0\r\n",
            _ => return Err(io::Error::new(io::ErrorKind::Other, "unsupported version")),
        };
        dst.extend_from_slice(ver.as_bytes());

        // put headers
        for (name, value) in item.headers.iter() {
            dst.extend_from_slice(name.as_str().as_bytes());
            dst.extend_from_slice(b": ");
            dst.extend_from_slice(value.as_bytes());
            dst.extend_from_slice(b"\r\n");
        }
        dst.extend_from_slice(b"\r\n");
        Ok(())
    }
}

impl Encoder<ResponseHead> for HeadEncoder {
    type Error = io::Error;

    fn encode(&mut self, item: ResponseHead, dst: &mut BytesMut) -> Result<(), Self::Error> {
        self.encode(&item, dst)
    }
}

impl Encoder<&ResponseHead> for HeadEncoder {
    type Error = io::Error;

    fn encode(
        &mut self,
        item: &ResponseHead,
        dst: &mut bytes::BytesMut,
    ) -> Result<(), Self::Error> {
        dst.reserve(256 + item.headers.len() * AVERAGE_HEADER_SIZE);
        // put version
        let ver = match item.version {
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
        dst.extend_from_slice(&ver[..]);
        // put status code
        dst.extend_from_slice(item.status.as_str().as_bytes());
        dst.extend_from_slice(b" ");
        // put reason
        if let Some(reason) = item.extensions.get::<Reason>() {
            dst.extend_from_slice(reason.as_bytes());
        } else {
            let reason = item
                .status
                .canonical_reason()
                .unwrap_or("Unknown StatusCode");
            dst.extend_from_slice(reason.as_bytes());
        }
        dst.extend_from_slice(b"\r\n");

        // put headers
        for (name, value) in item.headers.iter() {
            dst.extend_from_slice(name.as_str().as_bytes());
            dst.extend_from_slice(b": ");
            dst.extend_from_slice(value.as_bytes());
            dst.extend_from_slice(b"\r\n");
        }
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

enum Length {
    None,
    ContentLength(usize),
    Chunked,
}

impl<T, R> Sink<R> for GenericEncoder<T>
where
    T: AsyncWriteRent,
    R: IntoParts<Body = Payload>,
    R::Parts: ParamMut<HeaderMap>,
    HeadEncoder: Encoder<R::Parts>,
    <HeadEncoder as Encoder<R::Parts>>::Error: Into<EncodeError>,
{
    type Error = EncodeError;

    type SendFuture<'a> = impl Future<Output = Result<(), Self::Error>> where Self: 'a;

    type FlushFuture<'a> = impl Future<Output = Result<(), Self::Error>> where Self: 'a;

    type CloseFuture<'a> = impl Future<Output = Result<(), Self::Error>> where Self: 'a;

    fn send(&mut self, item: R) -> Self::SendFuture<'_> {
        let (mut head, payload) = item.into_parts();
        async move {
            // if there is too much content in buffer, flush it first
            if self.buf.len() > BACKPRESSURE_BOUNDARY {
                Sink::<R>::flush(self).await?;
            }
            let header_map = head.param_mut();
            match payload {
                Payload::None => {
                    // set special header
                    HeadEncoder::set_length_header(header_map, Length::None);
                    // encode head to buffer
                    HeadEncoder
                        .encode(head, &mut self.buf)
                        .map_err(Into::into)?;
                }
                Payload::Fixed(p) => {
                    // get data(to set content length and body)
                    let data = p.get().await?;
                    // set special header
                    HeadEncoder::set_length_header(header_map, Length::ContentLength(data.len()));
                    // encode head to buffer
                    HeadEncoder
                        .encode(head, &mut self.buf)
                        .map_err(Into::into)?;
                    // flush
                    if self.buf.len() + data.len() > BACKPRESSURE_BOUNDARY {
                        // if data to send is too long, we will flush the buffer
                        // first, and send Bytes directly.
                        Sink::<R>::flush(self).await?;
                        let (r, _) = self.io.write_all(data).await;
                        r?;
                    } else {
                        // the data length is small, we copy it to avoid too many
                        // syscall(head and body will be sent together).
                        FixedBodyEncoder.encode(&data, &mut self.buf)?;
                    }
                }
                Payload::Stream(mut p) => {
                    // set special header
                    HeadEncoder::set_length_header(header_map, Length::Chunked);
                    // encode head to buffer
                    HeadEncoder
                        .encode(head, &mut self.buf)
                        .map_err(Into::into)?;

                    while let Some(data_result) = p.next().await {
                        let data = data_result?;
                        if self.buf.len() + data.len() > BACKPRESSURE_BOUNDARY {
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
                            FixedBodyEncoder.encode(&data, &mut self.buf)?;
                        }
                    }
                }
            }

            Ok(())
        }
    }

    fn flush(&mut self) -> Self::FlushFuture<'_> {
        async move {
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
    }

    // copied from monoio-codec
    fn close(&mut self) -> Self::CloseFuture<'_> {
        async move {
            Sink::<R>::flush(self).await?;
            self.io.shutdown().await?;
            Ok(())
        }
    }
}
