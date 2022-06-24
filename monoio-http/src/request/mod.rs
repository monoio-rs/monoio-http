use http::{HeaderMap, Method, Uri, Version};

use crate::ParamRef;

#[derive(Debug, Clone)]
pub struct RequestHead {
    pub method: Method,
    pub uri: Uri,
    pub version: Version,
    pub headers: HeaderMap,
}

pub struct Request<P> {
    pub head: RequestHead,
    pub payload: P,
}

impl<P> From<(RequestHead, P)> for Request<P> {
    fn from(inner: (RequestHead, P)) -> Self {
        Self {
            head: inner.0,
            payload: inner.1,
        }
    }
}

impl ParamRef<HeaderMap> for RequestHead {
    fn param_ref(&self) -> &HeaderMap {
        &self.headers
    }
}
