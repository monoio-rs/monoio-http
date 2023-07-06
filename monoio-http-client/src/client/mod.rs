pub mod connection;
pub mod connector;
pub mod key;
pub mod pool;

use std::rc::Rc;

use bytes::Bytes;
use http::{uri::Scheme, HeaderMap};
use monoio_http::common::{
    body::{Body, HttpBody},
    error::HttpError,
    request::Request,
    response::Response,
};

use self::{
    connector::{Connector, DefaultTcpConnector, DefaultTlsConnector},
    key::Key,
};
use crate::request::ClientRequest;

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

#[derive(Default, Clone, PartialEq, Eq)]
pub enum Proto {
    #[default]
    Http1, // HTTP1_1 only client
    Http2, // HTTP2 only client
    Auto,  // Uses version header in request
}

// HTTP1 & HTTP2 Connection specific.
#[derive(Default, Clone)]
pub struct ConnectionConfig {
    pub proto: Proto,
    h2_builder: monoio_http::h2::client::Builder,
}

// Global config applicable to
// all connections maintained by client
#[derive(Default, Clone)]
pub struct ClientGlobalConfig {
    max_idle_connections: usize,
}

#[derive(Default, Clone)]
pub struct Builder {
    connection_config: ConnectionConfig,
    global_config: ClientGlobalConfig,
}

impl Builder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn http1_client(&mut self) -> &mut Self {
        self.connection_config.proto = Proto::Http1;
        self
    }

    pub fn http2_client(&mut self) -> &mut Self {
        self.connection_config.proto = Proto::Http2;
        self
    }

    pub fn http_auto(&mut self) -> &mut Self {
        self.connection_config.proto = Proto::Auto;
        self
    }

    pub fn max_idle_connections(&mut self, conns: usize) -> &mut Self {
        self.global_config.max_idle_connections = conns;
        self
    }

    pub fn http2_max_frame_size(&mut self, sz: u32) -> &mut Self {
        self.connection_config.h2_builder.max_frame_size(sz);
        self
    }

    pub fn http2_max_send_buf_size(&mut self, sz: usize) -> &mut Self {
        self.connection_config.h2_builder.max_send_buffer_size(sz);
        self
    }

    pub fn http2_max_concurrent_reset_streams(&mut self, max: usize) -> &mut Self {
        self.connection_config.h2_builder.max_send_buffer_size(max);
        self
    }

    pub fn http2_initial_stream_window_size(&mut self, size: u32) -> &mut Self {
        self.connection_config.h2_builder.initial_window_size(size);
        self
    }

    pub fn http2_initial_connection_window_size(&mut self, size: u32) -> &mut Self {
        self.connection_config
            .h2_builder
            .initial_connection_window_size(size);
        self
    }

    pub fn http2_max_concurrent_streams(&mut self, max: u32) -> &mut Self {
        self.connection_config
            .h2_builder
            .max_concurrent_streams(max);
        self
    }

    pub fn build_http1(self) -> Client {
        Client::new(self.global_config, self.connection_config)
    }

    pub fn build_http2(mut self) -> Client {
        self.http2_client();
        Client::new(self.global_config, self.connection_config)
    }

    pub fn build_auto(mut self) -> Client {
        self.http_auto();
        Client::new(self.global_config, self.connection_config)
    }
}

macro_rules! client_clone_impl {
    ( $( $x:item )* ) => {
        #[cfg(not(any(feature = "rustls", feature = "native-tls")))]
        impl<C> Clone for Client<C>
        {
            $($x)*
        }

        #[cfg(any(feature = "rustls", feature = "native-tls"))]
        impl<C, CS> Clone for Client<C, CS>
        {
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
        Builder::default().build_http1()
    }
}

impl Client {
    fn new(g_config: ClientGlobalConfig, c_config: ConnectionConfig) -> Self {
        let shared = Rc::new(ClientInner {
            cfg: ClientConfig::default(),
            http_connector: DefaultTcpConnector::new(g_config.clone(), c_config.clone()),
            #[cfg(any(feature = "rustls", feature = "native-tls"))]
            https_connector: DefaultTlsConnector::new(g_config, c_config),
        });
        Self { shared }
    }

    pub async fn send_request<B: Body<Data = Bytes, Error = HttpError> + 'static>(
        &self,
        req: Request<B>,
    ) -> crate::Result<Response<HttpBody>> {
        let mut key: Key = req.uri().try_into()?;
        key.set_version(req.version());

        match req.uri().scheme() {
            Some(s) if s == &Scheme::HTTP => {
                let conn = self.shared.http_connector.connect(key).await?;
                conn.send_request(req).await
            }
            #[cfg(any(feature = "rustls", feature = "native-tls"))]
            Some(s) if s == &Scheme::HTTPS => {
                let conn = self.shared.https_connector.connect(key).await?;
                conn.send_request(req).await
            }
            // Key creation should error first
            _ => unreachable!(),
        }
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

macro_rules! client_impl {
    ( $( $x:item )* ) => {
        #[cfg(not(any(feature = "rustls", feature = "native-tls")))]
        impl<C> Client<B, C> {
            $($x)*
        }

        #[cfg(any(feature = "rustls", feature = "native-tls"))]
        impl<C, CS> Client<C, CS> {
            $($x)*
        }
    };
}

client_impl! {
    http_method!(get, http::Method::GET);
    http_method!(post, http::Method::POST);
    http_method!(put, http::Method::PUT);
    http_method!(patch, http::Method::PATCH);
    http_method!(delete, http::Method::DELETE);
    http_method!(head, http::Method::HEAD);
}

#[cfg(not(any(feature = "rustls", feature = "native-tls")))]
impl<B: Body<Data = Bytes, Error = HttpError>, C> Client<B, C> {
    pub fn request<M, U>(&self, method: M, uri: U) -> ClientRequest<B, C>
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
        let mut req = ClientRequest::<C, CS>::new(self.clone())
            .method(method)
            .uri(uri);
        for (key, value) in self.shared.cfg.default_headers.iter() {
            req = req.header(key, value);
        }
        req
    }
}
