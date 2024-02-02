pub use http::request::{Builder as RequestBuilder, Parts as RequestHead};

use super::BorrowHeaderMap;
use crate::{
    common::{FromParts, IntoParts},
    h1::payload::Payload,
};

pub type Request<P = Payload> = http::request::Request<P>;

impl<P> FromParts<RequestHead, P> for Request<P> {
    fn from_parts(parts: RequestHead, body: P) -> Self {
        Self::from_parts(parts, body)
    }
}

impl<P> IntoParts for Request<P> {
    type Parts = RequestHead;
    type Body = P;
    fn into_parts(self) -> (Self::Parts, Self::Body) {
        self.into_parts()
    }
}

pub struct RequestForEncoder<'a, P = Payload> {
    parts: &'a mut RequestHead,
    body: P,
}

impl<'a, P> FromParts<&'a mut RequestHead, P> for RequestForEncoder<'a, P> {
    fn from_parts(parts: &'a mut RequestHead, body: P) -> Self {
        Self { parts, body }
    }
}

impl<'a, P> IntoParts for RequestForEncoder<'a, P> {
    type Parts = &'a mut RequestHead;
    type Body = P;
    fn into_parts(self) -> (Self::Parts, Self::Body) {
        (self.parts, self.body)
    }
}

impl BorrowHeaderMap for RequestHead {
    fn header_map(&self) -> &http::HeaderMap {
        &self.headers
    }

    fn header_map_mut(&mut self) -> &mut http::HeaderMap {
        &mut self.headers
    }
}

impl BorrowHeaderMap for &mut RequestHead {
    fn header_map(&self) -> &http::HeaderMap {
        &self.headers
    }

    fn header_map_mut(&mut self) -> &mut http::HeaderMap {
        &mut self.headers
    }
}
