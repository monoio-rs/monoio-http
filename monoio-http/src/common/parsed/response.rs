use std::cell::UnsafeCell;

use cookie::{Cookie, CookieJar};
use http::header::COOKIE;
pub use http::response::{Builder as ResponseBuilder, Parts as ResponseHead};

use crate::common::{error::ParseError, parsed::Parse, response::Response, IntoParts};

pub struct ParsedResponse<P> {
    inner: Response<P>,
    cookie_jar: UnsafeCell<Parse<CookieJar>>,
}

impl<P> From<Response<P>> for ParsedResponse<P> {
    #[inline]
    fn from(inner: Response<P>) -> Self {
        Self {
            inner,
            cookie_jar: UnsafeCell::new(Parse::Unparsed),
        }
    }
}

impl<P> ParsedResponse<P> {
    #[inline]
    pub const fn new(inner: Response<P>) -> Self {
        Self {
            inner,
            cookie_jar: UnsafeCell::new(Parse::Unparsed),
        }
    }

    #[inline]
    pub fn into_http_response(mut self) -> Response<P> {
        self.serialize_cookies_into_header();
        self.inner
    }

    #[inline]
    pub fn writeback(&mut self) {
        self.serialize_cookies_into_header();
    }

    #[inline]
    pub fn raw_response(&self) -> &Response<P> {
        &self.inner
    }

    #[inline]
    pub fn raw_response_mut(&mut self) -> &mut Response<P> {
        &mut self.inner
    }
}

impl<P> std::ops::Deref for ParsedResponse<P> {
    type Target = Response<P>;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl<P> std::ops::DerefMut for ParsedResponse<P> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}

impl<P> From<ParsedResponse<P>> for Response<P> {
    #[inline]
    fn from(parsed: ParsedResponse<P>) -> Self {
        parsed.into_http_response()
    }
}

impl<P> IntoParts for ParsedResponse<P> {
    type Parts = ResponseHead;
    type Body = P;
    fn into_parts(mut self) -> (Self::Parts, Self::Body) {
        self.serialize_cookies_into_header();
        self.inner.into_parts()
    }
}

impl<P> ParsedResponse<P> {
    /// Parse the cookies into a CookieJar.
    pub fn parse_cookies_params(&self) -> Result<(), ParseError> {
        let cookie_jar = unsafe { &mut *self.cookie_jar.get() };
        if cookie_jar.is_parsed() {
            return Ok(());
        }
        if cookie_jar.is_failed() {
            return Err(ParseError::Previous);
        }

        let mut jar = CookieJar::new();
        if let Some(cookie_header) = self.inner.headers().get(COOKIE) {
            let cookie_str = cookie_header.to_str().map_err(|_| {
                *cookie_jar = Parse::Failed;
                ParseError::InvalidHeaderValue
            })?;
            // TODO: maybe we should use split_parse_encoded?
            for cookie in Cookie::split_parse(cookie_str) {
                let cookie = match cookie {
                    Ok(c) => c,
                    Err(_) => {
                        *cookie_jar = Parse::Failed;
                        return Err(ParseError::InvalidHeaderValue);
                    }
                };
                jar.add_original(cookie.into_owned());
            }
        }
        // Allow empty cookie.
        *cookie_jar = Parse::Parsed(jar);
        Ok(())
    }

    /// Set a cookie into the CookieJar.
    /// Note: if the cookies are not parsed before, this method will parse the cookies first.
    #[inline]
    pub fn set_cookie_param(&self, cookie: Cookie<'static>) -> Result<(), ParseError> {
        self.parse_cookies_params()?;
        unsafe {
            let parsed = &mut *self.cookie_jar.get();
            parsed.as_mut().unwrap_unchecked().add_original(cookie);
            Ok(())
        }
    }

    /// Get a cookie from the CookieJar.
    /// Note: if the cookies are not parsed before, this method will parse the cookies first.
    #[inline]
    pub fn get_cookie_param(&self, name: &str) -> Result<Option<&Cookie<'static>>, ParseError> {
        self.parse_cookies_params()?;
        unsafe {
            let parsed = &*self.cookie_jar.get();
            Ok(parsed.as_ref().unwrap_unchecked().get(name).to_owned())
        }
    }

    fn serialize_cookies_into_header(&mut self) {
        let jar = unsafe { &*self.cookie_jar.get() };
        if let Parse::Parsed(ref jar) = jar {
            let cookies = jar
                .iter()
                .map(|cookie| cookie.encoded().to_string())
                .collect::<Vec<String>>()
                .join("; ");
            let cookie_header = http::header::HeaderValue::from_str(&cookies).unwrap();
            self.inner.headers_mut().insert(COOKIE, cookie_header);
        }
    }
}
