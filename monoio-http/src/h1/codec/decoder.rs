use std::{
    borrow::Cow, future::Future, hint::unreachable_unchecked, io, marker::PhantomData,
    time::Duration,
};

use bytes::{Buf, Bytes};
use http::{
    header::HeaderName, method::InvalidMethod, status::InvalidStatusCode, uri::InvalidUri,
    HeaderMap, HeaderValue, Method, StatusCode, Uri, Version,
};
use monoio::io::{stream::Stream, AsyncReadRent, OwnedReadHalf};
use monoio_codec::{Decoded, Decoder, FramedRead};
use thiserror::Error as ThisError;

use crate::{
    common::{
        body::StreamHint,
        error::HttpError,
        ext::Reason,
        request::{Request, RequestHead},
        response::{Response, ResponseHead},
        BorrowHeaderMap, FromParts,
    },
    h1::{
        payload::{
            fixed_payload_pair, stream_payload_pair, FixedPayloadSender, FramedPayload, Payload,
            PayloadError, StreamPayloadSender,
        },
        BorrowFramedRead,
    },
};

const MAX_HEADERS: usize = 96;

#[derive(ThisError, Debug)]
pub enum InvalidRequestError {
    #[error("Invalid method error {0}")]
    InvalidMethod(String),
    #[error("Invalid uri error {0}")]
    InvalidUri(String),
    #[error("Invalid status error {0}")]
    InvalidStatus(String),
}

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
    #[error("timeout error")]
    TimedOut,
    #[error("invalid error {0}")]
    Invalid(InvalidRequestError),
}

impl Clone for DecodeError {
    fn clone(&self) -> Self {
        match self {
            Self::Method(e) => Self::Invalid(InvalidRequestError::InvalidMethod(e.to_string())),
            Self::Uri(e) => Self::Invalid(InvalidRequestError::InvalidUri(e.to_string())),
            Self::Status(e) => Self::Invalid(InvalidRequestError::InvalidStatus(e.to_string())),
            _ => self.clone(),
        }
    }
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

#[allow(clippy::derivable_impls)]
impl<FDE, SDE, BI> Default for NextDecoder<FDE, SDE, BI>
where
    FDE: Decoder<Item = BI>,
    SDE: Decoder<Item = Option<BI>>,
{
    fn default() -> Self {
        NextDecoder::None
    }
}

pub enum PayloadDecoder<FDE, SDE> {
    None,
    Fixed(FDE),
    Streamed(SDE),
}

impl<FDE, SDE> PayloadDecoder<FDE, SDE> {
    pub fn hint(&self) -> StreamHint {
        match self {
            PayloadDecoder::None => StreamHint::None,
            PayloadDecoder::Fixed(_) => StreamHint::Fixed,
            PayloadDecoder::Streamed(_) => StreamHint::Stream,
        }
    }
}

/// Decoder of http1 request header.
#[derive(Default)]
pub struct RequestHeadDecoder;

impl Decoder for RequestHeadDecoder {
    type Item = RequestHead;
    type Error = DecodeError;

    fn decode(&mut self, src: &mut bytes::BytesMut) -> Result<Decoded<Self::Item>, Self::Error> {
        let header_data = match memchr::memmem::find(src, b"\r\n\r\n")
            .map(|idx| src.split_to(idx + 4).freeze())
        {
            Some(h) => h,
            None => return Ok(Decoded::Insufficient),
        };
        let base_ptr = header_data.as_ptr() as usize;
        let mut headers = [httparse::EMPTY_HEADER; MAX_HEADERS];
        let mut req = httparse::Request::new(&mut headers);
        let parse_status = req.parse(&header_data)?;
        if httparse::Status::Partial == parse_status {
            return Ok(Decoded::Insufficient);
        }
        let mut headers = HeaderMap::with_capacity(req.headers.len());
        for h in req.headers.iter() {
            let n_begin = h.name.as_ptr() as usize - base_ptr;
            let n_end = n_begin + h.name.len();
            let v_begin = h.value.as_ptr() as usize - base_ptr;
            let v_end = v_begin + h.value.len();
            let name = HeaderName::from_bytes(&header_data[n_begin..n_end]).unwrap();
            let value = unsafe {
                HeaderValue::from_maybe_shared_unchecked(header_data.slice(v_begin..v_end))
            };
            headers.append(name, value);
        }
        let version = match req.version {
            Some(1) => Version::HTTP_11,
            _ => Version::HTTP_10,
        };
        let method = Method::from_bytes(req.method.unwrap().as_bytes())?;
        let uri = match req.path {
            Some("/") => Uri::default(),
            Some(path) => {
                let uri_start = path.as_bytes().as_ptr() as usize - base_ptr;
                let uri_end = uri_start + path.len();
                Uri::from_maybe_shared(header_data.slice(uri_start..uri_end))?
            }
            _ => Uri::default(),
        };

        let (mut request_head, _) = http::request::Request::new(()).into_parts();
        request_head.method = method;
        request_head.uri = uri;
        request_head.version = version;
        request_head.headers = headers;

        Ok(Decoded::Some(request_head))
    }
}

// TODO: less code copy
/// Decoder of http1 response header.
#[derive(Default)]
pub struct ResponseHeadDecoder;

impl Decoder for ResponseHeadDecoder {
    type Item = ResponseHead;
    type Error = DecodeError;

