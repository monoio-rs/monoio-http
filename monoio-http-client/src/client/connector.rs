use std::{
    fmt::{Debug, Display},
    future::Future,
    hash::Hash,
    io,
    marker::PhantomData,
    net::ToSocketAddrs,
};

use bytes::Bytes;
use http::Version;
use monoio::{
    io::{AsyncReadRent, AsyncWriteRent, Split},
    net::TcpStream,
};
use monoio_http::{
    h1::codec::ClientCodec,
};

use super::{
    connection::{HttpConnection},
    key::HttpVersion,
    pool::{ConnectionPool, PooledConnection},
    ClientGlobalConfig, ConnectionConfig, Proto,
};
#[cfg(feature = "rustls")]
pub type TlsStream = monoio_rustls::ClientTlsStream<TcpStream>;

#[cfg(all(feature = "native-tls", not(feature = "rustls")))]
pub type TlsStream = monoio_native_tls::TlsStream<TcpStream>;

pub type DefaultTcpConnector<T> = PooledConnector<TcpConnector, T, TcpStream>;
#[cfg(any(feature = "rustls", feature = "native-tls"))]
pub type DefaultTlsConnector<T> = PooledConnector<TlsConnector, T, TlsStream>;

pub trait Connector<K> {
    type Connection;
    type Error;
    type ConnectionFuture<'a>: Future<Output = Result<Self::Connection, Self::Error>>
    where
        Self: 'a,
        K: 'a;
    fn connect(&self, key: K) -> Self::ConnectionFuture<'_>;
}

#[derive(Default, Clone, Debug)]
pub struct TcpConnector;

impl<T> Connector<T> for TcpConnector
where
    T: ToSocketAddrs,
{
    type Connection = TcpStream;
    type Error = io::Error;
    type ConnectionFuture<'a> = impl Future<Output = Result<Self::Connection, Self::Error>> + 'a where T: 'a;

    fn connect(&self, key: T) -> Self::ConnectionFuture<'_> {
        async move { TcpStream::connect(key).await }
    }
}

#[cfg(any(feature = "rustls", feature = "native-tls"))]
#[derive(Clone)]
pub struct TlsConnector {
    tcp_connector: TcpConnector,
    #[cfg(feature = "rustls")]
    tls_connector: monoio_rustls::TlsConnector,
    #[cfg(all(feature = "native-tls", not(feature = "rustls")))]
    tls_connector: monoio_native_tls::TlsConnector,
}

#[cfg(any(feature = "rustls", feature = "native-tls"))]
impl Default for TlsConnector {
    #[cfg(feature = "rustls")]
    fn default() -> Self {
        let mut root_store = rustls::RootCertStore::empty();
        root_store.add_server_trust_anchors(webpki_roots::TLS_SERVER_ROOTS.0.iter().map(|ta| {
            rustls::OwnedTrustAnchor::from_subject_spki_name_constraints(
                ta.subject,
                ta.spki,
                ta.name_constraints,
            )
        }));

        let cfg = rustls::ClientConfig::builder()
            .with_safe_defaults()
            .with_root_certificates(root_store)
            .with_no_client_auth();

        Self {
            tcp_connector: TcpConnector,
            tls_connector: cfg.into(),
        }
    }

    #[cfg(all(feature = "native-tls", not(feature = "rustls")))]
    fn default() -> Self {
        Self {
            tcp_connector: TcpConnector,
            tls_connector: native_tls::TlsConnector::builder().build().unwrap().into(),
        }
    }
}

#[cfg(feature = "rustls")]
impl<T> Connector<T> for TlsConnector
where
    T: ToSocketAddrs + service_async::Param<rustls::ServerName>,
{
    type Connection = TlsStream;
    type Error = monoio_rustls::TlsError;
    type ConnectionFuture<'a> = impl Future<Output = Result<Self::Connection, Self::Error>> + 'a where T: 'a;

    fn connect(&self, key: T) -> Self::ConnectionFuture<'_> {
        let server_name = key.param();
        async move {
            let stream = self.tcp_connector.connect(key).await?;
            let tls_stream = self.tls_connector.connect(server_name, stream).await?;
            Ok(tls_stream)
        }
    }
}

