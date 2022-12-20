use std::{borrow::Cow, future::Future, hint::unreachable_unchecked, io, marker::PhantomData};

use bytes::{Buf, Bytes};
use http::{
    header::HeaderName, method::InvalidMethod, status::InvalidStatusCode, uri::InvalidUri,
    HeaderMap, HeaderValue, Method, StatusCode, Uri, Version,
};
use monoio::io::{stream::Stream, AsyncReadRent};
use monoio_codec::{Decoder, FramedRead};
use thiserror::Error as ThisError;

use crate::{
    common::{
        ext::Reason,
        request::{Request, RequestHead},
        response::{Response, ResponseHead},
        FromParts,
    },
    h1::payload::{
        fixed_payload_pair, stream_payload_pair, FixedPayloadSender, Payload, PayloadError,
        StreamPayloadSender,
    },
    ParamRef,
};

const MAX_HEADERS: usize = 96;

#[derive(ThisError, Debug)]
pub enum DecodeError {
    #[error("httparse error {0}")]
    Parse(#[from] httparse::Error),
    #[error("method parse error {0}")]
    Method(#[from] InvalidMethod),
    #[error("uri parse error: {0}")]
    Uri(#[from] InvalidUri),
    #[error("status code parse error: {0}")]
    Status(#[from] InvalidStatusCode),
    #[error("invalid header")]
    Header,
    #[error("chunked")]
    Chunked,
    #[error("io error {0}")]
    Io(#[from] io::Error),
    #[error("unexpected eof")]
    UnexpectedEof,
}

/// NextDecoder maybe None, Fixed or Streamed.
/// Mainly designed for no body, fixed-length body and chunked body.
/// But generally, NextDecoder can be used to represent 0, 1, or more
/// than 1 things to decode.
pub enum NextDecoder<FDE, SDE, BI>
where
    FDE: Decoder<Item = BI>,
    SDE: Decoder<Item = Option<BI>>,
{
    None,
    Fixed(FDE, FixedPayloadSender<BI>),
    Streamed(SDE, StreamPayloadSender<BI>),
}

impl<FDE, SDE, BI> Default for NextDecoder<FDE, SDE, BI>
where
    FDE: Decoder<Item = BI>,
    SDE: Decoder<Item = Option<BI>>,
{
    fn default() -> Self {
        NextDecoder::None
    }
}

struct HeaderIndex {
    name: (usize, usize),
    value: (usize, usize),
}

const EMPTY_HEADER_INDEX: HeaderIndex = HeaderIndex {
    name: (0, 0),
    value: (0, 0),
};

/// Decoder of http1 request header.
#[derive(Default)]
pub struct RequestHeadDecoder;

impl Decoder for RequestHeadDecoder {
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

        let (mut request_head, _) = http::request::Request::new(()).into_parts();
        request_head.method = method;
        request_head.uri = uri;
        request_head.version = version;
        request_head.headers = headers;

        Ok(Some(request_head))
    }
}

// TODO: less code copy
/// Decoder of http1 response header.
#[derive(Default)]
pub struct ResponseHeadDecoder;

impl Decoder for ResponseHeadDecoder {
    type Item = ResponseHead;
    type Error = DecodeError;

    fn decode(&mut self, src: &mut bytes::BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        let mut header_indices = [EMPTY_HEADER_INDEX; MAX_HEADERS];
        let (data_len, header_len, version, status, reason) = {
            let mut headers = [httparse::EMPTY_HEADER; MAX_HEADERS];
            let mut res = httparse::Response::new(&mut headers);
            let base_ptr = src.as_ptr() as usize;

            let l = match res.parse(src)? {
                httparse::Status::Complete(l) => l,
                httparse::Status::Partial => return Ok(None),
            };
            for (h, index) in res.headers.iter().zip(header_indices.iter_mut()) {
                let n_begin = h.name.as_ptr() as usize - base_ptr;
                let n_end = n_begin + h.name.len();
                let v_begin = h.value.as_ptr() as usize - base_ptr;
                let v_end = v_begin + h.value.len();
                index.name = (n_begin, n_end);
                index.value = (v_begin, v_end);
            }
            let version = if res.version.unwrap() == 1 {
                Version::HTTP_11
            } else {
                Version::HTTP_10
            };
            let status = StatusCode::from_u16(res.code.unwrap())?;
            let reason = res.reason.and_then(|r| {
                if Some(r) == status.canonical_reason() {
                    None
                } else {
                    Some(Cow::Owned(r.to_owned()))
                }
            });

            (l, res.headers.len(), version, status, reason)
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

        let (mut response_head, _) = http::response::Response::new(()).into_parts();
        response_head.version = version;
        response_head.status = status;
        response_head.headers = headers;

        if let Some(reason) = reason {
            response_head.extensions.insert(Reason::from(reason));
        }

        Ok(Some(response_head))
    }
}

/// Decoder of http1 body with fixed length.
pub struct FixedBodyDecoder(usize);

impl Decoder for FixedBodyDecoder {
    type Item = Bytes;
    type Error = DecodeError;

    #[inline]
    fn decode(&mut self, src: &mut bytes::BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        if src.len() < self.0 {
            return Ok(None);
        }
        let body = src.split_to(self.0).freeze();
        Ok(Some(body))
    }
}

// TODO: support trailer
/// Decoder of http1 chunked body.
#[derive(Default)]
pub struct ChunkedBodyDecoder(Option<usize>);

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

/// A wrapper around D(normally RequestHeaderDecoder and ResponseHeaderDecoder).
/// Mainly for extract special headers and return the raw item and payload to
/// satisfy the constraint of `ComposeDecoder`.
pub struct GenericHeadDecoder<R, D> {
    decoder: D,
    _marker: PhantomData<R>,
}

impl<R, D> GenericHeadDecoder<R, D> {
    pub fn new(decoder: D) -> Self {
        Self {
            decoder,
            _marker: PhantomData,
        }
    }
}

impl<R, D: Default> Default for GenericHeadDecoder<R, D> {
    fn default() -> Self {
        Self::new(D::default())
    }
}

impl<R, D> Decoder for GenericHeadDecoder<R, D>
where
    D: Decoder<Error = DecodeError>,
    R: FromParts<D::Item, Payload>,
    D::Item: ParamRef<HeaderMap>,
{
    type Item = (R, NextDecoder<FixedBodyDecoder, ChunkedBodyDecoder, Bytes>);
    type Error = DecodeError;

    #[inline]
    fn decode(&mut self, src: &mut bytes::BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        match self.decoder.decode(src) {
            // TODO:
            // 1. iter single pass to find out content length and if is chunked
            // 2. validate headers to make sure content length can not be set with chunked encoding
            Ok(Some(head)) => {
                if let Some(content_length) = head.param_ref().get(http::header::CONTENT_LENGTH) {
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
                    let request = R::from_parts(head, Payload::from(payload));
                    return Ok(Some((
                        request,
                        NextDecoder::Fixed(FixedBodyDecoder(content_length), sender),
                    )));
                }
                if let Some(x) = head.param_ref().get(http::header::TRANSFER_ENCODING) {
                    // TODO: only allow chunked and identity and ignore case.
                    if x.as_bytes() == b"chunked" {
                        let (payload, sender) = stream_payload_pair();
                        let request = R::from_parts(head, Payload::from(payload));
                        return Ok(Some((
                            request,
                            NextDecoder::Streamed(ChunkedBodyDecoder::default(), sender),
                        )));
                    }
                }

                let request = R::from_parts(head, Payload::None);
                Ok(Some((request, NextDecoder::None)))
            }
            Ok(None) => Ok(None),
            Err(e) => Err(e),
        }
    }
}

pub struct GenericDecoder<IO, HD> {
    framed: FramedRead<IO, HD>,
    next_decoder: NextDecoder<FixedBodyDecoder, ChunkedBodyDecoder, Bytes>,
}

impl<IO, HD> GenericDecoder<IO, HD>
where
    HD: Default,
{
    pub fn new(io: IO) -> Self {
        Self {
            framed: FramedRead::new(io, HD::default()),
            next_decoder: NextDecoder::default(),
        }
    }
}

pub trait FillPayload {
    type Error;
    type FillPayloadFuture<'a>: std::future::Future<Output = Result<(), Self::Error>>
    where
        Self: 'a;

    fn fill_payload(&mut self) -> Self::FillPayloadFuture<'_>;
}

impl<IO, HD, I> FillPayload for GenericDecoder<IO, HD>
where
    IO: AsyncReadRent,
    HD: Decoder<
        Item = (I, NextDecoder<FixedBodyDecoder, ChunkedBodyDecoder, Bytes>),
        Error = DecodeError,
    >,
{
    type Error = DecodeError;

    type FillPayloadFuture<'a> = impl std::future::Future<Output = Result<(), Self::Error>> + 'a
    where
        Self: 'a;

    fn fill_payload(&mut self) -> Self::FillPayloadFuture<'_> {
        async move {
            loop {
                match &mut self.next_decoder {
                    // If there is no next_decoder, use main decoder
                    NextDecoder::None => {
                        return Ok(());
                    }
                    NextDecoder::Fixed(_, _) => {
                        // Swap sender out
                        let (mut decoder, sender) =
                            match std::mem::replace(&mut self.next_decoder, NextDecoder::None) {
                                NextDecoder::None => unsafe { unreachable_unchecked() },
                                NextDecoder::Fixed(decoder, sender) => (decoder, sender),
                                NextDecoder::Streamed(_, _) => unsafe { unreachable_unchecked() },
                            };
                        match self.framed.next_with(&mut decoder).await {
                            // EOF
                            None => {
                                sender.feed(Err(PayloadError::UnexpectedEof));
                                return Err(DecodeError::UnexpectedEof);
                            }
                            Some(Ok(item)) => {
                                sender.feed(Ok(item));
                            }
                            Some(Err(e)) => {
                                sender.feed(Err(PayloadError::Decode));
                                return Err(e);
                            }
                        }
                    }
                    NextDecoder::Streamed(decoder, sender) => {
                        match self.framed.next_with(decoder).await {
                            // EOF
                            None => {
                                sender.feed_error(PayloadError::UnexpectedEof);
                                return Err(DecodeError::UnexpectedEof);
                            }
                            Some(Ok(item)) => {
                                // Send data
                                match item {
                                    Some(item) => {
                                        sender.feed_data(Some(item));
                                    }
                                    None => {
                                        sender.feed_data(None);
                                        self.next_decoder = NextDecoder::None;
                                    }
                                }
                            }
                            Some(Err(e)) => {
                                // Send error
                                sender.feed_error(PayloadError::Decode);
                                return Err(e);
                            }
                        }
                    }
                }
            }
        }
    }
}

impl<IO, HD, I> Stream for GenericDecoder<IO, HD>
where
    IO: AsyncReadRent,
    HD: Decoder<
        Item = (I, NextDecoder<FixedBodyDecoder, ChunkedBodyDecoder, Bytes>),
        Error = DecodeError,
    >,
{
    type Item = Result<I, DecodeError>;
    type NextFuture<'a> = impl Future<Output = Option<Self::Item>> + 'a where Self: 'a;

    fn next(&mut self) -> Self::NextFuture<'_> {
        async move {
            if !matches!(self.next_decoder, NextDecoder::None) {
                if let Err(e) = self.fill_payload().await {
                    return Some(Err(e));
                }
            }

            match self.framed.next().await {
                None => None,
                Some(Ok((item, next_decoder))) => {
                    self.next_decoder = next_decoder;
                    Some(Ok(item))
                }
                Some(Err(e)) => Some(Err(e)),
            }
        }
    }
}

pub type RequestDecoder<IO> = GenericDecoder<IO, GenericHeadDecoder<Request, RequestHeadDecoder>>;

pub type ResponseDecoder<IO> =
    GenericDecoder<IO, GenericHeadDecoder<Response, ResponseHeadDecoder>>;

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use bytes::BytesMut;
    use monoio::{buf::IoVecWrapperMut, io::stream::Stream};

    use super::*;

    #[test]
    fn decode_request_header() {
        let mut data = BytesMut::from("GET /test HTTP/1.1\r\n\r\n");
        let head = RequestHeadDecoder.decode(&mut data).unwrap().unwrap();
        assert_eq!(head.method, Method::GET);
        assert_eq!(head.version, Version::HTTP_11);
        assert_eq!(head.uri, "/test");
        assert!(data.is_empty());
    }

    #[test]
    fn decode_response_header() {
        let mut data = BytesMut::from("HTTP/1.1 200 OK\r\n\r\n");
        let head = ResponseHeadDecoder.decode(&mut data).unwrap().unwrap();
        assert_eq!(head.status, StatusCode::OK);
        assert_eq!(head.version, Version::HTTP_11);
        assert!(data.is_empty());
    }

    #[test]
    fn decode_fixed_body() {
        let mut data = BytesMut::from("balabalabalabala");
        let mut decoder = FixedBodyDecoder(8);
        let head = decoder.decode(&mut data).unwrap().unwrap();
        assert_eq!(&head, &"balabala");
        assert_eq!(data.len(), 8);
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

    macro_rules! mock {
        ($($x:expr),*) => {{
            let mut v = VecDeque::new();
            v.extend(vec![$($x),*]);
            Mock { calls: v }
        }};
    }

    #[monoio::test_all]
    async fn decode_request_without_body() {
        let io = mock! { Ok(b"GET /test HTTP/1.1\r\n\r\n".to_vec()) };
        let mut decoder = RequestDecoder::new(io);
        let req = decoder.next().await.unwrap().unwrap();
        assert_eq!(req.method(), Method::GET);
        assert!(matches!(req.body(), Payload::None));
    }

    #[monoio::test_all]
    async fn decode_response_without_body() {
        let io = mock! { Ok(b"HTTP/1.1 200 OK\r\n\r\n".to_vec()) };
        let mut decoder = ResponseDecoder::new(io);
        let req = decoder.next().await.unwrap().unwrap();
        assert_eq!(req.status(), StatusCode::OK);
        assert!(matches!(req.body(), Payload::None));
    }

    #[monoio::test_all]
    async fn decode_fixed_body_request() {
        let io = mock! { Ok(b"POST /test HTTP/1.1\r\nContent-Length: 4\r\ntest-key: test-val\r\n\r\nbody".to_vec()) };
        let mut decoder = RequestDecoder::new(io);
        let req = decoder.next().await.unwrap().unwrap();
        assert_eq!(req.method(), Method::POST);
        assert_eq!(req.headers().get("test-key").unwrap(), "test-val");
        let payload = match req.into_body() {
            Payload::Fixed(p) => p,
            _ => panic!("wrong payload type"),
        };
        assert!(decoder.fill_payload().await.is_ok());
        let data = payload.get().await.unwrap();
        assert_eq!(&data, &"body");
        assert!(decoder.next().await.is_none());
    }

    #[monoio::test_all]
    async fn decode_fixed_body_response() {
        let io = mock! { Ok(b"HTTP/1.1 200 OK\r\ncontent-lenGth: 4\r\ntest-key: test-val\r\n\r\nbody".to_vec()) };
        let mut decoder = ResponseDecoder::new(io);
        let req = decoder.next().await.unwrap().unwrap();
        assert_eq!(req.status(), StatusCode::OK);
        assert_eq!(req.headers().get("test-key").unwrap(), "test-val");
        let payload = match req.into_body() {
            Payload::Fixed(p) => p,
            _ => panic!("wrong payload type"),
        };
        assert!(decoder.fill_payload().await.is_ok());
        let data = payload.get().await.unwrap();
        assert_eq!(&data, &"body");
        assert!(decoder.next().await.is_none());
    }

    #[monoio::test_all]
    async fn decode_chunked_request() {
        let io = mock! { Ok(b"PUT /test HTTP/1.1\r\ntransfer-encoding: chunked\r\n\r\n\
        4\r\ndata\r\n4\r\nline\r\n0\r\n\r\n".to_vec()) };
        let mut decoder = RequestDecoder::new(io);
        let req = decoder.next().await.unwrap().unwrap();
        assert_eq!(req.method(), Method::PUT);
        let mut payload = match req.into_body() {
            Payload::Stream(p) => p,
            _ => panic!("wrong payload type"),
        };
        // Here we use spawn to read the body because calling next will not do real io.
        // We must do decode to push the streaming body.
        // There are two choices: spawn or select.
        // Use spawn is easy for testing. However, for better performance, use select
        // in hot path.
        let handler = monoio::spawn(async move {
            assert_eq!(&payload.next().await.unwrap().unwrap(), &"data");
            assert_eq!(&payload.next().await.unwrap().unwrap(), &"line");
            assert!(payload.next().await.is_none());
        });
        assert!(decoder.fill_payload().await.is_ok());
        assert!(decoder.next().await.is_none());
        handler.await
    }

    #[monoio::test_all]
    async fn decode_chunked_response() {
        let io = mock! { Ok(b"HTTP/1.1 200 OK\r\nTransfer-encoDing: chunked\r\n\r\n\
        4\r\ndata\r\n4\r\nline\r\n0\r\n\r\n".to_vec()) };
        let mut decoder = ResponseDecoder::new(io);
        let resp = decoder.next().await.unwrap().unwrap();
        let mut payload = match resp.into_body() {
            Payload::Stream(p) => p,
            _ => panic!("wrong payload type"),
        };
        let handler = monoio::spawn(async move {
            assert_eq!(&payload.next().await.unwrap().unwrap(), &"data");
            assert_eq!(&payload.next().await.unwrap().unwrap(), &"line");
            assert!(payload.next().await.is_none());
        });
        assert!(decoder.fill_payload().await.is_ok());
        assert!(decoder.next().await.is_none());
        handler.await
    }

    // Mock struct copied from monoio-codec and tokio-util.
    struct Mock {
        calls: VecDeque<io::Result<Vec<u8>>>,
    }

    impl AsyncReadRent for Mock {
        type ReadFuture<'a, B> = impl std::future::Future<Output = monoio::BufResult<usize, B>> + 'a where
                B: monoio::buf::IoBufMut + 'a;
        type ReadvFuture<'a, B> = impl std::future::Future<Output = monoio::BufResult<usize, B>> + 'a where
                B: monoio::buf::IoVecBufMut + 'a;

        fn read<T: monoio::buf::IoBufMut>(&mut self, mut buf: T) -> Self::ReadFuture<'_, T> {
            async {
                match self.calls.pop_front() {
                    Some(Ok(data)) => {
                        let n = data.len();
                        debug_assert!(buf.bytes_total() >= n);
                        unsafe {
                            buf.write_ptr().copy_from_nonoverlapping(data.as_ptr(), n);
                            buf.set_init(n)
                        }
                        (Ok(n), buf)
                    }
                    Some(Err(e)) => (Err(e), buf),
                    None => (Ok(0), buf),
                }
            }
        }

        fn readv<T: monoio::buf::IoVecBufMut>(&mut self, mut buf: T) -> Self::ReadvFuture<'_, T> {
            async move {
                let slice = match IoVecWrapperMut::new(buf) {
                    Ok(slice) => slice,
                    Err(buf) => return (Ok(0), buf),
                };

                let (result, slice) = self.read(slice).await;
                buf = slice.into_inner();
                if let Ok(n) = result {
                    unsafe { buf.set_init(n) };
                }
                (result, buf)
            }
        }
    }
}