    fn decode(&mut self, src: &mut bytes::BytesMut) -> Result<Decoded<Self::Item>, Self::Error> {
        let header_data = match memchr::memmem::find(src, b"\r\n\r\n")
            .map(|idx| src.split_to(idx + 4).freeze())
        {
            Some(h) => h,
            None => return Ok(Decoded::Insufficient),
        };
        let base_ptr = header_data.as_ptr() as usize;
        let mut headers = [httparse::EMPTY_HEADER; MAX_HEADERS];
        let mut res = httparse::Response::new(&mut headers);
        let parse_status = res.parse(&header_data)?;
        if httparse::Status::Partial == parse_status {
            return Ok(Decoded::Insufficient);
        }

        let mut headers = HeaderMap::with_capacity(res.headers.len());
        for h in res.headers.iter() {
            let n_begin = h.name.as_ptr() as usize - base_ptr;
            let n_end = n_begin + h.name.len();
            let v_begin = h.value.as_ptr() as usize - base_ptr;
            let v_end = v_begin + h.value.len();
            let name = HeaderName::from_bytes(&header_data[n_begin..n_end]).unwrap();
            let value = unsafe {
                HeaderValue::from_maybe_shared_unchecked(header_data.slice(v_begin..v_end))
            };
            headers.append(name, value);
        }
        let version = match res.version {
            Some(1) => Version::HTTP_11,
            _ => Version::HTTP_10,
        };
        let status = StatusCode::from_u16(res.code.unwrap())?;
        let reason = match res.reason {
            Some(r) if Some(r) == status.canonical_reason() => None,
            Some(r) => Some(Cow::Owned(r.to_owned())),
            None => None,
        };

        let (mut response_head, _) = http::response::Response::new(()).into_parts();
        response_head.version = version;
        response_head.status = status;
        response_head.headers = headers;

        if let Some(reason) = reason {
            response_head.extensions.insert(Reason::from(reason));
        }

        Ok(Decoded::Some(response_head))
    }
}

/// Decoder of http1 body with fixed length.
pub struct FixedBodyDecoder(usize);

impl Decoder for FixedBodyDecoder {
    type Item = Bytes;
    type Error = DecodeError;

    #[inline]
    fn decode(&mut self, src: &mut bytes::BytesMut) -> Result<Decoded<Self::Item>, Self::Error> {
        if src.len() < self.0 {
            return Ok(Decoded::Insufficient);
        }
        let body = src.split_to(self.0).freeze();
        Ok(Decoded::Some(body))
    }
}

// TODO: support trailer
/// Decoder of http1 chunked body.
#[derive(Default)]
pub struct ChunkedBodyDecoder(Option<usize>);

impl Decoder for ChunkedBodyDecoder {
    type Item = Option<Bytes>;
    type Error = DecodeError;

