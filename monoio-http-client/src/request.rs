use bytes::Bytes;
use http::{header::HeaderName, request::Builder, HeaderValue, Method, Uri};
use monoio_http::common::body::{FixedBody, HttpBody};

#[cfg(any(feature = "rustls", feature = "native-tls"))]
use crate::client::connector::DefaultTlsConnector;
use crate::{
    client::{connector::DefaultTcpConnector, key::Key, Client},
    response::ClientResponse,
};

#[cfg(any(feature = "rustls", feature = "native-tls"))]
pub struct ClientRequest<C = DefaultTcpConnector<Key>, CS = DefaultTlsConnector<Key>> {
    client: Client<C, CS>,
    builder: Builder,
}

#[cfg(not(any(feature = "rustls", feature = "native-tls")))]
pub struct ClientRequest<B, C = DefaultTcpConnector<Key>> {
    client: Client<C>,
    builder: Builder,
}

macro_rules! client_request_impl {
    ( $( $x:item )* ) => {
        #[cfg(not(any(feature = "rustls", feature = "native-tls")))]
        impl<C> ClientRequest<C> {
            $($x)*
        }

        #[cfg(any(feature = "rustls", feature = "native-tls"))]
        impl<C, CS> ClientRequest<C, CS> {
            $($x)*
        }
    };
}

client_request_impl! {
    pub fn new(client: Client<C, CS> ) -> Self {
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

    fn build_request(builder: Builder, body: HttpBody) -> crate::Result<http::Request<HttpBody>> {
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

impl ClientRequest {
    pub async fn send(self) -> crate::Result<ClientResponse> {
        let request = Self::build_request(self.builder, HttpBody::fixed_body(None))?;
        let resp = self.client.send_request(request).await?;
        Ok(ClientResponse::new(resp))
    }

    pub async fn send_body(self, data: Bytes) -> crate::Result<ClientResponse> {
        let request = Self::build_request(self.builder, HttpBody::fixed_body(Some(data)))?;
        let resp = self.client.send_request(request).await?;
        Ok(ClientResponse::new(resp))
    }

    pub async fn send_json<T: serde::Serialize>(self, data: &T) -> crate::Result<ClientResponse> {
        let body: Bytes = serde_json::to_vec(data)?.into();
        let builder = self.builder.header(
            http::header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
        let request = Self::build_request(builder, HttpBody::fixed_body(Some(body)))?;
        let resp = self.client.send_request(request).await?;
        Ok(ClientResponse::new(resp))
    }
}
