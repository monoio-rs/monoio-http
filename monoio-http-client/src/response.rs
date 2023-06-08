use std::hint::unreachable_unchecked;

use bytes::{Bytes, BytesMut};
use http::{Extensions, HeaderMap, HeaderValue, StatusCode, Version};
use monoio::io::stream::Stream;
use monoio_http::h1::payload::Payload;

pub struct ClientResponse {
    /// The response's status
    status: StatusCode,
    /// The response's version
    version: Version,
    /// The response's headers
    headers: HeaderMap<HeaderValue>,
    /// The response's extensions
    extensions: Extensions,
    /// Payload
    body: Payload,
}

impl ClientResponse {
    pub fn new(inner: http::Response<Payload>) -> Self {
        let (head, body) = inner.into_parts();
        Self {
            status: head.status,
            version: head.version,
            headers: head.headers,
            extensions: head.extensions,
            body,
        }
    }

    /// Get the `StatusCode` of this `Response`.
    #[inline]
    pub fn status(&self) -> StatusCode {
        self.status
    }

    /// Get the HTTP `Version` of this `Response`.
    #[inline]
    pub fn version(&self) -> Version {
        self.version
    }

    /// Get the `Headers` of this `Response`.
    #[inline]
    pub fn headers(&self) -> &HeaderMap {
        &self.headers
    }

    /// Get a mutable reference to the `Headers` of this `Response`.
    #[inline]
    pub fn headers_mut(&mut self) -> &mut HeaderMap {
        &mut self.headers
    }

    /// Returns a reference to the associated extensions.
    pub fn extensions(&self) -> &http::Extensions {
        &self.extensions
    }

    /// Returns a mutable reference to the associated extensions.
    pub fn extensions_mut(&mut self) -> &mut http::Extensions {
        &mut self.extensions
    }

    /// Get the full response body as `Bytes`.
    pub async fn bytes(self) -> crate::Result<Bytes> {
        match self.body {
            Payload::None => Ok(Bytes::new()),
            Payload::Fixed(p) => p.get().await.map_err(Into::into),
            Payload::Stream(mut s) => {
                let mut ret = BytesMut::new();
                while let Some(payload_result) = s.next().await {
                    let data = payload_result?;
                    ret.extend(data);
                }
                Ok(ret.freeze())
            }
            Payload::H2BodyStream(mut s) => {
                let mut ret = BytesMut::new();
                while let Some(payload_result) = s.data().await {
                    let data = payload_result.unwrap();
                    ret.extend(data);
                }
                Ok(ret.freeze())
            }
        }
    }

    /// Stream a chunk of the response body.
    pub async fn chunk(&mut self) -> crate::Result<Option<Bytes>> {
        match &mut self.body {
            Payload::None => Ok(None),
            Payload::Fixed(_) => {
                let p = match std::mem::replace(&mut self.body, Payload::None) {
                    Payload::None => unsafe { unreachable_unchecked() },
                    Payload::Fixed(p) => p,
                    Payload::Stream(_) => unsafe { unreachable_unchecked() },
                    Payload::H2BodyStream(_) => unsafe { unreachable_unchecked() },
                };
                p.get().await.map_err(Into::into).map(Option::Some)
            }
            Payload::Stream(s) => s.next().await.transpose().map_err(Into::into),
            Payload::H2BodyStream(s) => s.data().await.transpose().map_err(Into::into),
        }
    }

    /// Get raw body(Payload).
    pub fn raw_body(self) -> Payload {
        self.body
    }

    /// Try to deserialize the response body as JSON.
    pub async fn json<T: serde::de::DeserializeOwned>(self) -> crate::Result<T> {
        let bytes = self.bytes().await?;
        let d = serde_json::from_slice(&bytes)?;
        Ok(d)
    }
}
