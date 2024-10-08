use std::{
    cell::UnsafeCell,
    ops::{Deref, DerefMut},
};

use bytes::{Bytes, BytesMut};
use cookie::{Cookie, CookieJar};
pub use http::request::{Builder as RequestBuilder, Parts as RequestHead};
use http::{
    header::{CONTENT_LENGTH, CONTENT_TYPE, COOKIE},
    Request,
};
use mime::{APPLICATION_WWW_FORM_URLENCODED, MULTIPART_FORM_DATA};

use super::multipart::{FieldHeader, FileHeader, ParsedMultiPartForm};
use crate::common::{
    body::{Body, FixedBody, HttpBodyStream, StreamHint},
    error::{HttpError, ParseError},
    parsed::{Parse, QueryMap},
    IntoParts,
};

pub struct ParsedRequest<P> {
    inner: Request<P>,
    cookie_jar: UnsafeCell<Parse<CookieJar>>,
    url_params: UnsafeCell<Parse<QueryMap>>,
    body_form_params: Parse<QueryMap>,
    multipart_params: Parse<ParsedMultiPartForm>,
    body_cache: Option<Bytes>,
}

impl<P> Deref for ParsedRequest<P> {
    type Target = Request<P>;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl<P> DerefMut for ParsedRequest<P> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}

impl<P> From<Request<P>> for ParsedRequest<P> {
    #[inline]
    fn from(req: Request<P>) -> Self {
        Self {
            inner: req,
            cookie_jar: UnsafeCell::new(Parse::Unparsed),
            url_params: UnsafeCell::new(Parse::Unparsed),
            body_form_params: Parse::Unparsed,
            multipart_params: Parse::Unparsed,
            body_cache: None,
        }
    }
}

impl<P> ParsedRequest<P> {
    #[inline]
    pub const fn new(req: Request<P>) -> Self {
        Self {
            inner: req,
            cookie_jar: UnsafeCell::new(Parse::Unparsed),
            url_params: UnsafeCell::new(Parse::Unparsed),
            body_form_params: Parse::Unparsed,
            multipart_params: Parse::Unparsed,
            body_cache: None,
        }
    }
}

impl<P> ParsedRequest<P> {
    #[inline]
    pub fn cookies(&mut self) -> &mut Parse<CookieJar> {
        // Safe since we hold the &mut self.
        unsafe { &mut *self.cookie_jar.get() }
    }

    #[inline]
    pub fn url_params(&mut self) -> &mut Parse<QueryMap> {
        // Safe since we hold the &mut self.
        unsafe { &mut *self.url_params.get() }
    }

    #[inline]
    pub fn body_form_params(&mut self) -> &mut Parse<QueryMap> {
        &mut self.body_form_params
    }
}

impl<P: FixedBody + From<ParsedMultiPartForm>> From<ParsedRequest<P>> for Request<P> {
    #[inline]
    fn from(pr: ParsedRequest<P>) -> Self {
        pr.into_http_request()
    }
}

impl<P: FixedBody + From<ParsedMultiPartForm>> ParsedRequest<P> {
    #[inline]
    pub fn into_http_request(mut self) -> Request<P> {
        self.serialize_cookies_into_header();
        // Handle URL body encoding
        if let Some(body) = self.body_cache {
            let body = P::fixed_body(Some(body));
            let (parts, _) = self.inner.into_parts();
            let new_req = Request::from_parts(parts, body);
            return new_req;
        }
        // Handle multipart form data
        if self.multipart_params.is_parsed() {
            // Multipart form data is transfered as a stream, chunked body for H1.
            self.inner.headers_mut().remove(CONTENT_LENGTH);
            let body = P::from(self.multipart_params.unwrap());
            let (parts, _) = self.inner.into_parts();
            return Request::from_parts(parts, body);
        }
        self.inner
    }

    #[inline]
    pub fn writeback(&mut self) {
        self.serialize_cookies_into_header();
        if let Some(body) = &self.body_cache {
            let body = P::fixed_body(Some(body.clone()));
            *self.inner.body_mut() = body;
        }
    }
}

