use std::{fmt::Write, future::Future, io};

use bytes::{Buf, Bytes, BytesMut};
use http::Version;
use monoio::{
    buf::RawBuf,
    io::{sink::Sink, stream::Stream, AsyncWriteRent, AsyncWriteRentExt},
};
use monoio_codec::Encoder;

use crate::{
    common::request::RequestHead,
    common::{response::ResponseHead, ReqOrResp},
    h1::payload::Payload,
};

const AVERAGE_HEADER_SIZE: usize = 30;

struct HeadEncoder;

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
        if let Some(reason) = &item.reason {
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

pub struct ReqOrRespEncoder<T> {
    io: T,
    buf: BytesMut,
}

const INITIAL_CAPACITY: usize = 8 * 1024;
const BACKPRESSURE_BOUNDARY: usize = INITIAL_CAPACITY;

impl<T> ReqOrRespEncoder<T> {
    pub fn new(io: T) -> Self {
        Self {
            io,
            buf: BytesMut::with_capacity(INITIAL_CAPACITY),
        }
    }
}

impl<T, H> Sink<ReqOrResp<H, Payload<Bytes, io::Error>>> for ReqOrRespEncoder<T>
where
    T: AsyncWriteRent,
    HeadEncoder: Encoder<H, Error = io::Error>,
{
    type Error = io::Error;

    type SendFuture<'a> = impl Future<Output = Result<(), Self::Error>> where Self: 'a;

    type FlushFuture<'a> = impl Future<Output = Result<(), Self::Error>> where Self: 'a;

    type CloseFuture<'a> = impl Future<Output = Result<(), Self::Error>> where Self: 'a;

    fn send(&mut self, item: ReqOrResp<H, Payload<Bytes, io::Error>>) -> Self::SendFuture<'_> {
        async move {
            // if there is too many content in buffer, flush it first
            if self.buf.len() > BACKPRESSURE_BOUNDARY {
                self.flush().await?;
            }
            // encode head to buffer
            HeadEncoder.encode(item.head, &mut self.buf)?;
            match item.payload {
                // if there is no payload, flush it now and return.
                Payload::None => {
                    self.flush().await?;
                }
                Payload::Fixed(p) => {
                    let data = p.get().await?;
                    if self.buf.len() + data.len() > BACKPRESSURE_BOUNDARY {
                        // if data to send is too long, we will flush the buffer
                        // first, and send Bytes directly.
                        self.flush().await?;
                        let (r, _) = self.io.write_all(data).await;
                        r?;
                    } else {
                        // the data length is small, we copy it to avoid too many
                        // syscall(head and body will be sent together).
                        FixedBodyEncoder.encode(&data, &mut self.buf)?;
                        self.flush().await?;
                    }
                }
                Payload::Stream(mut p) => {
                    while let Some(data_result) = p.next().await {
                        let data = data_result?;
                        if self.buf.len() + data.len() > BACKPRESSURE_BOUNDARY {
                            // if data to send is too long, we will flush the buffer
                            // first, and send Bytes directly.
                            if !self.buf.is_empty() {
                                self.flush().await?;
                            }
                            let (r, _) = self.io.write_all(data).await;
                            r?;
                        } else {
                            // the data length is small, we copy it to avoid too many
                            // syscall.
                            FixedBodyEncoder.encode(&data, &mut self.buf)?;
                        }
                    }
                    if !self.buf.is_empty() {
                        self.flush().await?;
                    }
                }
            }
            Ok(())
        }
    }

    // copied from monoio-codec
    fn flush(&mut self) -> Self::FlushFuture<'_> {
        async move {
            while !self.buf.is_empty() {
                let buf = self.buf.chunk();
                let raw_buf = unsafe { RawBuf::new(buf.as_ptr(), buf.len()) };
                match self.io.write(raw_buf).await.0 {
                    Ok(0) => {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::UnexpectedEof,
                            "failed to write whole buffer",
                        ));
                    }
                    Ok(n) => {
                        self.buf.advance(n);
                    }
                    Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                    Err(e) => return Err(e),
                }
            }
            self.io.flush().await?;
            Ok(())
        }
    }

    // copied from monoio-codec
    fn close(&mut self) -> Self::CloseFuture<'_> {
        async move {
            self.flush().await?;
            self.io.shutdown().await?;
            Ok(())
        }
    }
}
