use bytes::Bytes;
use http::{header::HeaderName, request::Builder, HeaderValue, Method, Uri};
use monoio_http::{
    common::{
        body::{Body, FixedBody, HttpBody},
        error::HttpError,
    },
    h1::payload::FramedPayloadRecvr,
    h2::RecvStream,
};

#[cfg(any(feature = "rustls", feature = "native-tls"))]
use crate::client::connector::DefaultTlsConnector;
use crate::{
    client::{connector::DefaultTcpConnector, key::Key, Client},
    response::ClientResponse,
};

#[cfg(any(feature = "rustls", feature = "native-tls"))]
pub struct ClientRequest<B, C = DefaultTcpConnector<Key, B>, CS = DefaultTlsConnector<Key, B>> {
    client: Client<B, C, CS>,
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
        impl<B, C> ClientRequest<B, C> {
            $($x)*
        }

        #[cfg(any(feature = "rustls", feature = "native-tls"))]
        impl<B, C, CS> ClientRequest<B, C, CS> {
            $($x)*
        }
    };
}

client_request_impl! {
    pub fn new(client: Client<B, C, CS> ) -> Self {
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

    fn build_request(builder: Builder, body: B) -> crate::Result<http::Request<B>> {
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

impl<B> ClientRequest<B>
where
    B: Body<Data = Bytes, Error = HttpError>
        + From<RecvStream>
        + From<FramedPayloadRecvr>
        + 'static
        + FixedBody<BodyType = B>,
    HttpBody: From<B>,
{
    pub async fn send(self) -> crate::Result<ClientResponse<B>> {
        let request = Self::build_request(self.builder, B::fixed_body(None))?;
        let resp = self.client.send_request(request).await?;
        Ok(ClientResponse::new(resp))
    }

    pub async fn send_body(self, data: Bytes) -> crate::Result<ClientResponse<B>> {
        let request = Self::build_request(self.builder, B::fixed_body(Some(data)))?;
        let resp = self.client.send_request(request).await?;
        Ok(ClientResponse::new(resp))
    }

    pub async fn send_json<T: serde::Serialize>(
        self,
        data: &T,
    ) -> crate::Result<ClientResponse<B>> {
        let body: Bytes = serde_json::to_vec(data)?.into();
        let builder = self.builder.header(
            http::header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
        let request = Self::build_request(builder, B::fixed_body(Some(body)))?;
        let resp = self.client.send_request(request).await?;
        Ok(ClientResponse::new(resp))
    }
}
