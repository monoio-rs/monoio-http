use std::io::{Error, ErrorKind};

use thiserror::Error as ThisError;

use crate::h1::{
    codec::{decoder::DecodeError, encoder::EncodeError},
    payload::PayloadError,
};

#[derive(ThisError, Debug)]
pub enum ExtractError {
    #[error("Uninitialized cookie jar")]
    UninitializedCookieJar,
    #[error("http cookie parsing error {0}")]
    CookieParseError(#[from] cookie::ParseError),
    #[error("Invalid Header Value")]
    InvalidHeaderValue,
    #[error("Invalid content type")]
    InvalidContentType,
}

#[derive(ThisError, Debug)]
pub enum HttpError {
    #[error("http1 Encode error {0}")]
    H1EncodeError(#[from] EncodeError),
    #[error("http2 decode error {0}")]
    H1DecodeError(#[from] DecodeError),
    #[error("receive body error {0}")]
    PayloadError(#[from] PayloadError),
    #[error("H2 error {0}")]
    H2Error(#[from] crate::h2::Error),
    #[error("IO error {0}")]
    IOError(#[from] std::io::Error),
    #[error("Cookie error {0}")]
    CookieError(#[from] ExtractError),
    #[error("SerDe error")]
    SerDeError,
}

impl Clone for HttpError {
    fn clone(&self) -> Self {
        match self {
            Self::IOError(e) => Self::IOError(Error::new(ErrorKind::Other, e.to_string())),
            _ => self.clone(),
        }
    }
}