impl<P: FixedBody + From<ParsedMultiPartForm>> IntoParts for ParsedRequest<P> {
    type Parts = RequestHead;
    type Body = P;
    #[inline]
    fn into_parts(self) -> (Self::Parts, Self::Body) {
        let req: Request<P> = self.into();
        req.into_parts()
    }
}

// URI parsing methods
impl<P> ParsedRequest<P> {
    /// Parse the URL parameters into a QueryMap.
    pub fn parse_url_params(&self) -> Result<(), ParseError> {
        let url_params = unsafe { &mut *self.url_params.get() };
        if url_params.is_parsed() {
            return Ok(());
        }
        if url_params.is_failed() {
            return Err(ParseError::Previous);
        }

        let uri = self.inner.uri();
        // Allow empty query.
        let Some(query) = uri.query() else {
            *url_params = Parse::Parsed(Default::default());
            return Ok(());
        };

        let map: Vec<(String, String)> = serde_urlencoded::from_str(query).map_err(|e| {
            *url_params = Parse::Failed;
            e
        })?;

        let mut qmap = QueryMap::new();
        for (key, value) in map {
            qmap.entry(key).or_insert(Vec::new()).push(value);
        }
        *url_params = Parse::Parsed(qmap);
        Ok(())
    }

    /// Try parse if not parsed before, then get the value of the URL parameter.
    #[inline]
    pub fn get_url_param(&self, name: &str) -> Result<Option<Vec<&str>>, ParseError> {
        self.parse_url_params()?;
        unsafe {
            let parsed = &*self.url_params.get();
            Ok(parsed
                .as_ref()
                .unwrap_unchecked()
                .get(name)
                .map(|v| v.iter().map(|s| s.as_str()).collect()))
        }
    }
}

impl<P> ParsedRequest<P> {
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

// Request specific, multipart and url encoded body parsing methods
impl<P> ParsedRequest<P>
where
    P: Body<Data = Bytes, Error = HttpError> + FixedBody + Sized,
{
    /// Deserializes"x-www-form-urlencoded" body into a QueryMap.
    pub async fn parse_body_url_encoded_params(&mut self) -> Result<&mut QueryMap, ParseError> {
        if self.body_form_params.is_parsed() {
            return Ok(unsafe { self.body_form_params.as_mut().unwrap_unchecked() });
        }
        if self.body_form_params.is_failed() {
            return Err(ParseError::Previous);
        }

        match self.inner.headers().get(CONTENT_TYPE) {
            Some(content_type)
                if content_type.as_bytes()
                    == APPLICATION_WWW_FORM_URLENCODED.as_ref().as_bytes() => {}
            _ => {
                self.body_form_params = Parse::Failed;
                return Err(ParseError::InvalidContentType);
            }
        }
        let data = match self.inner.body_mut().stream_hint() {
            StreamHint::None => Ok(Bytes::new()),
            StreamHint::Fixed => self
                .inner
                .body_mut()
                .next_data()
                .await
                .unwrap_or(Ok(Bytes::new())),
            StreamHint::Stream => {
                let mut data = BytesMut::new();
                while let Some(chunk) = self.inner.body_mut().next_data().await {
                    data.extend_from_slice(&chunk?);
                }
                Ok(data.freeze())
            }
        }?;
        *self.inner.body_mut() = P::fixed_body(Some(data.clone()));

        let body_ref: &mut Bytes = self.body_cache.insert(data);
        let map: Vec<(String, String)> = serde_urlencoded::from_bytes(body_ref).map_err(|e| {
            self.body_form_params = Parse::Failed;
            e
        })?;

        let mut form = QueryMap::new();
        for (key, map) in map {
            form.entry(key).or_insert(Vec::new()).push(map);
        }
        self.body_form_params = Parse::Parsed(form);

        Ok(unsafe { self.body_form_params.as_mut().unwrap_unchecked() })
    }

    /// Get the value of the body URL parameter.
    /// If the body URL parameters are not parsed before, this method will parse and get the value.
    pub async fn parse_get_body_url_encoded_param(
        &mut self,
        name: &str,
    ) -> Result<Option<Vec<&str>>, ParseError> {
        self.parse_body_url_encoded_params().await?;
        Ok(unsafe { self.body_form_params.as_ref().unwrap_unchecked() }
            .get(name)
            .map(|v| v.iter().map(|s| s.as_str()).collect()))
    }

    /// Get the value of the body URL parameter.
    /// Note: if the body URL parameters are not parsed before, this method will return None.
    pub fn get_body_url_encoded_param(&self, name: &str) -> Option<Vec<&str>> {
        match self.body_form_params {
            Parse::Parsed(ref map) => map
                .get(name)
                .map(|v| v.iter().map(|s| s.as_str()).collect()),
            _ => None,
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
    ) -> Result<multer::Multipart<'a>, ParseError> {
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

                let body_stream: HttpBodyStream = body.into();

                let constraints_to_use = user_constraints.unwrap_or_default();
                let m =
                    multer::Multipart::with_constraints(body_stream, boundary, constraints_to_use);
                Ok(m)
            } else {
                Err(ParseError::InvalidContentType)
            }
        } else {
            Err(ParseError::InvalidHeaderValue)
        }
    }
}

