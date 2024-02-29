use std::{hint::unreachable_unchecked, io::Cursor};

use bytes::Bytes;
use cookie::Cookie; // Import the Cookie type from the cookie crate
use cookie::CookieJar;
use http::header::{HeaderMap, HeaderValue};
pub use http::request::{Builder as RequestBuilder, Parts as RequestHead};
use multipart::server::Multipart;

use super::{
    body::{Body, BodyExt, FixedBody},
    error::{ExtractError, HttpError},
    request::Request,
    Parse, QueryMap,
};
use crate::{common::IntoParts, impl_cookie_extractor};

#[derive(Clone)]
pub struct ParsedRequest<P> {
    inner: Request<P>,
    cookie_jar: Parse<CookieJar>,
    url_params: Parse<QueryMap>,
    body_url_params: Parse<QueryMap>,
}

impl<P> From<Request<P>> for ParsedRequest<P> {
    #[inline]
    fn from(value: Request<P>) -> Self {
        Self {
            inner: value,
            cookie_jar: Parse::Unparsed,
            url_params: Parse::Unparsed,
            body_url_params: Parse::Unparsed,
        }
    }
}

impl<P> ParsedRequest<P> {
    pub fn new(req: Request<P>) -> Self {
        Self {
            inner: req,
            cookie_jar: Parse::Unparsed,
            url_params: Parse::Unparsed,
            body_url_params: Parse::Unparsed,
        }
    }

    pub fn into_http_request(mut self) -> Result<Request<P>, HttpError> {
        self.serialize_cookies_into_header()?;
        Ok(self.inner)
    }
}

impl<P> std::ops::Deref for ParsedRequest<P> {
    type Target = http::request::Request<P>;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl<P> std::ops::DerefMut for ParsedRequest<P> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}

impl<P> IntoParts for ParsedRequest<P> {
    type Parts = RequestHead;
    type Body = P;
    fn into_parts(mut self) -> (Self::Parts, Self::Body) {
        let _ = self.serialize_cookies_into_header();
        self.inner.into_parts()
    }
}

// URI parsing methods
impl<P> ParsedRequest<P> {
    pub fn parse_url_params(&mut self) -> Result<&QueryMap, HttpError> {
        let uri = self.inner.uri();
        if let Some(query) = uri.query() {
            let parsed = serde_urlencoded::from_str(query).map_err(|e| {
                self.url_params = Parse::Failed;
                e
            })?;
            self.url_params = Parse::Parsed(parsed);
        } else {
            self.url_params = Parse::Parsed(Default::default());
        }
        Ok(match &self.url_params {
            Parse::Parsed(inner) => inner,
            _ => unsafe { unreachable_unchecked() },
        })
    }

    #[inline]
    pub fn get_url_param(&mut self, name: &str) -> Option<&String> {
        match self.url_params {
            Parse::Parsed(ref map) => map.get(name),
            Parse::Failed => None,
            Parse::Unparsed => match self.parse_url_params() {
                Ok(map) => map.get(name),
                Err(_) => None,
            },
        }
    }

    #[inline]
    pub fn url_params(&self) -> &Parse<QueryMap> {
        &self.url_params
    }

    #[inline]
    pub fn url_params_mut(&mut self) -> &mut Parse<QueryMap> {
        &mut self.url_params
    }
}

// Cookie parsing and serialization methods
impl_cookie_extractor!(ParsedRequest, "COOKIE");

// Request specific, multipart and url encoded body parsing methods
impl<P> ParsedRequest<P>
where
    P: Body<Data = Bytes, Error = HttpError> + FixedBody + Sized,
{
    /// Streams the entire body and then attempts to parse it. The body is NOT
    /// consumed as part of this call. May not be suitable for large bodies.
    /// See test_request_multi_part_parse2 for an example of how to use this method.
    pub async fn parse_multipart(&mut self) -> Result<Multipart<Cursor<Bytes>>, HttpError> {
        if let Some(content_type) = self.inner.headers().get("Content-Type") {
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

                let orig_req =
                    std::mem::replace(&mut self.inner, Request::new(P::fixed_body(None)));
                let (orig_parts, orig_body) = orig_req.into_parts();
                let data = orig_body.bytes().await?;

                let rebuilt_req =
                    Request::from_parts(orig_parts, P::fixed_body(Some(data.clone())));
                let _ = std::mem::replace(&mut self.inner, rebuilt_req);

                Ok(multipart::server::Multipart::with_body(
                    Cursor::new(data),
                    boundary,
                ))
            } else {
                Err(ExtractError::InvalidContentType.into())
            }
        } else {
            Err(ExtractError::InvalidHeaderValue.into())
        }
    }

    /// Deserializes"x-www-form-urlencoded" body into a QueryMap.
    pub async fn parse_body_url_encoded(&mut self) -> Result<&QueryMap, HttpError> {
        if let Some(content_type) = self.inner.headers().get("Content-Type") {
            let content_type_str = content_type
                .to_str()
                .ok()
                .and_then(|s| s.split(';').next())
                .unwrap_or_default();
            if content_type_str == "application/x-www-form-urlencoded" {
                let orig_req =
                    std::mem::replace(&mut self.inner, Request::new(P::fixed_body(None)));
                let (orig_parts, orig_body) = orig_req.into_parts();
                let data = orig_body.bytes().await?;

                let mut params = QueryMap::new();
                let params = serde_urlencoded::from_bytes::<QueryMap>(&data).map(|p| {
                    params.extend(p);
                    params
                })?;

                let rebuilt_req = Request::from_parts(orig_parts, P::fixed_body(Some(data)));
                let _ = std::mem::replace(&mut self.inner, rebuilt_req);

                self.body_url_params = Parse::Parsed(params);

                Ok(match &self.body_url_params {
                    Parse::Parsed(inner) => inner,
                    _ => unsafe { unreachable_unchecked() },
                })
            } else {
                Err(ExtractError::InvalidContentType.into())
            }
        } else {
            Err(ExtractError::InvalidHeaderValue.into())
        }
    }
}

