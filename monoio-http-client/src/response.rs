// use bytes::Bytes;
// use http::{Extensions, HeaderMap, HeaderValue, StatusCode, Version};
// use monoio_http::common::body::{BodyExt, HttpBody};

// pub struct ClientResponse {
//     /// The response's status
//     status: StatusCode,
//     /// The response's version
//     version: Version,
//     /// The response's headers
//     headers: HeaderMap<HeaderValue>,
//     /// The response's extensions
//     extensions: Extensions,
//     /// Payload
//     body: HttpBody,
// }

// impl ClientResponse {
//     pub fn new(inner: http::Response<HttpBody>) -> Self {
//         let (head, body) = inner.into_parts();
//         Self {
//             status: head.status,
//             version: head.version,
//             headers: head.headers,
//             extensions: head.extensions,
//             body,
//         }
//     }

//     /// Get the `StatusCode` of this `Response`.
//     #[inline]
//     pub fn status(&self) -> StatusCode {
//         self.status
//     }

//     /// Get the HTTP `Version` of this `Response`.
//     #[inline]
//     pub fn version(&self) -> Version {
//         self.version
//     }

//     /// Get the `Headers` of this `Response`.
//     #[inline]
//     pub fn headers(&self) -> &HeaderMap {
//         &self.headers
//     }

//     /// Get a mutable reference to the `Headers` of this `Response`.
//     #[inline]
//     pub fn headers_mut(&mut self) -> &mut HeaderMap {
//         &mut self.headers
//     }

//     /// Returns a reference to the associated extensions.
//     pub fn extensions(&self) -> &http::Extensions {
//         &self.extensions
//     }

//     /// Returns a mutable reference to the associated extensions.
//     pub fn extensions_mut(&mut self) -> &mut http::Extensions {
//         &mut self.extensions
//     }

//     /// Get the full response body as `Bytes`.
//     pub async fn bytes(self) -> crate::Result<Bytes> {
//         let body = self.body;
//         body.bytes().await.map_err(Into::into)
//     }

//     /// Get raw body(Payload).
//     pub fn raw_body(self) -> HttpBody {
//         self.body
//     }

//     /// Try to deserialize the response body as JSON.
//     // Note: using from_reader to read discontinuous data pays more
//     // than using from_slice. So here we read the entire content into
//     // a Bytes and call from_slice.
//     pub async fn json<T: serde::de::DeserializeOwned>(self) -> crate::Result<T> {
//         let bytes = self.body.bytes().await?;
//         let d = serde_json::from_slice(&bytes)?;
//         Ok(d)
//     }
// }
