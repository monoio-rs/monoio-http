use std::cell::{Ref, RefCell, UnsafeCell};

use bytes::Bytes;
use cookie::{Cookie, CookieJar};
pub use http::request::{Builder as RequestBuilder, Parts as RequestHead};

// use multipart::server::nickel::nickel::hyper::header::Origin;
// use multipart::server::Multipart;
use super::body::HttpBodyStream;
use super::{
    body::{Body, BodyExt, FixedBody},
    error::{ExtractError, HttpError},
    request::Request,
    Parse, QueryMap,
};
use crate::common::IntoParts;

pub struct ParsedRequest<P> {
    inner: UnsafeCell<Request<P>>,
    cookie_jar: RefCell<Parse<CookieJar>>,
    url_params: RefCell<Parse<QueryMap>>,
    body_url_params: RefCell<Parse<QueryMap>>,
}

impl<P> From<Request<P>> for ParsedRequest<P> {
    #[inline]
    fn from(value: Request<P>) -> Self {
        Self {
            inner: UnsafeCell::new(value),
            cookie_jar: RefCell::new(Parse::Unparsed),
            url_params: RefCell::new(Parse::Unparsed),
            body_url_params: RefCell::new(Parse::Unparsed),
        }
    }
}

impl<P> ParsedRequest<P> {
    pub fn new(req: Request<P>) -> Self {
        Self {
            inner: UnsafeCell::new(req),
            cookie_jar: RefCell::new(Parse::Unparsed),
            url_params: RefCell::new(Parse::Unparsed),
            body_url_params: RefCell::new(Parse::Unparsed),
        }
    }

    pub fn into_http_request(self) -> Request<P> {
        self.serialize_cookies_into_header();
        self.inner.into_inner()
    }

    pub fn inner_ref(&self) -> &Request<P> {
        self.serialize_cookies_into_header();
        self.inner()
    }

    fn inner(&self) -> &Request<P> {
        unsafe { &*self.inner.get() }
    }
}

impl<P> ParsedRequest<P> {
    pub fn get_url_params(&self) -> Ref<'_, Parse<QueryMap>> {
        self.url_params.borrow()
    }
}

impl<P> std::ops::Deref for ParsedRequest<P> {
    type Target = http::request::Request<P>;

    fn deref(&self) -> &Self::Target {
        self.inner()
    }
}

impl<P> std::ops::DerefMut for ParsedRequest<P> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.inner.get_mut()
    }
}

impl<P> IntoParts for ParsedRequest<P> {
    type Parts = RequestHead;
    type Body = P;
    fn into_parts(self) -> (Self::Parts, Self::Body) {
        self.serialize_cookies_into_header();
        self.inner.into_inner().into_parts()
    }
}

// URI parsing methods
impl<P> ParsedRequest<P> {
    pub fn parse_url_params(&self) -> Result<Ref<QueryMap>, HttpError> {
        if self.url_params.borrow().is_unparsed() {
            let uri = self.inner().uri();
            match uri.query() {
                Some(query) => {
                    let parsed = serde_urlencoded::from_str(query).map_err(|e| {
                        *self.url_params.borrow_mut() = Parse::Failed;
                        e
                    })?;
                    *self.url_params.borrow_mut() = Parse::Parsed(parsed);
                    Ok(Ref::map(self.url_params.borrow(), |params| {
                        params.parsed_inner()
                    }))
                }
                None => {
                    *self.url_params.borrow_mut() = Parse::Failed;
                    Err(ExtractError::MissingURL.into())
                }
            }
        } else {
            Ok(Ref::map(self.url_params.borrow(), |params| {
                params.parsed_inner()
            }))
        }
    }

    pub fn get_url_param(&self, name: &str) -> Option<&str> {
        self.parse_url_params()
            .ok()
            .and_then(|map| Ref::leak(map).get(name).map(|s| s.as_str()))
    }
}

