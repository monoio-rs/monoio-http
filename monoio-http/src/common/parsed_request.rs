use std::{collections::HashMap, io::Cursor};

use bytes::Bytes;
use cookie::Cookie; // Import the Cookie type from the cookie crate
use cookie::CookieJar;
pub use http::request::{Builder as RequestBuilder, Parts as RequestHead};
use multipart::server::Multipart;

use super::{
    body::{Body, FixedBody},
    error::{ExtractError, HttpError},
    request::Request,
};
use crate::{common::IntoParts, impl_cookie_extractor};

#[derive(Clone)]
pub struct ParsedRequest<P> {
    inner: Request<P>,
    cookie_jar: Option<CookieJar>,
    url_params: Option<HashMap<String, String>>,
}

impl<P> ParsedRequest<P> {
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
    fn into_parts(self) -> (Self::Parts, Self::Body) {
        self.inner.into_parts()
    }
}

// URI parsing methods
impl<P> ParsedRequest<P> {
    pub fn parse_url_params(req: Request<P>) -> Result<Self, (Request<P>, HttpError)> {
        let url = req.uri();
        let query = url.query().unwrap_or("");
        let params = match serde_urlencoded::from_str(query) {
            Ok(params) => params,
            Err(_e) => return Err((req, HttpError::SerDeError)),
        };

        Ok(Self {
            inner: req,
            cookie_jar: None,
            url_params: Some(params),
        })
    }

    pub fn get_url_param(&self, name: &str) -> Option<&String> {
        self.url_params.as_ref().and_then(|params| params.get(name))
    }
}

// Cookie parsing and serialization methods
impl_cookie_extractor!(ParsedRequest, Request, "COOKIE");

// Request specific, multipart and url encoded body parsing methods
impl<P> ParsedRequest<P>
where
    P: Body<Data = Bytes, Error = HttpError> + FixedBody + Sized,
{
    /// Consumes the request, streams the entry body and returns a `Multipart` struct.
    /// See test_request_multi_part_parse2 for an example of how to use this method.
    pub async fn parse_multipart(
        req: Request<P>,
    ) -> Result<Multipart<Cursor<Bytes>>, (Option<Request<P>>, HttpError)> {
        if let Some(content_type) = req.headers().get("Content-Type") {
            let content_type_str = content_type
                .to_str()
                .ok()
                .and_then(|s| s.split(";").next())
                .unwrap_or_default();

            if content_type_str == "multipart/form-data" {
                // Parse the multipart request
                let boundary = content_type
                    .to_str()
                    .ok()
                    .and_then(|s| s.split("boundary=").nth(1))
                    .unwrap_or_default()
                    .to_string();
                super::body::parse_body_multipart(req.into_body(), boundary)
                    .await
                    .map_err(|e| (None, e))
            } else {
                Err((Some(req), ExtractError::InvalidContentType.into()))
            }
        } else {
            Err((Some(req), ExtractError::InvalidHeaderValue.into()))
        }
    }

    /// Consumes the request and deserializes"x-www-form-urlencoded" body into a HashMap.
    pub async fn parse_body_url_encoded(
        req: Request<P>,
    ) -> Result<HashMap<String, String>, (Option<Request<P>>, HttpError)> {
        if let Some(content_type) = req.headers().get("Content-Type") {
            let content_type_str = content_type
                .to_str()
                .ok()
                .and_then(|s| s.split(";").next())
                .unwrap_or_default();
            if content_type_str == "application/x-www-form-urlencoded" {
                super::body::parse_body_url_encoded(req.into_body())
                    .await
                    .map_err(|e| (None, e))
            } else {
                Err((Some(req), ExtractError::InvalidContentType.into()))
            }
        } else {
            Err((Some(req), ExtractError::InvalidHeaderValue.into()))
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
        let request = Request::new(body);
        request
    }

    #[test]
    fn test_request_cookie_parse() {
        let request = build_request_with_cookies();
        let parsed_req = ParsedRequest::parse_cookies_params(request).unwrap();
        assert_eq!(parsed_req.get_cookie_value("user_id"), Some("123"));
        assert_eq!(parsed_req.get_cookie_value("email"), Some("some_email"));
    }

    #[test]
    fn test_request_cookie_set() {
        let request = build_request();

        let mut parsed_req = ParsedRequest::parse_cookies_params(request).unwrap();
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
        let mut parsed_req = ParsedRequest::parse_cookies_params(request).unwrap();

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
        let parsed_req = ParsedRequest::parse_url_params(request).unwrap();

        assert_eq!(
            parsed_req.get_url_param("user_id"),
            Some(&"123".to_string())
        );
        assert_eq!(
            parsed_req.get_url_param("email"),
            Some(&"some_email".to_string())
        );
    }

    fn create_request_with_url_encoded_body() -> Request<HttpBody> {
        // let url_str = "https://example.com/api";
        let form_data = vec![("key1", "value1"), ("key2", "value2")];

        let body = serde_urlencoded::to_string(&form_data).expect("Failed to serialize form data");
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

        let result = ParsedRequest::parse_body_url_encoded(request)
            .await
            .unwrap();

        assert_eq!(result.get("key1"), Some(&"value1".to_string()));
        assert_eq!(result.get("key2"), Some(&"value2".to_string()));
    }

    #[monoio::test_all]
    async fn test_request_multi_part_parse() {
        let request = create_request_multi_part();
        let mut result = ParsedRequest::parse_multipart(request).await.unwrap();

        let mut count = 0;
        result
            .foreach_entry(|_entry| {
                count += 1;
            })
            .unwrap();

        assert_eq!(count, 3);
    }

    #[monoio::test_all]
    async fn test_request_multi_part_parse2() {
        let request = create_request_multi_part();
        let mut result = ParsedRequest::parse_multipart(request).await.unwrap();

        let mut entry = result.read_entry().unwrap().unwrap();
        assert_eq!(&(*entry.headers.name), "field1");
        let bytes = entry.data.fill_buf().unwrap();
        assert_eq!(bytes, b"value1");

        let mut entry = result.read_entry().unwrap().unwrap();
        assert_eq!(&(*entry.headers.name), "field2");
        let bytes = entry.data.fill_buf().unwrap();
        assert_eq!(bytes, b"value2");

        let mut entry = result.read_entry().unwrap().unwrap();
        assert_eq!(&(*entry.headers.name), "file");
        let mut vec = Vec::new();
        entry.data.read_to_end(&mut vec).unwrap();
        assert_eq!(vec, b"Hello, World!");
        assert_eq!(entry.headers.filename, Some("HelloWorld.txt".to_string()));
        assert_eq!(entry.headers.content_type, Some(mime::TEXT_PLAIN));
    }
}
