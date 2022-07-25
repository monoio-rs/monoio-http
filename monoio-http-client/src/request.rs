use bytes::Bytes;
use http::{header::HeaderName, request::Builder, HeaderValue, Method, Uri};
use monoio_http::h1::payload::{fixed_payload_pair, Payload};

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

    pub async fn send(self) -> crate::Result<ClientResponse> {
        let request = Self::build_request(self.builder, Payload::None)?;
        let resp = self.client.send(request).await?;
        Ok(ClientResponse::new(resp))
    }

    pub async fn send_body(self, data: Bytes) -> crate::Result<ClientResponse> {
        let (payload, payload_sender) = fixed_payload_pair();
        payload_sender.feed(Ok(data));
        let request = Self::build_request(self.builder, Payload::Fixed(payload))?;
        let resp = self.client.send(request).await?;
        Ok(ClientResponse::new(resp))
    }

    pub async fn send_json<T: serde::Serialize>(self, data: &T) -> crate::Result<ClientResponse> {
        let body: Bytes = serde_json::to_vec(data)?.into();
        let builder = self.builder.header(
            http::header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
        let (payload, payload_sender) = fixed_payload_pair();
        payload_sender.feed(Ok(body));
        let request = Self::build_request(builder, Payload::Fixed(payload))?;
        let resp = self.client.send(request).await?;
        Ok(ClientResponse::new(resp))
    }

    fn build_request(builder: Builder, body: Payload) -> crate::Result<http::Request<Payload>> {
        let mut req = builder.version(http::Version::HTTP_11).body(body)?;
        if let Some(host) = req.uri().host() {
            let host = HeaderValue::try_from(host).map_err(http::Error::from)?;
            let headers = req.headers_mut();
            if !headers.contains_key(http::header::HOST) {
                headers.insert(http::header::HOST, host);
            }
        }
        Ok(req)
    }
}
