pub mod connection;
pub mod connector;
pub mod key;
pub mod pool;
pub mod unified;

use std::rc::Rc;

use bytes::Bytes;
use http::HeaderMap;
use monoio_http::common::{
    body::{Body, HttpBody},
    error::HttpError,
    request::Request,
    response::Response,
};

use self::{
    connector::{Connector, PooledConnector},
    key::Key,
    pool::PooledConnection,
    unified::{UnifiedTransportConnection, UnifiedTransportConnector},
};
use crate::request::ClientRequest;

#[derive(Debug)]
pub struct ClientInner<C> {
    cfg: ClientConfig,
    connector: PooledConnector<C, Key, UnifiedTransportConnection>,
}

pub struct Client<C = UnifiedTransportConnector> {
    shared: Rc<ClientInner<C>>,
}

// Impl Clone manually because C does not have to be Clone
impl<C> Clone for Client<C> {
    fn clone(&self) -> Self {
        Self {
            shared: self.shared.clone(),
        }
    }
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

    pub fn http1_client(mut self) -> Self {
        self.connection_config.proto = Proto::Http1;
        self
    }

    pub fn http2_client(mut self) -> Self {
        self.connection_config.proto = Proto::Http2;
        self
    }

    pub fn http_auto(mut self) -> Self {
        self.connection_config.proto = Proto::Auto;
        self
    }

    pub fn max_idle_connections(mut self, conns: usize) -> Self {
        self.global_config.max_idle_connections = conns;
        self
    }

    pub fn http2_max_frame_size(mut self, sz: u32) -> Self {
        self.connection_config.h2_builder.max_frame_size(sz);
        self
    }

    pub fn http2_max_send_buf_size(mut self, sz: usize) -> Self {
        self.connection_config.h2_builder.max_send_buffer_size(sz);
        self
    }

    pub fn http2_max_concurrent_reset_streams(mut self, max: usize) -> Self {
        self.connection_config.h2_builder.max_send_buffer_size(max);
        self
    }

    pub fn http2_initial_stream_window_size(mut self, size: u32) -> Self {
        self.connection_config.h2_builder.initial_window_size(size);
        self
    }

    pub fn http2_initial_connection_window_size(mut self, size: u32) -> Self {
        self.connection_config
            .h2_builder
            .initial_connection_window_size(size);
        self
    }

    pub fn http2_max_concurrent_streams(mut self, max: u32) -> Self {
        self.connection_config
            .h2_builder
            .max_concurrent_streams(max);
        self
    }

    pub fn build(self) -> Client {
        Client::new(self.global_config, self.connection_config)
    }

    pub fn build_with_connector<C>(self, connector: C) -> Client<C> {
        let shared = Rc::new(ClientInner {
            cfg: ClientConfig::default(),
            connector: PooledConnector::new(self.global_config, self.connection_config, connector),
        });
        Client { shared }
    }
}

#[derive(Default, Clone, Debug)]
pub struct ClientConfig {
    default_headers: Rc<HeaderMap>,
}

impl Default for Client {
    fn default() -> Self {
        Builder::default().build()
    }
}

impl Client {
    fn new(g_config: ClientGlobalConfig, c_config: ConnectionConfig) -> Client {
        let shared = Rc::new(ClientInner {
            cfg: ClientConfig::default(),
            connector: PooledConnector::new_default(g_config, c_config),
        });
        Client { shared }
    }

    pub fn builder() -> Builder {
        Builder::new()
    }
}

macro_rules! http_method {
    ($fn: ident, $method: expr) => {
        pub fn $fn<U>(&self, uri: U) -> ClientRequest<C>
        where
            http::Uri: TryFrom<U>,
            <http::Uri as TryFrom<U>>::Error: Into<http::Error>,
        {
            self.request($method, uri)
        }
    };
}

impl<C> Client<C> {
    http_method!(get, http::Method::GET);
    http_method!(post, http::Method::POST);
    http_method!(put, http::Method::PUT);
    http_method!(patch, http::Method::PATCH);
    http_method!(delete, http::Method::DELETE);
    http_method!(head, http::Method::HEAD);

    pub fn request<M, U>(&self, method: M, uri: U) -> ClientRequest<C>
    where
        http::Method: TryFrom<M>,
        <http::Method as TryFrom<M>>::Error: Into<http::Error>,
        http::Uri: TryFrom<U>,
        <http::Uri as TryFrom<U>>::Error: Into<http::Error>,
    {
        let mut req = ClientRequest::<C>::new(self.clone())
            .method(method)
            .uri(uri);
        for (key, value) in self.shared.cfg.default_headers.iter() {
            req = req.header(key, value);
        }
        req
    }

    pub async fn send_request<B: Body<Data = Bytes, Error = HttpError> + 'static>(
        &self,
        req: Request<B>,
    ) -> crate::Result<Response<HttpBody>>
    where
        PooledConnector<C, Key, UnifiedTransportConnection>: Connector<
            Key,
            Connection = PooledConnection<Key, UnifiedTransportConnection>,
            Error = crate::Error,
        >,
    {
        let mut key: Key = req.uri().try_into()?;
        key.version = req.version();
        let conn = self.shared.connector.connect(key).await?;
        conn.send_request(req).await
    }
}
