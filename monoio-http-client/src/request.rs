use http::{header::HeaderName, request::Builder, HeaderValue, Method, Uri};
use monoio_http::h1::payload::Payload;

use crate::{client::Client, response::ClientResponse};

pub struct ClientRequest {
    client: Client,
    builder: Builder,
}

impl ClientRequest {
    pub fn new(client: Client) -> Self {
        Self {
            client,
            builder: Builder::new(),
        }
    }

    pub fn method<T>(mut self, method: T) -> Self
    where
        Method: TryFrom<T>,
        <Method as TryFrom<T>>::Error: Into<http::Error>,
    {
        self.builder = self.builder.method(method);
        self
    }

    pub fn uri<T>(mut self, uri: T) -> Self
    where
        Uri: TryFrom<T>,
        <Uri as TryFrom<T>>::Error: Into<http::Error>,
    {
        self.builder = self.builder.uri(uri);
        self
    }

    pub fn header<K, V>(mut self, key: K, value: V) -> Self
    where
        HeaderName: TryFrom<K>,
        <HeaderName as TryFrom<K>>::Error: Into<http::Error>,
        HeaderValue: TryFrom<V>,
        <HeaderValue as TryFrom<V>>::Error: Into<http::Error>,
    {
        self.builder = self.builder.header(key, value);
        self
    }

    // TODO: Response -> ClientResponse?
    // TODO: error handling
    pub async fn send(self) -> Result<ClientResponse, ()> {
        let resp = self
            .client
            .send(self.builder.body(Payload::None).unwrap())
            .await
            .unwrap();
        Ok(ClientResponse::new(resp))
    }
}