    fn decode(&mut self, src: &mut bytes::BytesMut) -> Result<Decoded<Self::Item>, Self::Error> {
        loop {
            match self.0 {
                Some(len) => {
                    // Now we know how long we need
                    if src.len() < len + 2 {
                        return Ok(Decoded::Insufficient);
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
                    return Ok(Decoded::Some(data));
                }
                None => {
                    // We don't know what size the next block is.
                    if src.len() < 3 {
                        // There must be at least 3 bytes("0\r\n").
                        return Ok(Decoded::Insufficient);
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
                        return Ok(Decoded::Insufficient);
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

pub trait ItemWrapper<I, R> {
    type Output;
    fn wrap_none(input: I) -> Self::Output;
    fn wrap_fixed(input: I, length: usize) -> Self::Output;
    fn wrap_stream(input: I) -> Self::Output;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct ChannelWrapper;

#[derive(Debug, Clone, Copy, Default)]
pub struct DirectWrapper;

impl<H, R> ItemWrapper<H, R> for ChannelWrapper
where
    R: FromParts<H, Payload>,
{
    type Output = (R, NextDecoder<FixedBodyDecoder, ChunkedBodyDecoder, Bytes>);

    #[inline]
    fn wrap_none(header: H) -> Self::Output {
        let request = R::from_parts(header, Payload::None);
        (request, NextDecoder::None)
    }

    #[inline]
    fn wrap_fixed(header: H, length: usize) -> Self::Output {
        let (payload, sender) = fixed_payload_pair();
        let request = R::from_parts(header, Payload::from(payload));
        (
            request,
            NextDecoder::Fixed(FixedBodyDecoder(length), sender),
        )
    }

    #[inline]
    fn wrap_stream(header: H) -> Self::Output {
        let (payload, sender) = stream_payload_pair();
        let request = R::from_parts(header, Payload::from(payload));
        (
            request,
            NextDecoder::Streamed(ChunkedBodyDecoder::default(), sender),
        )
    }
}

impl<H, R> ItemWrapper<H, R> for DirectWrapper {
    type Output = (H, PayloadDecoder<FixedBodyDecoder, ChunkedBodyDecoder>);

    #[inline]
    fn wrap_none(header: H) -> Self::Output {
        (header, PayloadDecoder::None)
    }

    #[inline]
    fn wrap_fixed(header: H, length: usize) -> Self::Output {
        (header, PayloadDecoder::Fixed(FixedBodyDecoder(length)))
    }

    #[inline]
    fn wrap_stream(header: H) -> Self::Output {
        (header, PayloadDecoder::Streamed(ChunkedBodyDecoder(None)))
    }
}

/// A wrapper around D(normally RequestHeaderDecoder and ResponseHeaderDecoder).
/// Mainly for extract special headers and return the raw item and payload to
/// satisfy the constraint of `ComposeDecoder`.
pub struct GenericHeadDecoder<R, D, F> {
    decoder: D,
    _marker_f: PhantomData<F>,
    _marker_r: PhantomData<R>,
}

impl<R, D, F> GenericHeadDecoder<R, D, F> {
    pub fn new(decoder: D) -> Self {
        Self {
            decoder,
            _marker_f: PhantomData,
            _marker_r: PhantomData,
        }
    }
}

impl<R, D: Default, F> Default for GenericHeadDecoder<R, D, F> {
    fn default() -> Self {
        Self::new(D::default())
    }
}

impl<R, D, F> Decoder for GenericHeadDecoder<R, D, F>
where
    D: Decoder<Error = DecodeError>,
    D::Item: BorrowHeaderMap,
    F: ItemWrapper<D::Item, R>,
{
    type Item = F::Output;
    type Error = HttpError;

    #[inline]
    fn decode(&mut self, src: &mut bytes::BytesMut) -> Result<Decoded<Self::Item>, Self::Error> {
        match self.decoder.decode(src) {
            // TODO:
            // 1. iter single pass to find out content length and if is chunked
            // 2. validate headers to make sure content length can not be set with chunked encoding
            Ok(Decoded::Some(head)) => {
                if let Some(x) = head.header_map().get(http::header::TRANSFER_ENCODING) {
                    // Check chunked
                    if x.as_bytes().eq_ignore_ascii_case(b"chunked") {
                        return Ok(Decoded::Some(F::wrap_stream(head)));
                    }
                    // Check not identity
                    if !x.as_bytes().eq_ignore_ascii_case(b"identity") {
                        // The transfer-encoding is illegal!
                        return Err(DecodeError::Header.into());
                    }
                }

                // Now transfer-encoding is identity.
                if let Some(content_length) = head.header_map().get(http::header::CONTENT_LENGTH) {
                    let content_length = match content_length.to_str() {
                        Ok(c) if c.starts_with('+') => return Err(DecodeError::Header.into()),
                        Ok(c) => c,
                        Err(_) => return Err(DecodeError::Header.into()),
                    };
                    let content_length = match content_length.parse::<usize>() {
                        Ok(c) => c,
                        Err(_) => return Err(DecodeError::Header.into()),
                    };
                    if content_length == 0 {
                        return Ok(Decoded::Some(F::wrap_none(head)));
                    } else {
                        return Ok(Decoded::Some(F::wrap_fixed(head, content_length)));
                    }
                }
                Ok(Decoded::Some(F::wrap_none(head)))
            }
            Ok(Decoded::Insufficient) => Ok(Decoded::Insufficient),
            Ok(Decoded::InsufficientAtLeast(l)) => Ok(Decoded::InsufficientAtLeast(l)),
            Err(e) => Err(e.into()),
        }
    }
}

pub struct GenericDecoder<IO, HD> {
    framed: FramedRead<IO, HD>,
    next_decoder: NextDecoder<FixedBodyDecoder, ChunkedBodyDecoder, Bytes>,
    timeout: Option<Duration>,
}

impl<IO, HD: Default> GenericDecoder<IO, HD> {
    pub fn new(io: IO) -> Self {
        Self {
            framed: FramedRead::new(io, HD::default()),
            next_decoder: NextDecoder::default(),
            timeout: None,
        }
    }

    pub fn new_with_timeout(io: IO, timeout: Duration) -> Self {
        Self {
            framed: FramedRead::new(io, HD::default()),
            next_decoder: NextDecoder::default(),
            timeout: Some(timeout),
        }
    }
}

impl<IO, HD> GenericDecoder<IO, HD> {
    #[inline]
    pub fn set_timeout(&mut self, timeout: Option<Duration>) {
        self.timeout = timeout;
    }
}

pub trait FillPayload {
    type Error;

    fn fill_payload(&mut self) -> impl Future<Output = Result<(), Self::Error>>;
}

impl<IO, HD, I> FillPayload for GenericDecoder<IO, HD>
where
    IO: AsyncReadRent,
    HD: Decoder<
        Item = (I, NextDecoder<FixedBodyDecoder, ChunkedBodyDecoder, Bytes>),
        Error = HttpError,
    >,
{
    type Error = HttpError;

    async fn fill_payload(&mut self) -> Result<(), Self::Error> {
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
                            sender.feed(Err((PayloadError::UnexpectedEof).into()));
                            return Err(DecodeError::UnexpectedEof.into());
                        }
                        Some(Ok(item)) => {
                            sender.feed(Ok(item));
                        }
                        Some(Err(e)) => {
                            sender.feed(Err(PayloadError::Decode.into()));
                            return Err(e.into());
                        }
                    }
                }
                NextDecoder::Streamed(decoder, sender) => {
                    match self.framed.next_with(decoder).await {
                        // EOF
                        None => {
                            sender.feed_error(PayloadError::UnexpectedEof.into());
                            return Err(DecodeError::UnexpectedEof.into());
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
                            sender.feed_error(PayloadError::Decode.into());
                            return Err(e.into());
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
        Error = HttpError,
    >,
{
    type Item = Result<I, HttpError>;

    async fn next(&mut self) -> Option<Self::Item> {
        if !matches!(self.next_decoder, NextDecoder::None) {
            if let Err(e) = self.fill_payload().await {
                return Some(Err(e));
            }
        }

        if let Some(duration) = self.timeout {
            match monoio::time::timeout(duration, self.framed.peek_data()).await {
                Err(_) => {
                    return Some(Err(DecodeError::TimedOut.into()));
                }
                Ok(Err(e)) => {
                    return Some(Err(e.into()));
                }
                Ok(Ok(_)) => {}
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

pub struct IoOwnedDecoder<IO, HD> {
    framed: FramedRead<IO, HD>,
    timeout: Option<Duration>,
}

impl<IO, HD> BorrowFramedRead for IoOwnedDecoder<IO, HD> {
    type IO = IO;
    type Codec = HD;

    #[inline]
    fn framed_mut(&mut self) -> &mut FramedRead<Self::IO, Self::Codec> {
        &mut self.framed
    }
}

impl<IO, HD: Default> IoOwnedDecoder<IO, HD> {
    #[inline]
    pub fn new(io: IO) -> Self {
        Self {
            framed: FramedRead::new(io, HD::default()),
            timeout: None,
        }
    }

    #[inline]
    pub fn new_with_timeout(io: IO, timeout: Duration) -> Self {
        Self {
            framed: FramedRead::new(io, HD::default()),
            timeout: Some(timeout),
        }
    }
}

impl<IO, HD> IoOwnedDecoder<IO, HD> {
    #[inline]
    pub fn set_timeout(&mut self, timeout: Option<Duration>) {
        self.timeout = timeout;
    }
}

impl<IO, HD> Stream for IoOwnedDecoder<IO, HD>
where
    IO: AsyncReadRent,
    HD: Decoder<
        Item = (
            ResponseHead,
            PayloadDecoder<FixedBodyDecoder, ChunkedBodyDecoder>,
        ),
        Error = HttpError,
    >,
{
    type Item =
        Result<http::Response<PayloadDecoder<FixedBodyDecoder, ChunkedBodyDecoder>>, HttpError>;

    async fn next(&mut self) -> Option<Self::Item> {
        if let Some(duration) = self.timeout {
            match monoio::time::timeout(duration, self.framed.peek_data()).await {
                Err(_) => {
                    return Some(Err(DecodeError::TimedOut.into()));
                }
                Ok(Err(e)) => {
                    return Some(Err(e.into()));
                }
                Ok(Ok(_)) => {}
            }
        }

        match self.framed.next().await? {
            Err(e) => Some(Err(e)),
            Ok((header, decoder)) => Some(Ok(http::Response::from_parts(header, decoder))),
        }
    }
}

impl PayloadDecoder<FixedBodyDecoder, ChunkedBodyDecoder> {
    #[inline]
    pub fn with_io<IO>(self, next_with: IO) -> FramedPayload<IO> {
        FramedPayload::new(next_with, self)
    }
}

pub type RequestDecoder<IO> =
    GenericDecoder<IO, GenericHeadDecoder<Request, RequestHeadDecoder, ChannelWrapper>>;
pub type ResponseDecoder<IO> =
    GenericDecoder<IO, GenericHeadDecoder<Response, ResponseHeadDecoder, ChannelWrapper>>;

pub type DirectHeadDecoder = GenericHeadDecoder<Response, ResponseHeadDecoder, DirectWrapper>;
pub type ClientResponseDecoder<IO> = IoOwnedDecoder<IO, DirectHeadDecoder>;
pub type ClientResponse<IO> =
    http::Response<FramedPayload<FramedRead<OwnedReadHalf<IO>, DirectHeadDecoder>>>;

#[cfg(test)]
mod tests {
    use std::{collections::VecDeque, time::Instant};

    use bytes::BytesMut;
    use monoio::{buf::IoVecWrapperMut, BufResult};

    use super::*;

    #[test]
    fn decode_request_header_multiple_times() {
        let current = Instant::now();
        for _ in 1..10000 {
            let mut data = BytesMut::from("GET /ping HTTP/1.1\r\n\r\n");
            let _ = RequestHeadDecoder.decode(&mut data).unwrap();
        }
        let elapse = current.elapsed().as_millis();

        println!("total time spend: {:?}", elapse);
    }

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
    fn decode_response_header_multiple_times() {
        let current = Instant::now();
        for _ in 1..10000 {
            let mut data =
                BytesMut::from("HTTP/1.1 200 OK\r\nContent-Type:application/json\r\n\r\n");
            let _ = ResponseHeadDecoder.decode(&mut data).unwrap();
        }
        let elapse = current.elapsed().as_millis();

        println!("total time spend: {:?}", elapse);
    }

    #[test]
    fn decode_response_header() {
        let mut data = BytesMut::from("HTTP/1.1 200 OK\r\nContent-Type:application/json\r\n\r\n");
        let head = ResponseHeadDecoder.decode(&mut data).unwrap().unwrap();
        assert_eq!(head.status, StatusCode::OK);
        assert_eq!(head.version, Version::HTTP_11);
        assert_eq!(
            head.headers.get(http::header::CONTENT_TYPE),
            Some(&HeaderValue::from_static("application/json"))
        );
    }

    #[test]
    fn decode_fixed_body_multiple_times() {
        let current = Instant::now();
        for _ in 1..10000 {
            let mut data = BytesMut::from("balabalabalabala");
            let mut decoder = FixedBodyDecoder(8);
            let _ = decoder.decode(&mut data).unwrap();
        }

        let elapse = current.elapsed().as_millis();

        println!("total time spend: {:?}", elapse);
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
            decoder.decode(&mut data).unwrap().unwrap().unwrap(),
            "0000000000"
        );
        assert_eq!(decoder.decode(&mut data).unwrap().unwrap().unwrap(), "x");
        assert!(decoder.decode(&mut data).unwrap().unwrap().is_none());
    }

    #[test]
    fn decode_too_big_chunked_body() {
        let mut data = BytesMut::from("a\r\n0000000000\r\ndeadbeefcafebabe0\r\nx\r\n0\r\n\r\n");
        let mut decoder = ChunkedBodyDecoder::default();
        assert_eq!(
            decoder.decode(&mut data).unwrap().unwrap().unwrap(),
            "0000000000"
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

    #[monoio::test]
    async fn decode_request_without_body() {
        let io = mock! { Ok(b"GET /test HTTP/1.1\r\n\r\n".to_vec()) };
        let mut decoder = RequestDecoder::new(io);
        let req = decoder.next().await.unwrap().unwrap();
        assert_eq!(req.method(), Method::GET);
        assert!(matches!(req.body(), Payload::None));
    }

    #[monoio::test]
    async fn decode_response_without_body() {
        let io = mock! { Ok(b"HTTP/1.1 200 OK\r\n\r\n".to_vec()) };
        let mut decoder = ResponseDecoder::new(io);
        let req = decoder.next().await.unwrap().unwrap();
        assert_eq!(req.status(), StatusCode::OK);
        assert!(matches!(req.body(), Payload::None));
    }

    #[monoio::test]
    async fn decode_fixed_body_request() {
        let io = mock! { Ok(b"POST /test HTTP/1.1\r\nContent-Length: 4\r\ntest-key: test-val\r\n\r\nbody".to_vec()) };
        let mut decoder = RequestDecoder::new(io);
        let req = decoder.next().await.unwrap().unwrap();
        assert_eq!(req.method(), Method::POST);
        assert_eq!(req.headers().get("test-key").unwrap(), "test-val");
        let mut payload = match req.into_body() {
            Payload::Fixed(p) => p,
            _ => panic!("wrong payload type"),
        };
        assert!(decoder.fill_payload().await.is_ok());
        let data = payload.next().await.unwrap().unwrap();
        assert_eq!(&data, &"body");
        assert!(decoder.next().await.is_none());
    }

    #[monoio::test]
    async fn decode_fixed_body_response() {
        let io = mock! { Ok(b"HTTP/1.1 200 OK\r\ncontent-lenGth: 4\r\ntest-key: test-val\r\n\r\nbody".to_vec()) };
        let mut decoder = ResponseDecoder::new(io);
        let req = decoder.next().await.unwrap().unwrap();
        assert_eq!(req.status(), StatusCode::OK);
        assert_eq!(req.headers().get("test-key").unwrap(), "test-val");
        let mut payload = match req.into_body() {
            Payload::Fixed(p) => p,
            _ => panic!("wrong payload type"),
        };
        assert!(decoder.fill_payload().await.is_ok());
        let data = payload.next().await.unwrap().unwrap();
        assert_eq!(&data, &"body");
        assert!(decoder.next().await.is_none());
    }

    #[monoio::test]
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

    #[monoio::test]
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
        async fn read<T: monoio::buf::IoBufMut>(&mut self, mut buf: T) -> BufResult<usize, T> {
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

        async fn readv<T: monoio::buf::IoVecBufMut>(&mut self, mut buf: T) -> BufResult<usize, T> {
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
