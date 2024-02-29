use std::hint::unreachable_unchecked;

use cookie::{Cookie, CookieJar};
use http::header::{HeaderMap, HeaderValue};
pub use http::response::{Builder as ResponseBuilder, Parts as ResponseHead};

use super::{
    error::{ExtractError, HttpError},
    response::Response,
    Parse, QueryMap,
};
use crate::{common::IntoParts, impl_cookie_extractor};

#[derive(Clone)]
#[allow(dead_code)]
pub struct ParsedResponse<P> {
    inner: http::Response<P>,
    cookie_jar: Parse<CookieJar>,
    url_params: Parse<QueryMap>,
}

impl<P> ParsedResponse<P> {
    pub fn into_http_request(mut self) -> Result<Response<P>, HttpError> {
        self.serialize_cookies_into_header()?;
        Ok(self.inner)
    }
}

impl<P> std::ops::Deref for ParsedResponse<P> {
    type Target = http::response::Response<P>;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl<P> std::ops::DerefMut for ParsedResponse<P> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}

impl<P> IntoParts for ParsedResponse<P> {
    type Parts = ResponseHead;
    type Body = P;
    fn into_parts(mut self) -> (Self::Parts, Self::Body) {
        let _ = self.serialize_cookies_into_header();
        self.inner.into_parts()
    }
}

impl_cookie_extractor!(ParsedResponse, "SET_COOKIE");
