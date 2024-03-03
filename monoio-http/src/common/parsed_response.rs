use std::cell::{Ref, RefCell, UnsafeCell};

use cookie::{Cookie, CookieJar};
pub use http::response::{Builder as ResponseBuilder, Parts as ResponseHead};

use super::{
    error::{ExtractError, HttpError},
    response::Response,
    Parse,
};
use crate::common::IntoParts;

pub struct ParsedResponse<P> {
    inner: UnsafeCell<Response<P>>,
    cookie_jar: RefCell<Parse<CookieJar>>,
}

impl<P> From<Response<P>> for ParsedResponse<P> {
    #[inline]
    fn from(value: Response<P>) -> Self {
        Self {
            inner: UnsafeCell::new(value),
            cookie_jar: RefCell::new(Parse::Unparsed),
        }
    }
}

impl<P> ParsedResponse<P> {
    pub fn into_http_response(self) -> Response<P> {
        self.serialize_cookies_into_header();
        self.inner.into_inner()
    }

    pub fn into_http_request_ref(&self) -> &Response<P> {
        self.serialize_cookies_into_header();
        self.inner()
    }

    fn inner(&self) -> &Response<P> {
        unsafe { &*self.inner.get() }
    }
}

impl<P> std::ops::Deref for ParsedResponse<P> {
    type Target = http::response::Response<P>;

    fn deref(&self) -> &Self::Target {
        self.inner()
    }
}

impl<P> std::ops::DerefMut for ParsedResponse<P> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.inner.get_mut()
    }
}

impl<P> IntoParts for ParsedResponse<P> {
    type Parts = ResponseHead;
    type Body = P;
    fn into_parts(self) -> (Self::Parts, Self::Body) {
        self.serialize_cookies_into_header();
        self.inner.into_inner().into_parts()
    }
}

impl<P> ParsedResponse<P> {
    pub fn parse_cookies_params(&self) -> Result<Ref<CookieJar>, HttpError> {
        if self.cookie_jar.borrow().is_unparsed() {
            let mut jar = CookieJar::new();
            if let Some(cookie_header) = self.inner().headers().get("SET_COOKIE") {
                let cookie_str = cookie_header
                    .to_str()
                    .map_err(|_| ExtractError::InvalidHeaderValue)?;
                let cookie_owned_str = cookie_str.to_string();
                for cookie in Cookie::split_parse(cookie_owned_str) {
                    let cookie = cookie.unwrap();
                    jar.add_original(cookie);
                }
            }
            *self.cookie_jar.borrow_mut() = Parse::Parsed(jar);
        }

        Ok(Ref::map(self.cookie_jar.borrow(), |params| {
            params.parsed_inner()
        }))
    }

    pub fn set_cookie_param(&self, cookie: &Cookie<'static>) -> Result<(), HttpError> {
        if self.cookie_jar.borrow().is_unparsed() {
            self.parse_cookies_params()?;
        }

        let mut jar = self.cookie_jar.borrow_mut();
        jar.parsed_inner_mut().add(cookie.clone());
        Ok(())
    }

    pub fn get_cookie_param(&self, name: &str) -> Option<Cookie> {
        self.parse_cookies_params()
            .ok()
            .and_then(|jar| jar.get(name).cloned())
    }

    pub fn get_cookie_value(&self, name: &str) -> Option<String> {
        self.get_cookie_param(name).map(|c| c.value().to_string())
    }

    fn serialize_cookies_into_header(&self) {
        if self.cookie_jar.borrow().is_parsed() {
            let jar = self.cookie_jar.borrow();
            let jar = jar.parsed_inner();

            let cookies = jar
                .iter()
                .map(|cookie| cookie.encoded().to_string())
                .collect::<Vec<String>>()
                .join("; ");
            let cookie_header = http::header::HeaderValue::from_str(&cookies).unwrap();
            let req = unsafe { &mut *self.inner.get() };
            req.headers_mut().insert("COOKIE", cookie_header);
        }
    }
}
