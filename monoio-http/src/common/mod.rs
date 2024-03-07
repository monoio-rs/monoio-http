use std::collections::HashMap;

use bytes::Bytes;

pub mod body;
pub mod error;
pub mod ext;
pub mod multipart;
pub mod parsed_request;
pub mod parsed_response;
pub mod request;
pub mod response;

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

type QueryMap = HashMap<String, String>;

#[derive(Default, Debug, Clone, Copy)]
pub enum Parse<T> {
    Parsed(T),
    Failed,
    #[default]
    Unparsed,
}

impl<T> Parse<T> {
    #[inline]
    pub fn reset(&mut self) {
        *self = Parse::Unparsed;
    }

    #[inline]
    pub fn set(&mut self, value: T) {
        *self = Parse::Parsed(value);
    }

    #[inline]
    pub fn is_unparsed(&self) -> bool {
        matches!(self, Parse::Unparsed)
    }

    #[inline]
    pub fn is_unparsed_or_failed(&self) -> bool {
        (matches!(self, Parse::Failed) || matches!(self, Parse::Unparsed))
    }

    #[inline]
    pub fn is_parsed(&self) -> bool {
        matches!(self, Parse::Parsed(_))
    }

    #[inline]
    pub fn is_parsing_failed(&self) -> bool {
        matches!(self, Parse::Failed)
    }

    #[inline]
    pub fn parsed_inner_ref(&self) -> &T {
        match self {
            Parse::Parsed(inner) => inner,
            _ => unsafe { std::hint::unreachable_unchecked() },
        }
    }

    #[inline]
    pub fn parsed_inner_mut(&mut self) -> &mut T {
        match self {
            Parse::Parsed(inner) => inner,
            _ => unsafe { std::hint::unreachable_unchecked() },
        }
    }

    #[inline]
    pub fn parsed_inner(self) -> T {
        match self {
            Parse::Parsed(inner) => inner,
            _ => unsafe { std::hint::unreachable_unchecked() },
        }
    }
}

impl<T: Default> Parse<T> {
    #[inline]
    pub fn set_default(&mut self) {
        *self = Parse::Parsed(T::default());
    }
}
