pub mod connector;
pub mod key;
pub mod pool;

use std::rc::Rc;

use http::{uri::Scheme, HeaderMap};
use monoio::io::{sink::SinkExt, stream::Stream, AsyncReadRent, AsyncWriteRent};
use monoio_http::h1::{codec::decoder::FillPayload, payload::Payload};

use self::{
    connector::{Connector, DefaultTcpConnector, DefaultTlsConnector},
    key::{FromUriError, Key},
    pool::PooledConnection,
};
use crate::{request::ClientRequest, Error};

const CONN_CLOSE: &[u8] = b"close";
// TODO: ClientBuilder
pub struct ClientInner<C, #[cfg(any(feature = "rustls", feature = "native-tls"))] CS> {
    cfg: ClientConfig,
    http_connector: C,
    #[cfg(any(feature = "rustls", feature = "native-tls"))]
    https_connector: CS,
}

pub struct Client<
    C = DefaultTcpConnector<Key>,
    #[cfg(any(feature = "rustls", feature = "native-tls"))] CS = DefaultTlsConnector<Key>,
> {
    #[cfg(any(feature = "rustls", feature = "native-tls"))]
    shared: Rc<ClientInner<C, CS>>,
    #[cfg(not(any(feature = "rustls", feature = "native-tls")))]
    shared: Rc<ClientInner<C>>,
}

macro_rules! client_clone_impl {
    ( $( $x:item )* ) => {
        #[cfg(not(any(feature = "rustls", feature = "native-tls")))]
        impl<C> Clone for Client<C> {
            $($x)*
        }

        #[cfg(any(feature = "rustls", feature = "native-tls"))]
        impl<C, CS> Clone for Client<C, CS> {
            $($x)*
        }
    };
}

client_clone_impl! {
    fn clone(&self) -> Self {
        Self {
            shared: self.shared.clone(),
        }
    }
}

#[derive(Default, Clone)]
pub struct ClientConfig {
    default_headers: Rc<HeaderMap>,
}

impl Default for Client {
    fn default() -> Self {
        Self::new()
    }
}

macro_rules! http_method {
    ($fn: ident, $method: expr) => {
        pub fn $fn<U>(&self, uri: U) -> ClientRequest<C, CS>
        where
            http::Uri: TryFrom<U>,
            <http::Uri as TryFrom<U>>::Error: Into<http::Error>,
        {
            self.request($method, uri)
        }
    };
}

impl Client {
    pub fn new() -> Self {
        let shared = Rc::new(ClientInner {
            cfg: ClientConfig::default(),
            http_connector: Default::default(),
            #[cfg(any(feature = "rustls", feature = "native-tls"))]
            https_connector: Default::default(),
        });
        Self { shared }
    }

    pub async fn send(
        &self,
        req: http::Request<Payload>,
    ) -> Result<http::Response<Payload>, crate::Error> {
        match req.uri().scheme() {
            Some(s) if s == &Scheme::HTTP => {
                Self::send_with_connector(&self.shared.http_connector, req).await
            }
            Some(s) if s == &Scheme::HTTPS => {
                Self::send_with_connector(&self.shared.https_connector, req).await
            }
            _ => Err(Error::FromUri(FromUriError::UnsupportScheme)),
        }
    }
}

macro_rules! client_impl {
    ( $( $x:item )* ) => {
        #[cfg(not(any(feature = "rustls", feature = "native-tls")))]
        impl<C> Client<C> {
            $($x)*
        }

        #[cfg(any(feature = "rustls", feature = "native-tls"))]
        impl<C, CS> Client<C, CS> {
            $($x)*
        }
    };
}

client_impl! {
    pub async fn send_with_connector<CNTR, IO>(
        connector: &CNTR,
        request: http::Request<Payload>,
    ) -> Result<http::Response<Payload>, crate::Error>
    where
        CNTR: Connector<Key, Connection = PooledConnection<Key, IO>>,
        IO: AsyncReadRent + AsyncWriteRent,
    {
        let key = request.uri().try_into()?;
        if let Ok(mut codec) = connector.connect(key).await {
            match codec.send_and_flush(request).await {
                Ok(_) => match codec.next().await {
                    Some(Ok(resp)) => {
                        if let Err(e) = codec.fill_payload().await {
                            #[cfg(feature = "logging")]
                            tracing::error!("fill payload error {:?}", e);
                            return Err(Error::Decode(e));
                        }
                        let resp: http::Response<Payload> = resp;
                        let header_value = resp.headers().get(http::header::CONNECTION);
                        let reuse_conn = match header_value {
                            Some(v) => !v.as_bytes().eq_ignore_ascii_case(CONN_CLOSE),
                            None => resp.version() != http::Version::HTTP_10,
                        };
                        codec.set_reuseable(reuse_conn);
                        Ok(resp)
                    }
                    Some(Err(e)) => {
                        #[cfg(feature = "logging")]
                        tracing::error!("decode upstream response error {:?}", e);
                        Err(Error::Decode(e))
                    }
                    None => {
                        #[cfg(feature = "logging")]
                        tracing::error!("upstream return eof");
                        codec.set_reuseable(false);
                        Err(Error::Io(std::io::Error::new(
                            std::io::ErrorKind::UnexpectedEof,
                            "unexpected eof when read response",
                        )))
                    }
                },
                Err(e) => {
                    #[cfg(feature = "logging")]
                    tracing::error!("send upstream request error {:?}", e);
                    Err(Error::Encode(e))
                }
            }
        } else {
            Err(Error::Io(std::io::Error::new(
                std::io::ErrorKind::ConnectionRefused,
                "connection established failed",
            )))
        }
    }

    http_method!(get, http::Method::GET);
    http_method!(post, http::Method::POST);
    http_method!(put, http::Method::PUT);
    http_method!(patch, http::Method::PATCH);
    http_method!(delete, http::Method::DELETE);
    http_method!(head, http::Method::HEAD);
}

#[cfg(not(any(feature = "rustls", feature = "native-tls")))]
impl<C> Client<C> {
    pub fn request<M, U>(&self, method: M, uri: U) -> ClientRequest<C>
    where
        http::Method: TryFrom<M>,
        <http::Method as TryFrom<M>>::Error: Into<http::Error>,
        http::Uri: TryFrom<U>,
        <http::Uri as TryFrom<U>>::Error: Into<http::Error>,
    {
        let mut req = ClientRequest::new(self.clone()).method(method).uri(uri);
        for (key, value) in self.shared.cfg.default_headers.iter() {
            req = req.header(key, value);
        }
        req
    }
}

#[cfg(any(feature = "rustls", feature = "native-tls"))]
impl<C, CS> Client<C, CS> {
    pub fn request<M, U>(&self, method: M, uri: U) -> ClientRequest<C, CS>
    where
        http::Method: TryFrom<M>,
        <http::Method as TryFrom<M>>::Error: Into<http::Error>,
        http::Uri: TryFrom<U>,
        <http::Uri as TryFrom<U>>::Error: Into<http::Error>,
    {
        let mut req = ClientRequest::new(self.clone()).method(method).uri(uri);
        for (key, value) in self.shared.cfg.default_headers.iter() {
            req = req.header(key, value);
        }
        req
    }
}
