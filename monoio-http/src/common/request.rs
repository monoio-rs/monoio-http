pub use http::request::{Builder as RequestBuilder, Parts as RequestHead};

use super::BorrowHeaderMap;
use crate::{
    common::{FromParts, IntoParts},
    h1::payload::Payload,
};

pub type Request<P = Payload> = http::request::Request<P>;

impl<P> FromParts<RequestHead, P> for Request<P> {
    #[inline]
    fn from_parts(parts: RequestHead, body: P) -> Self {
        Self::from_parts(parts, body)
    }
}

impl<P> IntoParts for Request<P> {
    type Parts = RequestHead;
    type Body = P;
    #[inline]
    fn into_parts(self) -> (Self::Parts, Self::Body) {
        self.into_parts()
    }
}

pub struct RequestHeadRef<'a> {
    pub method: &'a http::Method,
    pub uri: &'a http::Uri,
    pub version: http::Version,
    pub headers: &'a http::HeaderMap,
}

impl<'a> RequestHeadRef<'a> {
    #[inline]
    pub fn from_http<B>(req: &'a http::Request<B>) -> Self {
        Self {
            method: req.method(),
            uri: req.uri(),
            version: req.version(),
            headers: req.headers(),
        }
    }
}

pub struct RequestRef<'a, B = Payload> {
    pub parts: RequestHeadRef<'a>,
    pub body: B,
}

impl<'a, B> FromParts<RequestHeadRef<'a>, B> for RequestRef<'a, B> {
    #[inline]
    fn from_parts(parts: RequestHeadRef<'a>, body: B) -> Self {
        Self { parts, body }
    }
}

impl<'a, B> IntoParts for RequestRef<'a, B> {
    type Parts = RequestHeadRef<'a>;
    type Body = B;
    #[inline]
    fn into_parts(self) -> (Self::Parts, Self::Body) {
        (self.parts, self.body)
    }
}

impl BorrowHeaderMap for RequestHead {
    #[inline]
    fn header_map(&self) -> &http::HeaderMap {
        &self.headers
    }
}
