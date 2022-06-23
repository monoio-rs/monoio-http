use std::io;

use bytes::{Buf, Bytes};
use http::{
    header::HeaderName, method::InvalidMethod, uri::InvalidUri, HeaderMap, HeaderValue, Method,
    Uri, Version,
};
use monoio_codec::Decoder;
use thiserror::Error as ThisError;

use crate::{
    h1::payload::{fixed_payload_pair, stream_payload_pair, Payload},
    request::{Request, RequestHead},
};

use super::compose::{ComposeDecoder, NextDecoder};

const MAX_HEADERS: usize = 96;

#[derive(ThisError, Debug)]
pub enum DecodeError {
    #[error("httparse error {0}")]
    Parse(#[from] httparse::Error),
    #[error("method parse error {0}")]
    Method(#[from] InvalidMethod),
    #[error("uri parse error: {0}")]
    Uri(#[from] InvalidUri),
    #[error("invalid header")]
    Header,
    #[error("chunked")]
    Chunked,
    #[error("io error {0}")]
    Io(#[from] io::Error),
}

struct HeaderIndex {
    name: (usize, usize),
    value: (usize, usize),
}

const EMPTY_HEADER_INDEX: HeaderIndex = HeaderIndex {
    name: (0, 0),
    value: (0, 0),
};

struct RequestHeaderDecoder;

impl Decoder for RequestHeaderDecoder {
    type Item = RequestHead;
    type Error = DecodeError;

    fn decode(&mut self, src: &mut bytes::BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        let mut header_indices = [EMPTY_HEADER_INDEX; MAX_HEADERS];
        let (data_len, header_len, version, method, uri) = {
            let mut headers = [httparse::EMPTY_HEADER; MAX_HEADERS];
            let mut req = httparse::Request::new(&mut headers);
            let base_ptr = src.as_ptr() as usize;

            let l = match req.parse(src)? {
                httparse::Status::Complete(l) => l,
                httparse::Status::Partial => return Ok(None),
            };
            for (h, index) in req.headers.iter().zip(header_indices.iter_mut()) {
                let n_begin = h.name.as_ptr() as usize - base_ptr;
                let n_end = n_begin + h.name.len();
                let v_begin = h.value.as_ptr() as usize - base_ptr;
                let v_end = v_begin + h.value.len();
                index.name = (n_begin, n_end);
                index.value = (v_begin, v_end);
            }
            let version = if req.version.unwrap() == 1 {
                Version::HTTP_11
            } else {
                Version::HTTP_10
            };
            let method = Method::from_bytes(req.method.unwrap().as_bytes())?;
            let uri = Uri::try_from(req.path.unwrap())?;

            (l, req.headers.len(), version, method, uri)
        };

        let data = src.split_to(data_len).freeze();
        let mut headers = HeaderMap::with_capacity(MAX_HEADERS);
        for header in header_indices.iter().take(header_len) {
            let name = HeaderName::from_bytes(&data[header.name.0..header.name.1]).unwrap();
            let value = unsafe {
                HeaderValue::from_maybe_shared_unchecked(data.slice(header.value.0..header.value.1))
            };
            headers.append(name, value);
        }

        let request_head = RequestHead {
            method,
            uri,
            version,
            headers,
        };

        Ok(Some(request_head))
    }
}

pub(crate) struct FixedBodyDecoder(usize);

impl Decoder for FixedBodyDecoder {
    type Item = Bytes;
    type Error = DecodeError;

    fn decode(&mut self, src: &mut bytes::BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        if src.len() < self.0 {
            return Ok(None);
        }
        let body = src.split_to(self.0).freeze();
        Ok(Some(body))
    }
}

#[derive(Default)]
pub(crate) struct ChunkedBodyDecoder(Option<usize>);

// TODO: support trailer
impl Decoder for ChunkedBodyDecoder {
    type Item = Option<Bytes>;
    type Error = DecodeError;

    fn decode(&mut self, src: &mut bytes::BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        loop {
            match self.0 {
                Some(len) => {
                    // Now we know how long we need
                    if src.len() < len + 2 {
                        return Ok(None);
                    }
                    // \r\n
                    if &src[len..len + 2] != b"\r\n" {
                        return Err(DecodeError::Chunked);
                    }
                    let data = if len != 0 {
                        let body = src.split_to(len).freeze();
                        Some(body)
                    } else {
                        None
                    };
                    src.advance(2);
                    self.0 = None;
                    return Ok(Some(data));
                }
                None => {
                    // We don't know what size the next block is.
                    if src.len() < 3 {
                        // There must be at least 3 bytes("0\r\n").
                        return Ok(None);
                    }
                    let mut len: usize = 0;
                    let mut read = 0;
                    for b in src.iter() {
                        let n = match b {
                            b @ b'0'..=b'9' => b - b'0',
                            b @ b'a'..=b'f' => b + 10 - b'a',
                            b @ b'A'..=b'F' => b + 10 - b'A',
                            b'\r' => break,
                            _ => return Err(DecodeError::Chunked),
                        };
                        read += 1;
                        match len.checked_mul(16) {
                            Some(new_len) => {
                                len = new_len + n as usize;
                            }
                            None => {
                                // Too big chunk size.
                                return Err(DecodeError::Chunked);
                            }
                        }
                    }
                    if len > usize::MAX - 2 {
                        // Too big chunk size.
                        return Err(DecodeError::Chunked);
                    }
                    if src.len() < read + 2 {
                        return Ok(None);
                    }
                    if &src[read..read + 2] != b"\r\n" {
                        return Err(DecodeError::Chunked);
                    }
                    src.advance(read + 2);
                    self.0 = Some(len);
                    // Now we can read data, just continue will be fine.
                    // The loop will only happen once.
                }
            }
        }
    }
}

pub(crate) struct ComposedRequestDecoder;

impl Decoder for ComposedRequestDecoder {
    type Item = (
        Request<Payload<Bytes, DecodeError>>,
        NextDecoder<FixedBodyDecoder, ChunkedBodyDecoder, Bytes>,
    );
    type Error = DecodeError;

    #[inline]
    fn decode(&mut self, src: &mut bytes::BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        match RequestHeaderDecoder.decode(src) {
            // TODO:
            // 1. iter single pass to find out content length and if is chunked
            // 2. validate headers to make sure content length can not be set with chunked encoding
            Ok(Some(head)) => {
                if let Some(content_length) = head.headers.get(http::header::CONTENT_LENGTH) {
                    let content_length = match content_length.to_str() {
                        Ok(c) if c.starts_with('+') => return Err(DecodeError::Header),
                        Ok(c) => c,
                        Err(_) => return Err(DecodeError::Header),
                    };
                    let content_length = match content_length.parse::<usize>() {
                        Ok(c) => c,
                        Err(_) => return Err(DecodeError::Header),
                    };
                    let (payload, sender) = fixed_payload_pair();
                    let request = Request {
                        head,
                        payload: payload.into(),
                    };
                    return Ok(Some((
                        request,
                        NextDecoder::Fixed(FixedBodyDecoder(content_length), sender),
                    )));
                }
                if let Some(x) = head.headers.get(http::header::TRANSFER_ENCODING) {
                    // TODO: only allow chunked and identity and ignore case.
                    if x.as_bytes() == b"chunked" {
                        let (payload, sender) = stream_payload_pair();
                        let request = Request {
                            head,
                            payload: payload.into(),
                        };
                        return Ok(Some((
                            request,
                            NextDecoder::Streamed(ChunkedBodyDecoder::default(), sender),
                        )));
                    }
                }

                let request = Request {
                    head,
                    payload: Payload::None,
                };
                Ok(Some((request, NextDecoder::None)))
            }
            Ok(None) => Ok(None),
            Err(e) => Err(e),
        }
    }
}

pub(crate) type RequestDecoder = ComposeDecoder<
    ComposedRequestDecoder,
    Request<Payload<Bytes, DecodeError>>,
    FixedBodyDecoder,
    ChunkedBodyDecoder,
    Bytes,
>;

pub(crate) fn new_request_decoder() -> RequestDecoder {
    ComposeDecoder::new(ComposedRequestDecoder)
}

#[cfg(test)]
mod tests {
    use bytes::BytesMut;
    use monoio::io::stream::Stream;

    use super::*;

    #[test]
    fn decode_header() {
        let mut data = BytesMut::from("GET /test HTTP/1.1\r\n\r\n");
        let head = RequestHeaderDecoder.decode(&mut data).unwrap().unwrap();
        assert_eq!(head.method, Method::GET);
        assert_eq!(head.version, Version::HTTP_11);
        assert_eq!(head.uri, "/test");
    }

    #[test]
    fn decode_fixed_body() {
        let mut data = BytesMut::from("balabalabalabala");
        let mut decoder = FixedBodyDecoder(8);
        let head = decoder.decode(&mut data).unwrap().unwrap();
        assert_eq!(&head, &"balabala")
    }

    #[test]
    fn decode_chunked_body() {
        let mut data = BytesMut::from("a\r\n0000000000\r\n1\r\nx\r\n0\r\n\r\n");
        let mut decoder = ChunkedBodyDecoder::default();
        assert_eq!(
            &decoder.decode(&mut data).unwrap().unwrap().unwrap(),
            &"0000000000"
        );
        assert_eq!(&decoder.decode(&mut data).unwrap().unwrap().unwrap(), &"x");
        assert!(decoder.decode(&mut data).unwrap().unwrap().is_none());
    }

    #[test]
    fn decode_too_big_chunked_body() {
        let mut data = BytesMut::from("a\r\n0000000000\r\ndeadbeefcafebabe0\r\nx\r\n0\r\n\r\n");
        let mut decoder = ChunkedBodyDecoder::default();
        assert_eq!(
            &decoder.decode(&mut data).unwrap().unwrap().unwrap(),
            &"0000000000"
        );
        assert!(&decoder.decode(&mut data).is_err());
    }

    #[test]
    fn decode_get_request() {
        let mut data = BytesMut::from("GET /test HTTP/1.1\r\n\r\n");
        let mut decoder = new_request_decoder();
        let req = decoder.decode(&mut data).unwrap().unwrap();
        assert_eq!(req.head.method, Method::GET);
        assert!(matches!(req.payload, Payload::None));
    }

    #[monoio::test_all]
    async fn decode_fixed_request() {
        let mut data = BytesMut::from("POST /test HTTP/1.1\r\nContent-Length: 4\r\n\r\nbody");
        let mut decoder = new_request_decoder();
        let req = decoder.decode(&mut data).unwrap().unwrap();
        assert_eq!(req.head.method, Method::POST);
        let payload = match req.payload {
            Payload::Fixed(p) => p,
            _ => panic!("wrong payload type"),
        };
        let handler = monoio::spawn(async move {
            let data = payload.get().await.unwrap();
            assert_eq!(&data, &"body");
        });
        assert!(decoder.decode_eof(&mut data).unwrap().is_none());
        handler.await
    }

    #[monoio::test_all]
    async fn decode_chunked_request() {
        let mut data = BytesMut::from(
            "PUT /test HTTP/1.1\r\ntransfer-encoding: chunked\r\n\r\n\
            4\r\ndata\r\n4\r\nline\r\n0\r\n\r\n",
        );
        let mut decoder = new_request_decoder();
        let req = decoder.decode(&mut data).unwrap().unwrap();
        assert_eq!(req.head.method, Method::PUT);
        let mut payload = match req.payload {
            Payload::Stream(p) => p,
            _ => panic!("wrong payload type"),
        };
        let handler = monoio::spawn(async move {
            assert_eq!(&payload.next().await.unwrap().unwrap(), &"data");
            assert_eq!(&payload.next().await.unwrap().unwrap(), &"line");
            assert!(payload.next().await.is_none());
        });
        assert!(decoder.decode_eof(&mut data).unwrap().is_none());
        handler.await
    }
}