impl<P> ParsedRequest<P>
where
    P: Into<HttpBodyStream> + 'static + FixedBody,
{
    /// See https://docs.rs/multer/latest/multer/struct.Constraints.html for constraints.
    /// Size limits for whole stream body, per field, allowed fields etc.
    /// Any field with a file size greater than max_file_size will be stored on disk.
    /// Default max_file_size is 10MB.
    pub async fn parse_multipart_params<'a>(
        &mut self,
        user_constraints: Option<multer::Constraints>,
        max_file_size: Option<u64>,
    ) -> Result<&mut ParsedMultiPartForm, ParseError> {
        if self.multipart_params.is_parsed() {
            return Ok(unsafe { self.multipart_params.as_mut().unwrap_unchecked() });
        }

        if self.multipart_params.is_failed() {
            return Err(ParseError::Previous);
        }

        println!("{:?}", self.inner.headers().get(CONTENT_TYPE));

        let boundary = match self.inner.headers().get(CONTENT_TYPE) {
            Some(content_type)
                if content_type
                    .to_str()
                    .ok()
                    .and_then(|s| s.split(';').next())
                    .unwrap_or_default()
                    .as_bytes()
                    == MULTIPART_FORM_DATA.as_ref().as_bytes() =>
            {
                content_type
                    .to_str()
                    .ok()
                    .and_then(|s| s.split("boundary=").nth(1))
                    .unwrap_or_default()
                    .to_string()
            }
            _ => {
                self.multipart_params = Parse::Failed;
                return Err(ParseError::InvalidContentType);
            }
        };

        let body = std::mem::replace(self.inner.body_mut(), P::fixed_body(None));
        let body_stream: HttpBodyStream = body.into();

        let constraints_to_use = user_constraints.unwrap_or_default();
        let multer =
            multer::Multipart::with_constraints(body_stream, boundary.clone(), constraints_to_use);

        let max_file_size = max_file_size.unwrap_or(super::multipart::MAX_FILE_SIZE);

        let parsed_multi_part =
            ParsedMultiPartForm::read_form(multer, boundary, max_file_size).await?;

        self.multipart_params = Parse::Parsed(parsed_multi_part);

        Ok(unsafe { self.multipart_params.as_mut().unwrap_unchecked() })
    }

    pub async fn parse_get_multipart_field_param(
        &mut self,
        name: &str,
    ) -> Result<Option<Vec<FieldHeader>>, ParseError> {
        self.parse_multipart_params(None, None).await?;
        Ok(unsafe { self.multipart_params.as_mut().unwrap_unchecked() }.get_field_value(name))
    }

    pub fn get_multipart_field_param(&self, name: &str) -> Option<Vec<FieldHeader>> {
        match self.multipart_params {
            Parse::Parsed(ref map) => map.get_field_value(name),
            _ => None,
        }
    }

    pub async fn parse_get_multipart_file_param(
        &mut self,
        name: &str,
    ) -> Result<Option<Vec<FileHeader>>, ParseError> {
        self.parse_multipart_params(None, None).await?;
        Ok(unsafe { self.multipart_params.as_mut().unwrap_unchecked() }.get_file(name))
    }

    pub fn get_multipart_file_param(&self, name: &str) -> Option<Vec<FileHeader>> {
        match self.multipart_params {
            Parse::Parsed(ref map) => map.get_file(name),
            _ => None,
        }
    }

    pub fn get_multipart_value_keys(&self) -> Option<impl Iterator<Item = &String>> {
        match &self.multipart_params {
            Parse::Parsed(ref map) => Some(map.value_keys()),
            _ => None,
        }
    }

    pub fn get_multipart_file_keys(&self) -> Option<impl Iterator<Item = &String>> {
        match &self.multipart_params {
            Parse::Parsed(ref map) => Some(map.file_keys()),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{io::Read, path::PathBuf, vec};

    use http::header::HeaderValue;

    use super::*;
    use crate::common::body::{BodyExt, HttpBody};

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
            parsed_req
                .get_cookie_param("user_id")
                .unwrap()
                .unwrap()
                .value(),
            "123"
        );
        assert_eq!(
            parsed_req
                .get_cookie_param("email")
                .unwrap()
                .unwrap()
                .value(),
            "some_email"
        );
    }

    #[test]
    fn test_request_cookie_set() {
        let request = build_request();

        let parsed_req = ParsedRequest::new(request);
        parsed_req.parse_cookies_params().unwrap();

        parsed_req
            .set_cookie_param(Cookie::new("user_id", "123"))
            .unwrap();
        parsed_req
            .set_cookie_param(Cookie::new("email", "some_email"))
            .unwrap();

        // Calling into_http_request will call serialize_cookies_into_header
        // and set the Cookie header in the request.
        let new_req = parsed_req.into_http_request();
        let cookie_header = new_req.headers().get(COOKIE).unwrap();
        let cookie_str = cookie_header.to_str().unwrap();
        assert!(
            cookie_str == "email=some_email; user_id=123"
                || cookie_str == "user_id=123; email=some_email"
        );
    }

    #[test]
    fn test_request_additional_cookies_set() {
        let request = build_request_with_cookies();
        let parsed_req = ParsedRequest::new(request);
        parsed_req.parse_cookies_params().unwrap();

        parsed_req
            .set_cookie_param(Cookie::new("org", "ByteDance"))
            .unwrap();
        parsed_req
            .set_cookie_param(Cookie::new("id", "36603"))
            .unwrap();

        assert_eq!(
            parsed_req
                .get_cookie_param("user_id")
                .unwrap()
                .unwrap()
                .value(),
            "123"
        );
        assert_eq!(
            parsed_req
                .get_cookie_param("email")
                .unwrap()
                .unwrap()
                .value(),
            "some_email"
        );
        assert_eq!(
            parsed_req.get_cookie_param("org").unwrap().unwrap().value(),
            "ByteDance"
        );
        assert_eq!(
            parsed_req.get_cookie_param("id").unwrap().unwrap().value(),
            "36603"
        );
    }

    fn create_request_with_url_params() -> Request<HttpBody> {
        let body = HttpBody::fixed_body(None);
        let mut request = Request::new(body);

        let uri = http::Uri::builder()
            .scheme("http")
            .authority("example.com")
            .path_and_query("/path?user_id=123&email=some_email&email=another_email")
            .build()
            .unwrap();

        *request.uri_mut() = uri;

        request
    }

    #[test]
    fn test_request_url_params_parse() {
        let request = create_request_with_url_params();
        let parsed = ParsedRequest::new(request);

        assert_eq!(parsed.get_url_param("user_id").unwrap(), Some(vec!["123"]));
        assert_eq!(
            parsed.get_url_param("email").unwrap(),
            Some(vec!["some_email", "another_email"])
        );
    }

    fn create_request_with_url_encoded_body() -> Request<HttpBody> {
        let form_data = vec![("key1", "value1"), ("key2", "value2"), ("key1", "value3")];

        let body = serde_urlencoded::to_string(form_data).expect("Failed to serialize form data");
        let mut request = Request::new(HttpBody::fixed_body(Some(Bytes::from(body))));
        request.headers_mut().insert(
            http::header::CONTENT_TYPE,
            HeaderValue::from_static("application/x-www-form-urlencoded"),
        );
        request
    }

    #[monoio::test_all]
    async fn test_request_url_encoded_body_parse() {
        use crate::common::body::BodyExt;
        let request = create_request_with_url_encoded_body();
        let mut parsed_request = ParsedRequest::new(request);

        let query_map = parsed_request
            .parse_body_url_encoded_params()
            .await
            .unwrap();

        assert_eq!(
            query_map.get("key1"),
            Some(&vec!["value1".to_string(), "value3".to_string()])
        );
        assert_eq!(query_map.get("key2"), Some(&vec!["value2".to_string()]));

        let form_data = vec![("key1", "value1"), ("key2", "value2"), ("key1", "value3")];
        let orig_body =
            serde_urlencoded::to_string(form_data).expect("Failed to serialize form data");

        let (_parts, body) = parsed_request.into_parts();
        assert_eq!(body.bytes().await.unwrap(), orig_body.into_bytes());
    }

    #[monoio::test_all]
    async fn test_request_url_encoded_body_parse2() {
        let request = create_request_with_url_encoded_body();
        let mut parsed_request = ParsedRequest::new(request);

        parsed_request
            .parse_body_url_encoded_params()
            .await
            .unwrap();

        let res1 = parsed_request.get_body_url_encoded_param("key1");
        assert_eq!(res1, Some(vec!["value1", "value3"]));
        assert_eq!(
            parsed_request.get_body_url_encoded_param("key2"),
            Some(vec!["value2"])
        );
    }

    fn create_request_multi_part() -> Request<HttpBody> {
        let body = b"--iYJaNWIc97YKxZYB\r\ncontent-disposition: form-data; name=\"field2\"\r\n\r\nvalue2\r\n--iYJaNWIc97YKxZYB\r\ncontent-disposition: form-data; name=\"field1\"\r\n\r\nvalue1\r\n--iYJaNWIc97YKxZYB\r\ncontent-disposition: form-data; name=\"file\"; filename=\"HelloWorld.txt\"\r\ncontent-type: text/plain\r\n\r\nHello, World!\nHello, World!\nHello, World!\nHello, World!\nHello, World!\nHello, World!\n\r\n--iYJaNWIc97YKxZYB--\r\n";

        let boundary = "iYJaNWIc97YKxZYB";
        let content_type = format!("multipart/form-data; boundary={}", boundary);
        let body = HttpBody::fixed_body(Some(Bytes::from(body.to_vec())));
        let mut request = Request::new(body);
        request.headers_mut().insert(
            http::header::CONTENT_TYPE,
            HeaderValue::from_str(content_type.as_str()).unwrap(),
        );

        request
    }

    fn create_request_multi_part_files_only() -> Request<HttpBody> {
        let body = b"--iYJaNWIc97YKxZYB\r\ncontent-disposition: form-data; name=\"file1\"; filename=\"File1.txt\"\r\ncontent-type: text/plain\r\n\r\nHello from world1.\r\n--iYJaNWIc97YKxZYB\r\ncontent-disposition: form-data; name=\"file2\"; filename=\"File2.txt\"\r\ncontent-type: text/plain\r\n\r\nHello from world2.\r\n--iYJaNWIc97YKxZYB--\r\n";
        let boundary = "iYJaNWIc97YKxZYB";
        let content_type = format!("multipart/form-data; boundary={}", boundary);
        let body = HttpBody::fixed_body(Some(Bytes::from(body.to_vec())));
        let mut request = Request::new(body);
        request.headers_mut().insert(
            http::header::CONTENT_TYPE,
            HeaderValue::from_str(content_type.as_str()).unwrap(),
        );

        request
    }

    #[monoio::test_all]
    async fn test_request_parsed_multipartform_to_body() {
        let request = create_request_multi_part();
        let (part, body) = request.into_parts();
        let mp_body_bytes_1 = body.bytes().await.unwrap();
        let request =
            Request::from_parts(part, HttpBody::fixed_body(Some(mp_body_bytes_1.clone())));

        let mut parsed_request = ParsedRequest::new(request);

        parsed_request
            .parse_multipart_params(None, None)
            .await
            .unwrap();

        let result = parsed_request.get_multipart_field_param("field1").unwrap();
        assert_eq!(result[0].value, "value1");

        let result = parsed_request.get_multipart_field_param("field2").unwrap();
        assert_eq!(result[0].value, "value2");

        let req_mp = parsed_request.into_http_request();
        let mp_converted_body_bytes = req_mp.into_body().bytes().await.unwrap();

        // Order of fields can be different
        let body = b"--iYJaNWIc97YKxZYB\r\ncontent-disposition: form-data; name=\"field1\"\r\n\r\nvalue1\r\n--iYJaNWIc97YKxZYB\r\ncontent-disposition: form-data; name=\"field2\"\r\n\r\nvalue2\r\n--iYJaNWIc97YKxZYB\r\ncontent-disposition: form-data; name=\"file\"; filename=\"HelloWorld.txt\"\r\ncontent-type: text/plain\r\n\r\nHello, World!\nHello, World!\nHello, World!\nHello, World!\nHello, World!\nHello, World!\n\r\n--iYJaNWIc97YKxZYB--\r\n";
        let mp_body_bytes_2 = Bytes::from(body.to_vec());

        assert!(
            mp_converted_body_bytes == mp_body_bytes_1
                || mp_converted_body_bytes == mp_body_bytes_2
        );
    }

    fn read_file(path: PathBuf) -> Vec<u8> {
        let mut file = std::fs::File::open(path).unwrap();
        let mut buf = Vec::new();
        file.read_to_end(&mut buf).unwrap();
        buf
    }

    #[monoio::test_all]
    async fn test_request_parsed_multipartform_files() {
        let request = create_request_multi_part_files_only();
        let (part, body) = request.into_parts();
        let mp_body_bytes_1 = body.bytes().await.unwrap();
        let request =
            Request::from_parts(part, HttpBody::fixed_body(Some(mp_body_bytes_1.clone())));

        let mut parsed_request = ParsedRequest::new(request);

        // Restrict Max file size to 2 bytes, File will be stored on disk instead
        // of in memory
        parsed_request
            .parse_multipart_params(None, Some(2))
            .await
            .unwrap();

        let result = parsed_request.get_multipart_file_param("file1").unwrap();

        assert_eq!(result[0].get_filename(), "File1.txt");
        let path = result[0].get_file_path().unwrap();
        assert_eq!(&read_file(path), b"Hello from world1.");

        let result = parsed_request.get_multipart_file_param("file2").unwrap();
        assert_eq!(result[0].get_filename(), "File2.txt");
        let path = result[0].get_file_path().unwrap();
        assert_eq!(&read_file(path), b"Hello from world2.");

        let req_mp = parsed_request.into_http_request();
        let mp_converted_body_bytes = req_mp.into_body().bytes().await.unwrap();

        // Order of fields can be different
        let body = b"--iYJaNWIc97YKxZYB\r\ncontent-disposition: form-data; name=\"file2\"; filename=\"File2.txt\"\r\ncontent-type: text/plain\r\n\r\nHello from world2.\r\n--iYJaNWIc97YKxZYB\r\ncontent-disposition: form-data; name=\"file1\"; filename=\"File1.txt\"\r\ncontent-type: text/plain\r\n\r\nHello from world1.\r\n--iYJaNWIc97YKxZYB--\r\n";
        let mp_body_bytes_2 = Bytes::from(body.to_vec());

        assert!(
            mp_converted_body_bytes == mp_body_bytes_1
                || mp_converted_body_bytes == mp_body_bytes_2
        );
    }
}
