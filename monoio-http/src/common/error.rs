use thiserror::Error as ThisError;

use crate::h1::{
    codec::{decoder::DecodeError, encoder::EncodeError},
    payload::PayloadError,
};

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
}