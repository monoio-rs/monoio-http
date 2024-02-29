pub use http::response::{Builder as ResponseBuilder, Parts as ResponseHead};

use super::BorrowHeaderMap;
use crate::{
    common::{FromParts, IntoParts},
    h1::payload::Payload,
};

pub type Response<P = Payload> = http::response::Response<P>;

impl<P> FromParts<ResponseHead, P> for Response<P> {
    #[inline]
    fn from_parts(parts: ResponseHead, body: P) -> Self {
        Self::from_parts(parts, body)
    }
}

impl<P> IntoParts for Response<P> {
    type Parts = ResponseHead;
    type Body = P;
    #[inline]
    fn into_parts(self) -> (Self::Parts, Self::Body) {
        self.into_parts()
    }
}

pub struct ResponseHeadRef<'a> {
    pub status: http::StatusCode,
    pub version: http::Version,
    pub headers: &'a http::HeaderMap<http::HeaderValue>,
    pub extensions: &'a http::Extensions,
}

impl BorrowHeaderMap for ResponseHead {
    #[inline]
    fn header_map(&self) -> &http::HeaderMap {
        &self.headers
    }
}
