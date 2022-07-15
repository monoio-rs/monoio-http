use http::HeaderMap;

use crate::{
    common::{FromParts, IntoParts},
    h1::payload::Payload,
    ParamMut, ParamRef,
};

pub use http::response::Builder as ResponseBuilder;
pub use http::response::Parts as ResponseHead;

pub type Response<P = Payload> = http::response::Response<P>;

impl<P> FromParts<ResponseHead, P> for Response<P> {
    fn from_parts(parts: ResponseHead, body: P) -> Self {
        Self::from_parts(parts, body)
    }
}

impl<P> IntoParts for Response<P> {
    type Parts = ResponseHead;
    type Body = P;
    fn into_parts(self) -> (Self::Parts, Self::Body) {
        self.into_parts()
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
