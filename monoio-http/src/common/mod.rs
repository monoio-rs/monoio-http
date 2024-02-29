use bytes::Bytes;
pub mod body;
pub mod error;
pub mod ext;
pub mod parsed_request;
pub mod parsed_response;
pub mod request;
pub mod response;

#[macro_use]
pub(crate) mod macros;

pub(crate) mod waker;

pub trait FromParts<P, B = Bytes> {
    fn from_parts(parts: P, body: B) -> Self;
}

pub trait IntoParts {
    type Parts;
    type Body;
    fn into_parts(self) -> (Self::Parts, Self::Body);
}

pub trait BorrowHeaderMap {
    fn header_map(&self) -> &http::HeaderMap;
}

impl<T: BorrowHeaderMap> BorrowHeaderMap for &T {
    #[inline]
    fn header_map(&self) -> &http::HeaderMap {
        (**self).header_map()
    }
}

impl BorrowHeaderMap for http::HeaderMap<http::HeaderValue> {
    #[inline]
    fn header_map(&self) -> &http::HeaderMap {
        self
    }
}
