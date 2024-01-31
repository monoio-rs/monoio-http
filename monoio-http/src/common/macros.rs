#[macro_export]
macro_rules! impl_cookie_extractor {
    ($struct:ident, $cookie_str:literal) => {
        impl<P> $struct<P> {
            /// Builds a cookie jar from the $struct's headers.
            pub fn build_cookie_jar(&mut self) -> Result<CookieJar, ExtractError> {
                let headers = self.inner.headers();
                let mut jar = CookieJar::new();

                if let Some(cookie_header) = headers.get($cookie_str) {
                    let cookie_str = cookie_header
                        .to_str()
                        .map_err(|_| ExtractError::InvalidHeaderValue)?;
                    let cookie_owned_str = cookie_str.to_string();
                    println!("{}", cookie_owned_str);
                    for cookie in Cookie::split_parse(cookie_owned_str) {
                        let cookie = cookie?;
                        jar.add_original(cookie);
                    }
                }

                Ok(jar)
            }

            /// Parse cookies from the $struct. This will overwrite any existing cookies in the
            /// cookie jar. If you want to add cookies to the jar, use `add_cookie` or
            /// `add_original_cookie`. Lazy parsing is used, so the cookies are only parsed when
            /// this method is called.
            pub fn parse_cookies_from_header(&mut self) -> Result<(), HttpError> {
                let jar = self.build_cookie_jar()?;
                self.cookie_jar = Some(jar);
                Ok(())
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

            /// Returns an error if the cookie jar is not initialized via
            /// `parse_cookies_from_header`. Overwrites the `Cookie` header in the $struct with
            /// the cookies from the cookie jar. User should call this method before splitting
            /// the $struct into parts, or any changes to the cookie jar will not be reflected
            /// in the $struct.
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
