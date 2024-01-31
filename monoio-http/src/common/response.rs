use cookie::{Cookie, CookieJar};
pub use http::response::{Builder as ResponseBuilder, Parts as ResponseHead};

use super::{
    error::{ExtractError, HttpError},
    BorrowHeaderMap,
};
use crate::{
    common::{FromParts, IntoParts},
    h1::payload::Payload,
    impl_cookie_extractor,
};

#[derive(Clone)]
pub struct Response<P = Payload> {
    inner: http::Response<P>,
    cookie_jar: Option<CookieJar>,
}

impl<P> Response<P> {
    pub fn new(payload: P) -> Self {
        let inner = http::Response::new(payload);
        let cookie_jar = None;
        Self { inner, cookie_jar }
    }

    pub fn into_body(self) -> P {
        self.inner.into_body()
    }
}

impl<P> std::ops::Deref for Response<P> {
    type Target = http::response::Response<P>;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl<P> std::ops::DerefMut for Response<P> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}
impl<P> From<http::Response<P>> for Response<P> {
    fn from(inner: http::Response<P>) -> Self {
        Response {
            inner,
            cookie_jar: None,
        }
    }
}

impl<P> FromParts<ResponseHead, P> for Response<P> {
    fn from_parts(parts: ResponseHead, body: P) -> Self {
        let inner = http::Response::from_parts(parts, body);
        let cookie_jar = None;

        Self { inner, cookie_jar }
    }
}

impl<P> IntoParts for Response<P> {
    type Parts = ResponseHead;
    type Body = P;
    fn into_parts(self) -> (Self::Parts, Self::Body) {
        self.inner.into_parts()
    }
}

impl BorrowHeaderMap for ResponseHead {
    fn header_map(&self) -> &http::HeaderMap {
        &self.headers
    }

    fn header_map_mut(&mut self) -> &mut http::HeaderMap {
        &mut self.headers
    }
}

impl_cookie_extractor!(Response, "SET_COOKIE");
