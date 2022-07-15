use crate::{
    common::{FromParts, IntoParts},
    h1::payload::Payload,
    ParamMut, ParamRef,
};

pub use http::request::Builder as RequestBuilder;
pub use http::request::Parts as RequestHead;

pub type Request<P = Payload> = http::request::Request<P>;

impl<P> FromParts<RequestHead, P> for Request<P> {
    fn from_parts(parts: RequestHead, body: P) -> Self {
        Self::from_parts(parts, body)
    }
}

impl<P> IntoParts for Request<P> {
    type Parts = RequestHead;
    type Body = P;
    fn into_parts(self) -> (Self::Parts, Self::Body) {
        self.into_parts()
    }
}

impl ParamRef<http::HeaderMap> for RequestHead {
    fn param_ref(&self) -> &http::HeaderMap {
        &self.headers
    }
}

impl ParamMut<http::HeaderMap> for RequestHead {
    fn param_mut(&mut self) -> &mut http::HeaderMap {
        &mut self.headers
    }
}