impl<P> ParsedRequest<P> {
    pub fn parse_cookies_params(&self) -> Result<Ref<CookieJar>, HttpError> {
        if self.cookie_jar.borrow().is_unparsed() {
            let mut jar = CookieJar::new();
            if let Some(cookie_header) = self.inner().headers().get("COOKIE") {
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

// Request specific, multipart and url encoded body parsing methods
impl<P> ParsedRequest<P>
where
    P: Body<Data = Bytes, Error = HttpError> + FixedBody + Sized,
{
    /// Deserializes"x-www-form-urlencoded" body into a QueryMap.
    pub async fn parse_body_url_encoded_params(&self) -> Result<Ref<QueryMap>, HttpError> {
        if self.url_params.borrow().is_unparsed() {
            match self.inner().headers().get("Content-Type") {
                Some(content_type) => {
                    let content_type_str = content_type
                        .to_str()
                        .ok()
                        .and_then(|s| s.split(';').next())
                        .unwrap_or_default();
                    if content_type_str == "application/x-www-form-urlencoded" {
                        let temp_req = Request::new(P::fixed_body(None));
                        let orig_req = unsafe {
                            let orig_req = self.inner.get();
                            let orig_copy = std::ptr::read(orig_req);
                            std::ptr::write(orig_req, temp_req);
                            orig_copy
                        };

                        let (orig_parts, orig_body) = orig_req.into_parts();
                        let data = orig_body.bytes().await?;

                        let mut params = QueryMap::new();
                        let params = serde_urlencoded::from_bytes::<QueryMap>(&data).map(|p| {
                            params.extend(p);
                            params
                        })?;

                        let rebuilt_req =
                            Request::from_parts(orig_parts, P::fixed_body(Some(data)));

                        unsafe {
                            let req = self.inner.get();
                            std::ptr::write(req, rebuilt_req);
                        }

                        *self.body_url_params.borrow_mut() = Parse::Parsed(params);
                        Ok(Ref::map(self.body_url_params.borrow(), |params| {
                            params.parsed_inner()
                        }))
                    } else {
                        Err(ExtractError::InvalidContentType.into())
                    }
                }
                None => Err(ExtractError::InvalidHeaderValue.into()),
            }
        } else {
            Ok(Ref::map(self.body_url_params.borrow(), |params| {
                params.parsed_inner()
            }))
        }
    }

    pub fn get_body_url_param(&self, name: &str) -> Option<String> {
        if self.body_url_params.borrow().is_unparsed_or_failed() {
            None
        } else {
            self.body_url_params
                .borrow()
                .parsed_inner()
                .get(name)
                .map(|s| s.to_string())
        }
    }
}

impl<P> ParsedRequest<P>
where
    P: Into<HttpBodyStream> + 'static,
{
    /// Async multipart parsing. This method doesn't wait to stream the entire body before
    /// attempting to parse it, suitable for large bodies. It returns a Multipart struct that
    /// can be used to stream the parts of the body. It also provides constraints to limit the
    /// size of the parts and the entire body.
    /// See https://github.com/rousan/multer-rs/blob/master/examples/prevent_dos_attack.rs and
    /// test_request_multi_part_parse_async.
    pub fn parse_multipart_async<'a>(
        self,
        user_constraints: Option<multer::Constraints>,
    ) -> Result<multer::Multipart<'a>, HttpError> {
        if let Some(content_type) = self.inner().headers().get("Content-Type") {
            let content_type_str = content_type
                .to_str()
                .ok()
                .and_then(|s| s.split(';').next())
                .unwrap_or_default();

            if content_type_str == "multipart/form-data" {
                // Parse the multipart request
                let boundary = content_type
                    .to_str()
                    .ok()
                    .and_then(|s| s.split("boundary=").nth(1))
                    .unwrap_or_default()
                    .to_string();

                let req = self.inner.into_inner();
                let (_parts, body) = req.into_parts();

                let body_stream: HttpBodyStream = body.into();

                let constraints_to_use =
                    user_constraints.unwrap_or_else(multer::Constraints::default);
                let m =
                    multer::Multipart::with_constraints(body_stream, boundary, constraints_to_use);
                Ok(m)
            } else {
                Err(ExtractError::InvalidContentType.into())
            }
        } else {
            Err(ExtractError::InvalidHeaderValue.into())
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Cursor, Read};

    use http::header::{HeaderValue, COOKIE};

    use super::*;
    use crate::common::body::HttpBody;

    fn build_request_with_cookies() -> Request<HttpBody> {
        let body = HttpBody::fixed_body(None);
        let mut request = Request::new(body);

        let user_id_cookie = "user_id=123;";
        let email_cookie = "email=some_email;";

        // Combine both cookies into a single Cookie header
        let cookie_value = format!("{}; {}", user_id_cookie, email_cookie);
        let value = HeaderValue::from_str(&cookie_value).unwrap();
        request.headers_mut().insert(COOKIE, value);

        request
    }

    fn build_request() -> Request<HttpBody> {
        let body = HttpBody::fixed_body(None);
        Request::new(body)
    }

    #[test]
    fn test_request_cookie_parse() {
        let request = build_request_with_cookies();
        let parsed_req = ParsedRequest::new(request);
        parsed_req.parse_cookies_params().unwrap();

        assert_eq!(
            parsed_req.get_cookie_value("user_id"),
            Some("123".to_string())
        );
        assert_eq!(
            parsed_req.get_cookie_value("email"),
            Some("some_email".to_string())
        );
    }

    #[test]
    fn test_request_cookie_set() {
        let request = build_request();

        let parsed_req = ParsedRequest::new(request);
        // parsed_req.parse_cookies_params().unwrap();

        parsed_req
            .set_cookie_param(&Cookie::new("user_id", "123"))
            .unwrap();
        parsed_req
            .set_cookie_param(&Cookie::new("email", "some_email"))
            .unwrap();

        // Calling into_http_request will call serialize_cookies_into_header
        // and set the Cookie header in the request.
        let new_req = parsed_req.into_http_request();
        let cookie_header = new_req.headers().get(COOKIE).unwrap();
        let cookie_str = cookie_header.to_str().unwrap();
        assert_eq!(cookie_str, "email=some_email; user_id=123");
    }

    #[test]
    fn test_request_additional_cookies_set() {
        let request = build_request_with_cookies();
        let parsed_req = ParsedRequest::new(request);
        // parsed_req.parse_cookies_params().unwrap();

        parsed_req
            .set_cookie_param(&Cookie::new("org", "ByteDance"))
            .unwrap();
        parsed_req
            .set_cookie_param(&Cookie::new("id", "36603"))
            .unwrap();

        assert_eq!(
            parsed_req.get_cookie_value("user_id"),
            Some("123".to_string())
        );
        assert_eq!(
            parsed_req.get_cookie_value("email"),
            Some("some_email".to_string())
        );
        assert_eq!(
            parsed_req.get_cookie_value("org"),
            Some("ByteDance".to_string())
        );
        assert_eq!(parsed_req.get_cookie_value("id"), Some("36603".to_string()));
    }

    fn create_request_with_url_params() -> Request<HttpBody> {
        let body = HttpBody::fixed_body(None);
        let mut request = Request::new(body);

        let uri = http::Uri::builder()
            .scheme("http")
            .authority("example.com")
            .path_and_query("/path?user_id=123&email=some_email")
            .build()
            .unwrap();

        *request.uri_mut() = uri;

        request
    }

    #[test]
    fn test_request_url_params_parse() {
        let request = create_request_with_url_params();
        let parsed = ParsedRequest::new(request);

        assert_eq!(parsed.get_url_param("user_id"), Some("123"));
        assert_eq!(parsed.get_url_param("email"), Some("some_email"));

        assert_eq!(parsed.get_url_param("user_id"), Some("123"));
        assert_eq!(parsed.get_url_param("email"), Some("some_email"));
    }

    fn create_request_with_url_encoded_body() -> Request<HttpBody> {
        let form_data = vec![("key1", "value1"), ("key2", "value2")];

        let body = serde_urlencoded::to_string(form_data).expect("Failed to serialize form data");
        let mut request = Request::new(HttpBody::fixed_body(Some(Bytes::from(body))));
        request.headers_mut().insert(
            http::header::CONTENT_TYPE,
            HeaderValue::from_static("application/x-www-form-urlencoded"),
        );
        request
    }

    fn create_request_multi_part() -> Request<HttpBody> {
        let mut multipart = multipart::client::lazy::Multipart::new();
        multipart.add_text("field1", "value1");
        multipart.add_text("field2", "value2");

        let file_data = "Hello, World!".as_bytes();
        let file_mime = Some(mime::TEXT_PLAIN);
        multipart.add_stream(
            "file",
            Cursor::new(file_data),
            Some("HelloWorld.txt"),
            file_mime,
        );

        let mut p = multipart.prepare().unwrap();
        let boundary = p.boundary();
        let content_type = format!("multipart/form-data; boundary={}", boundary);
        let mut buf = Vec::new();
        p.read_to_end(&mut buf).unwrap();

        let body = HttpBody::fixed_body(Some(Bytes::from(buf)));
        let mut request = Request::new(body);
        request.headers_mut().insert(
            http::header::CONTENT_TYPE,
            HeaderValue::from_str(content_type.as_str()).unwrap(),
        );

        request
    }

    #[monoio::test_all]
    async fn test_request_url_encoded_body_parse() {
        let request = create_request_with_url_encoded_body();
        let parsed_request = ParsedRequest::new(request);

        let query_map = parsed_request
            .parse_body_url_encoded_params()
            .await
            .unwrap();

        assert_eq!(query_map.get("key1"), Some(&"value1".to_string()));
        assert_eq!(query_map.get("key2"), Some(&"value2".to_string()));

        let form_data = vec![("key1", "value1"), ("key2", "value2")];
        let orig_body =
            serde_urlencoded::to_string(form_data).expect("Failed to serialize form data");

        drop(query_map);

        let (_parts, body) = parsed_request.into_parts();

        assert_eq!(body.bytes().await.unwrap(), orig_body.into_bytes());
    }

    #[monoio::test_all]
    async fn test_request_url_encoded_body_parse2() {
        let request = create_request_with_url_encoded_body();
        let parsed_request = ParsedRequest::new(request);

        parsed_request.parse_body_url_encoded_params().await.unwrap();

        let res1 = parsed_request.get_body_url_param("key1");
        assert_eq!(res1, Some("value1".to_string()));
        drop(res1);
        assert_eq!(
            parsed_request.get_body_url_param("key2"),
            Some("value2".to_string())
        );
    }

    #[monoio::test_all]
    async fn test_request_multi_part_parse_async() {
        let request = create_request_multi_part();

        let parsed_request = ParsedRequest::new(request);

        let mut multipart_struct = parsed_request.parse_multipart_async(None).unwrap();

        let field = multipart_struct.next_field().await.unwrap().unwrap();
        assert_eq!(field.name().unwrap(), "field1");
        let bytes = field.bytes().await.unwrap();
        assert_eq!(bytes, Bytes::from_static(b"value1"));

        let field = multipart_struct.next_field().await.unwrap().unwrap();
        assert_eq!(field.name().unwrap(), "field2");
        let bytes = field.bytes().await.unwrap();
        assert_eq!(bytes, Bytes::from_static(b"value2"));

        let field = multipart_struct.next_field().await.unwrap().unwrap();
        assert_eq!(field.name().unwrap(), "file");
        assert_eq!(field.file_name().unwrap(), "HelloWorld.txt".to_string());
        assert_eq!(field.content_type().unwrap(), &mime::TEXT_PLAIN);
        let bytes = field.bytes().await.unwrap();
        assert_eq!(bytes, Bytes::from_static(b"Hello, World!"));
    }
}
