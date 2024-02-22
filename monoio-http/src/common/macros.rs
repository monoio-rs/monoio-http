#[macro_export]
macro_rules! impl_cookie_extractor {
    ($struct:ident, $orig_struct:ident, $cookie_str:literal) => {
        impl<P> $struct<P> {
            /// Builds a cookie jar from the $struct's headers.
            fn build_cookie_jar(r: &$orig_struct<P>) -> Result<CookieJar, ExtractError> {
                let headers = r.headers();
                let mut jar = CookieJar::new();

                if let Some(cookie_header) = headers.get($cookie_str) {
                    let cookie_str = cookie_header
                        .to_str()
                        .map_err(|_| ExtractError::InvalidHeaderValue)?;
                    let cookie_owned_str = cookie_str.to_string();
                    for cookie in Cookie::split_parse(cookie_owned_str) {
                        let cookie = cookie?;
                        jar.add_original(cookie);
                    }
                }

                Ok(jar)
            }

            /// Parse cookies from the $orig_struct's headers and return a new $struct.
            /// Returns the original $orig_struct if the cookie header is invalid.
            pub fn parse_cookies_params(
                r: $orig_struct<P>,
            ) -> Result<Self, ($orig_struct<P>, HttpError)> {
                let jar = match Self::build_cookie_jar(&r) {
                    Ok(jar) => jar,
                    Err(e) => return Err((r, e.into())),
                };
                Ok(Self {
                    inner: r,
                    cookie_jar: Some(jar),
                    url_params: None,
                })
            }

            /// Sets a cookie in the cookie jar.
            fn set_cookie(
                &mut self,
                cookie: &Cookie<'static>,
                original: bool,
            ) -> Result<(), HttpError> {
                match self.cookie_jar.as_mut() {
                    Some(jar) => {
                        if original {
                            jar.add_original(cookie.clone());
                        } else {
                            jar.add(cookie.clone());
                        }
                        Ok(())
                    }
                    None => Err(ExtractError::UninitializedCookieJar.into()),
                }
            }

            /// Adds a cookie to the cookie jar.
            pub fn add_cookie(&mut self, cookie: &Cookie<'static>) -> Result<(), HttpError> {
                self.set_cookie(cookie, false)
            }

            /// Adds an original cookie to the cookie jar.
            pub fn add_original_cookie(
                &mut self,
                cookie: &Cookie<'static>,
            ) -> Result<(), HttpError> {
                self.set_cookie(cookie, true)
            }

            /// Returns the cookie struct from the cookie jar by name.
            pub fn get_cookie(&self, name: &str) -> Option<&Cookie> {
                self.cookie_jar.as_ref().and_then(|jar| jar.get(name))
            }

            /// Returns the value of the cookie from the cookie jar by name.
            pub fn get_cookie_value(&self, name: &str) -> Option<&str> {
                self.get_cookie(name).map(|cookie| cookie.value())
            }

            /// Returns the cookie jar from the $struct.
            pub fn get_cookie_jar(&self) -> Option<&CookieJar> {
                self.cookie_jar.as_ref()
            }

            /// Overwrites the `Cookie` header in the $orig_struct inside the $struct.
            /// Alternatively just call into_http_request to get the original $orig_struct with
            /// the updated cookie header.
            pub fn serialize_cookies_into_header(&mut self) -> Result<(), HttpError> {
                match self.cookie_jar.as_ref() {
                    Some(jar) => {
                        let cookies = jar
                            .iter()
                            .map(|cookie| cookie.encoded().to_string())
                            .collect::<Vec<String>>()
                            .join("; ");
                        let header_value = match http::header::HeaderValue::from_str(&cookies) {
                            Ok(value) => value,
                            Err(_) => return Err(ExtractError::InvalidHeaderValue.into()),
                        };
                        self.inner.headers_mut().insert($cookie_str, header_value);
                        Ok(())
                    }
                    None => Err(ExtractError::UninitializedCookieJar.into()),
                }
            }
        }
    };
}
