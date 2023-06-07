use thiserror::Error as ThisError;

#[derive(ThisError, Debug)]
pub enum Error {
    #[error("convert from uri error {0}")]
    FromUri(#[from] crate::client::key::FromUriError),
    #[error("http header error")]
    Http(#[from] http::Error),
    #[error("encode error {0}")]
    Encode(#[from] monoio_http::h1::codec::encoder::EncodeError),
    #[error("decode error {0}")]
    Decode(#[from] monoio_http::h1::codec::decoder::DecodeError),
    #[error("receive body error {0}")]
    Payload(#[from] monoio_http::h1::payload::PayloadError),

    #[error("io error {0}")]
    Io(#[from] std::io::Error),

    #[cfg(feature = "tls")]
    #[error("tls error {0}")]
    Tls(#[from] monoio_rustls::TlsError),
    #[error("serde_json error {0}")]
    Json(#[from] serde_json::Error),
    #[error("H2 RecvStream decode error {0}")]
    H2PayloadError(#[from] monoio_http::h2::Error),
}

pub type Result<T> = std::result::Result<T, Error>;
