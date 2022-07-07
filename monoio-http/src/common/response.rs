use std::borrow::Cow;

use http::{HeaderMap, StatusCode, Version};

use crate::{ParamMut, ParamRef};

use super::ReqOrResp;

#[derive(Debug, Clone)]
pub struct ResponseHead {
    pub version: Version,
    pub status: StatusCode,
    pub reason: Option<Cow<'static, str>>,
    pub headers: HeaderMap,
}

pub type Response<P> = ReqOrResp<ResponseHead, P>;

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

impl ParamMut<HeaderMap> for ResponseHead {
    fn param_mut(&mut self) -> &mut HeaderMap {
        &mut self.headers
    }
}
