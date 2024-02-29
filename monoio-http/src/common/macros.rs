#[macro_export]
macro_rules! impl_cookie_extractor {
    ($struct:ident, $cookie_str:literal) => {
        impl<P> $struct<P> {
            /// Builds a cookie jar from the $struct's headers.
            fn build_cookie_jar(
                headers: &HeaderMap<HeaderValue>,
            ) -> Result<CookieJar, ExtractError> {
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

            /// Parse cookies in the headers and return the cookiejar
            pub fn parse_cookies_params(&mut self) -> Result<&CookieJar, HttpError> {
                match Self::build_cookie_jar(&self.inner.headers()) {
                    Ok(jar) => {
                        self.cookie_jar = Parse::Parsed(jar);
                    }
                    Err(e) => return Err(e.into()),
                }

                Ok(match &self.cookie_jar {
                    Parse::Parsed(jar) => jar,
                    _ => unsafe { unreachable_unchecked() },
                })
            }

            /// Adds a cookie to the cookie jar.
            pub fn add_cookie(&mut self, cookie: &Cookie<'static>) -> Result<(), HttpError> {
                match self.cookie_jar {
                    Parse::Parsed(ref mut jar) => {
                        jar.add(cookie.clone());
                        Ok(())
                    }
                    _ => return Err(ExtractError::UninitializedCookieJar.into()),
                }
            }

            /// Returns the cookie struct from the cookie jar by name.
            pub fn get_cookie(&self, name: &str) -> Option<&Cookie> {
                match self.cookie_jar {
                    Parse::Parsed(ref jar) => jar.get(name),
                    _ => None,
                }
            }

            /// Returns the value of the cookie from the cookie jar by name.
            pub fn get_cookie_value(&self, name: &str) -> Option<&str> {
                match self.cookie_jar {
                    Parse::Parsed(ref jar) => jar.get(name).map(|c| c.value()),
                    _ => None,
                }
            }

            /// Returns the cookie jar from the $struct.
            pub fn get_cookie_jar(&self) -> Option<&CookieJar> {
                match self.cookie_jar {
                    Parse::Parsed(ref jar) => Some(jar),
                    _ => None,
                }
            }

            /// Overwrites the `Cookie` header with the latest contents of the cookieJar.
            /// Converting to Request or Response will automatically call this method.
            pub fn serialize_cookies_into_header(&mut self) -> Result<(), HttpError> {
                match self.cookie_jar {
                    Parse::Parsed(ref jar) => {
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
                    _ => Err(ExtractError::UninitializedCookieJar.into()),
                }
            }
        }
    };
}
