use http::{HeaderMap, StatusCode, Version};

use crate::ParamRef;

#[derive(Debug, Clone)]
pub struct ResponseHead {
    pub version: Version,
    pub status: StatusCode,
    pub headers: HeaderMap,
}

pub struct Response<P> {
    pub head: ResponseHead,
    pub payload: P,
}

impl<P> From<(ResponseHead, P)> for Response<P> {
    fn from(inner: (ResponseHead, P)) -> Self {
        Self {
            head: inner.0,
            payload: inner.1,
        }
    }
}

impl ParamRef<HeaderMap> for ResponseHead {
    fn param_ref(&self) -> &HeaderMap {
        &self.headers
    }
}
