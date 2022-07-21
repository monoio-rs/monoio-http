use monoio_http::h1::payload::Payload;

pub struct ClientResponse<T = Payload> {
    inner: http::Response<T>,
}

impl<T> ClientResponse<T> {
    pub fn new(inner: http::Response<T>) -> Self {
        Self { inner }
    }
}
