pub use http::response::{Builder as ResponseBuilder, Parts as ResponseHead};

use super::BorrowHeaderMap;
use crate::{
    common::{FromParts, IntoParts},
    h1::payload::Payload,
};

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

impl BorrowHeaderMap for ResponseHead {
    fn header_map(&self) -> &http::HeaderMap {
        &self.headers
    }

    fn header_map_mut(&mut self) -> &mut http::HeaderMap {
        &mut self.headers
    }
}
