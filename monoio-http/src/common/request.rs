use http::{HeaderMap, Method, Uri, Version};

use crate::{ParamMut, ParamRef};

use super::ReqOrResp;

#[derive(Debug, Clone)]
pub struct RequestHead {
    pub method: Method,
    pub uri: Uri,
    pub version: Version,
    pub headers: HeaderMap,
}

pub type Request<P> = ReqOrResp<RequestHead, P>;

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

impl ParamMut<HeaderMap> for RequestHead {
    fn param_mut(&mut self) -> &mut HeaderMap {
        &mut self.headers
    }
}