impl<P> ParsedRequest<P>
where
    P: futures_core::Stream<Item = Result<Bytes, HttpError>> + Send + 'static,
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
        if let Some(content_type) = self.inner.headers().get("Content-Type") {
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

                let (_parts, body) = self.inner.into_parts();

                let constraints_to_use =
                    user_constraints.unwrap_or_else(|| multer::Constraints::default());
                let m = multer::Multipart::with_constraints(body, boundary, constraints_to_use);
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
    use std::io::{BufRead, Read};

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
        let mut parsed_req = ParsedRequest::new(request);
        parsed_req.parse_cookies_params().unwrap();

        assert_eq!(parsed_req.get_cookie_value("user_id"), Some("123"));
        assert_eq!(parsed_req.get_cookie_value("email"), Some("some_email"));
    }

    #[test]
    fn test_request_cookie_set() {
        let request = build_request();

        let mut parsed_req = ParsedRequest::new(request);
        parsed_req.parse_cookies_params().unwrap();

        parsed_req
            .add_cookie(&Cookie::new("user_id", "123"))
            .unwrap();
        parsed_req
            .add_cookie(&Cookie::new("email", "some_email"))
            .unwrap();

        // Calling into_http_request will call serialize_cookies_into_header
        // and set the Cookie header in the request.
        let new_req = parsed_req.into_http_request().unwrap();
        let cookie_header = new_req.headers().get(COOKIE).unwrap();
        let cookie_str = cookie_header.to_str().unwrap();
        assert_eq!(cookie_str, "user_id=123; email=some_email");
    }

    #[test]
    fn test_request_additional_cookies_set() {
        let request = build_request_with_cookies();
        let mut parsed_req = ParsedRequest::new(request);
        parsed_req.parse_cookies_params().unwrap();

        parsed_req
            .add_cookie(&Cookie::new("org", "ByteDance"))
            .unwrap();
        parsed_req.add_cookie(&Cookie::new("id", "36603")).unwrap();

        parsed_req.serialize_cookies_into_header().unwrap();

        assert_eq!(parsed_req.get_cookie_value("user_id"), Some("123"));
        assert_eq!(parsed_req.get_cookie_value("email"), Some("some_email"));
        assert_eq!(parsed_req.get_cookie_value("org"), Some("ByteDance"));
        assert_eq!(parsed_req.get_cookie_value("id"), Some("36603"));
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
        let mut parsed = ParsedRequest::new(request);

        assert_eq!(parsed.get_url_param("user_id"), Some(&"123".to_string()));
        assert_eq!(
            parsed.get_url_param("email"),
            Some(&"some_email".to_string())
        );
    }

    fn create_request_with_url_encoded_body() -> Request<HttpBody> {
        // let url_str = "https://example.com/api";
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
        let mut parsed_request = ParsedRequest::new(request);

        let query_map = parsed_request.parse_body_url_encoded().await.unwrap();

        assert_eq!(query_map.get("key1"), Some(&"value1".to_string()));
        assert_eq!(query_map.get("key2"), Some(&"value2".to_string()));
    }

    #[monoio::test_all]
    async fn test_request_multi_part_parse() {
        let request = create_request_multi_part();
        let mut parsed_request = ParsedRequest::new(request);

        let mut multipart_struct = parsed_request.parse_multipart().await.unwrap();

        let mut count = 0;
        multipart_struct
            .foreach_entry(|_entry| {
                count += 1;
            })
            .unwrap();

        assert_eq!(count, 3);
    }

    #[monoio::test_all]
    async fn test_request_multi_part_parse2() {
        let request = create_request_multi_part();

        let mut parsed_request = ParsedRequest::new(request);

        let mut multipart_struct = parsed_request.parse_multipart().await.unwrap();

        let mut entry = multipart_struct.read_entry().unwrap().unwrap();
        assert_eq!(&(*entry.headers.name), "field1");
        let bytes = entry.data.fill_buf().unwrap();
        assert_eq!(bytes, b"value1");

        let mut entry = multipart_struct.read_entry().unwrap().unwrap();
        assert_eq!(&(*entry.headers.name), "field2");
        let bytes = entry.data.fill_buf().unwrap();
        assert_eq!(bytes, b"value2");

        let mut entry = multipart_struct.read_entry().unwrap().unwrap();
        assert_eq!(&(*entry.headers.name), "file");
        let mut vec = Vec::new();
        entry.data.read_to_end(&mut vec).unwrap();
        assert_eq!(vec, b"Hello, World!");
        assert_eq!(entry.headers.filename, Some("HelloWorld.txt".to_string()));
        assert_eq!(entry.headers.content_type, Some(mime::TEXT_PLAIN));
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
