use std::collections::HashMap;

use cookie::{Cookie, CookieJar};
pub use http::response::{Builder as ResponseBuilder, Parts as ResponseHead};

use super::{
    error::{ExtractError, HttpError},
    response::Response,
};
use crate::{common::IntoParts, impl_cookie_extractor};

#[derive(Clone)]
#[allow(dead_code)]
pub struct ParsedResponse<P> {
    inner: http::Response<P>,
    cookie_jar: Option<CookieJar>,
    url_params: Option<HashMap<String, String>>,
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
    fn into_parts(self) -> (Self::Parts, Self::Body) {
        self.inner.into_parts()
    }
}

impl_cookie_extractor!(ParsedResponse, Response, "SET_COOKIE");
