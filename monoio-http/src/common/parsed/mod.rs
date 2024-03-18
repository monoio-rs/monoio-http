//! For lazy parse request/response and modify headers.
pub mod request;
pub mod response;

use std::collections::HashMap;

pub type QueryMap = HashMap<String, String>;

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
    pub const fn is_parsed(&self) -> bool {
        matches!(self, Parse::Parsed(..))
    }

    #[inline]
    pub const fn is_failed(&self) -> bool {
        matches!(self, Parse::Failed)
    }

    #[inline]
    pub const fn is_unparsed(&self) -> bool {
        matches!(self, Parse::Unparsed)
    }

    #[inline]
    pub const fn is_unparsed_or_failed(&self) -> bool {
        (matches!(self, Parse::Failed) || matches!(self, Parse::Unparsed))
    }

    #[inline]
    pub const fn as_ref(&self) -> Parse<&T> {
        match self {
            Parse::Parsed(inner) => Parse::Parsed(inner),
            Parse::Failed => Parse::Failed,
            Parse::Unparsed => Parse::Unparsed,
        }
    }

    #[inline]
    pub fn as_mut(&mut self) -> Parse<&mut T> {
        match self {
            Parse::Parsed(inner) => Parse::Parsed(inner),
            Parse::Failed => Parse::Failed,
            Parse::Unparsed => Parse::Unparsed,
        }
    }

    #[inline]
    pub fn unwrap(self) -> T {
        match self {
            Parse::Parsed(inner) => inner,
            _ => panic!("called `Parse::unwrap on a non-`Parsed` value"),
        }
    }

    #[inline]
    /// # Safety
    /// Caller must makes sure the value is parsed.
    pub unsafe fn unwrap_unchecked(self) -> T {
        debug_assert!(self.is_parsed());
        match self {
            Parse::Parsed(inner) => inner,
            _ => std::hint::unreachable_unchecked(),
        }
    }

    #[inline]
    pub fn parsed_inner_mut(&mut self) -> &mut T {
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