#[cfg(all(feature = "native-tls", not(feature = "rustls")))]
impl<T> Connector<T> for TlsConnector
where
    T: ToSocketAddrs + service_async::Param<String>,
{
    type Connection = TlsStream;
    type Error = monoio_native_tls::TlsError;
    type ConnectionFuture<'a> = impl Future<Output = Result<Self::Connection, Self::Error>> + 'a where T: 'a;

    fn connect(&self, key: T) -> Self::ConnectionFuture<'_> {
        let server_name = key.param();
        async move {
            let stream = self.tcp_connector.connect(key).await?;
            let tls_stream = self.tls_connector.connect(&server_name, stream).await?;
            Ok(tls_stream)
        }
    }
}

#[derive(Clone)]
pub struct HttpConnector {
    conn_config: ConnectionConfig,
}

impl HttpConnector {
    pub fn new(conn_config: ConnectionConfig) -> Self {
        Self { conn_config }
    }

    pub async fn connect<IO>(
        &self,
        io: IO,
        version: Version,
    ) -> crate::Result<HttpConnection<IO>>
    where
        IO: AsyncReadRent + AsyncWriteRent + Split + Unpin + 'static,
    {
        let proto = if self.conn_config.proto == Proto::Auto {
            version // Use version from the header
        } else {
            match self.conn_config.proto {
                Proto::Http1 => Version::HTTP_11,
                Proto::Http2 => Version::HTTP_2,
                Proto::Auto => unreachable!(),
            }
        };

        match proto {
            Version::HTTP_11 => {
                Ok(HttpConnection::H1(Some(ClientCodec::new(io))))
            }
            Version::HTTP_2 => {
                let (send_request, h2_conn) = self.conn_config.h2_builder.handshake(io).await?;
                monoio::spawn(async move {
                    if let Err(e) = h2_conn.await {
                        println!("H2 CONN ERR={:?}", e);
                    }
                });
               Ok(HttpConnection::H2(send_request))
            }
            _ => {
                unreachable!()
            }
        }
    }
}

/// PooledConnector does 2 things:
/// 1. pool
/// 2. combine connection with codec(of cause with buffer)
pub struct PooledConnector<TC, K, IO>
where
    K: Hash + Eq + Display,
    IO: AsyncWriteRent+ AsyncReadRent + Split
{
    global_config: ClientGlobalConfig,
    transport_connector: TC,
    http_connector: HttpConnector,
    pool: ConnectionPool<K, IO>,
    _phantom: PhantomData<IO>,
}

impl<C, K, IO> Clone for PooledConnector<C, K, IO>
where
    K: Hash + Eq + Display,
    IO: AsyncReadRent + AsyncWriteRent + Split,
    C: Clone,
{
    fn clone(&self) -> Self {
        Self {
            global_config: self.global_config.clone(),
            transport_connector: self.transport_connector.clone(),
            http_connector: self.http_connector.clone(),
            pool: self.pool.clone(),
            _phantom: PhantomData,
        }
    }
}

impl<TC, K, IO> PooledConnector<TC, K, IO>
where
    TC: Default,
    K: Hash + Eq + Display + 'static,
    IO: AsyncReadRent + AsyncWriteRent + Split +'static,
{
    pub fn new(global_config: ClientGlobalConfig, c_config: ConnectionConfig) -> Self {
        Self {
            global_config,
            transport_connector: Default::default(),
            http_connector: HttpConnector::new(c_config),
            pool: Default::default(),
            _phantom: PhantomData,
        }
    }
}

impl<TC, K, IO> Connector<K> for PooledConnector<TC, K, IO>
where
    K: ToSocketAddrs + Hash + Eq + ToOwned<Owned = K> + Display + HttpVersion + 'static,
    TC: Connector<K, Connection = IO>,
    IO: AsyncReadRent + AsyncWriteRent + Split + Unpin + 'static,
    crate::Error: From<<TC as Connector<K>>::Error>,
{
    type Connection = PooledConnection<K, IO>;
    type Error = crate::Error;
    type ConnectionFuture<'a> = impl Future<Output = Result<Self::Connection, Self::Error>> + 'a where Self: 'a;

    fn connect(&self, key: K) -> Self::ConnectionFuture<'_> {
        async move {
            if let Some(conn) = self.pool.get(&key) {
                return Ok(conn);
            }
            let key_owned = key.to_owned();
            let io = self.transport_connector.connect(key).await?;

            let pipe = self
                .http_connector
                .connect(io, key_owned.get_version())
                .await?;
            Ok(self.pool.link(key_owned, pipe))
        }
    }
}
