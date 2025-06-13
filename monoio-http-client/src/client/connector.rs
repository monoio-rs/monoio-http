use std::{
    fmt::{Debug, Display},
    future::Future,
    hash::Hash,
    io,
    net::ToSocketAddrs,
    path::Path,
};

use http::Version;
use monoio::{
    io::{AsyncReadRent, AsyncWriteRent, Split},
    net::{TcpStream, UnixStream},
};
use monoio_http::h1::codec::ClientCodec;

use super::{
    connection::HttpConnection, key::HttpVersion, pool::{ConnectionPool, PooledConnection}, ClientGlobalConfig, ConnectionConfig, Proto
};

#[cfg(not(feature = "native-tls"))]
pub type TlsStream<C> = monoio_rustls::ClientTlsStream<C>;

#[cfg(feature = "native-tls")]
pub type TlsStream<C> = monoio_native_tls::TlsStream<C>;

pub trait Connector<K> {
    type Connection;
    type Error;

    fn connect(&self, key: K) -> impl Future<Output = Result<Self::Connection, Self::Error>>;
}

#[derive(Default, Clone, Debug)]
pub struct TcpConnector;

impl<T> Connector<T> for TcpConnector
where
    T: ToSocketAddrs,
{
    type Connection = TcpStream;
    type Error = io::Error;

    async fn connect(&self, key: T) -> Result<Self::Connection, Self::Error> {
        TcpStream::connect(key).await.inspect(|io| {
            // we will ignore the set nodelay error
            let _ = io.set_nodelay(true);
        })
    }
}

#[derive(Default, Clone, Debug)]
pub struct UnixConnector;

impl<P> Connector<P> for UnixConnector
where
    P: AsRef<Path>,
{
    type Connection = UnixStream;
    type Error = io::Error;

    async fn connect(&self, key: P) -> Result<Self::Connection, Self::Error> {
        UnixStream::connect(key).await
    }
}

#[derive(Clone)]
pub struct TlsConnector<C> {
    inner_connector: C,
    #[cfg(not(feature = "native-tls"))]
    tls_connector: monoio_rustls::TlsConnector,
    #[cfg(feature = "native-tls")]
    tls_connector: monoio_native_tls::TlsConnector,
}

impl<C: Debug> std::fmt::Debug for TlsConnector<C> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "TlsConnector, inner: {:?}", self.inner_connector)
    }
}

impl <C:Default> TlsConnector<C>{
    pub fn new(c_config: &ConnectionConfig) -> Self{
        let mut root_store = rustls::RootCertStore::empty();
        root_store.add_trust_anchors(webpki_roots::TLS_SERVER_ROOTS.iter().map(|ta| {
            rustls::OwnedTrustAnchor::from_subject_spki_name_constraints(
                ta.subject,
                ta.spki,
                ta.name_constraints,
            )
        }));

        let mut cfg = rustls::ClientConfig::builder()
            .with_safe_defaults()
            .with_root_certificates(root_store)
            .with_no_client_auth();
        if c_config.proto == Proto::Http2{
            cfg.alpn_protocols = vec![b"h2".to_vec()];
        }
        Self {
            inner_connector: Default::default(),
            tls_connector: cfg.into(),
        }
    }
    #[cfg(feature = "native-tls")]
    fn new() -> Self {
        Self {
            inner_connector: Default::default(),
            tls_connector: native_tls::TlsConnector::builder().build().unwrap().into(),
        }
    }
} 

impl <C:Default> Default for TlsConnector<C>{
    fn default() -> Self {
        let mut root_store = rustls::RootCertStore::empty();
        root_store.add_trust_anchors(webpki_roots::TLS_SERVER_ROOTS.iter().map(|ta| {
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
            inner_connector: Default::default(),
            tls_connector: cfg.into(),
        }
    }
    #[cfg(feature = "native-tls")]
    fn default() -> Self {
        Self {
            inner_connector: Default::default(),
            tls_connector: native_tls::TlsConnector::builder().build().unwrap().into(),
        }
    }
}

#[cfg(not(feature = "native-tls"))]
impl<C, T> Connector<T> for TlsConnector<C>
where
    T: service_async::Param<super::key::ServerName>,
    C: Connector<T, Error = std::io::Error>,
    C::Connection: AsyncReadRent + AsyncWriteRent,
{
    type Connection = TlsStream<C::Connection>;
    type Error = monoio_rustls::TlsError;

    async fn connect(&self, key: T) -> Result<Self::Connection, Self::Error> {
        let server_name = key.param();

        let stream = self.inner_connector.connect(key).await?;
        let tls_stream = self.tls_connector.connect(server_name, stream).await?;
        Ok(tls_stream)
    }
}

#[cfg(feature = "native-tls")]
impl<C, T> Connector<T> for TlsConnector<C>
where
    T: service_async::Param<super::key::ServerName>,
    C: Connector<T, Error = std::io::Error>,
    C::Connection: AsyncReadRent + AsyncWriteRent,
{
    type Connection = TlsStream<C::Connection>;
    type Error = monoio_native_tls::TlsError;

    async fn connect(&self, key: T) -> Result<Self::Connection, Self::Error> {
        let server_name = key.param();

        let stream = self.inner_connector.connect(key).await?;
        self.tls_connector.connect(&server_name.0, stream).await
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

    pub async fn connect<IO>(&self, io: IO, version: Version) -> crate::Result<HttpConnection<IO>>
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
            Version::HTTP_11 => Ok(HttpConnection::H1(ClientCodec::new(io))),
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
pub struct PooledConnector<TC, K, IO: AsyncWriteRent> {
    global_config: ClientGlobalConfig,
    transport_connector: TC,
    http_connector: HttpConnector,
    pool: ConnectionPool<K, IO>,
}

impl<TC: Clone, K, IO: AsyncWriteRent> Clone for PooledConnector<TC, K, IO> {
    fn clone(&self) -> Self {
        Self {
            global_config: self.global_config.clone(),
            transport_connector: self.transport_connector.clone(),
            http_connector: self.http_connector.clone(),
            pool: self.pool.clone(),
        }
    }
}

impl<TC, K, IO: AsyncWriteRent> std::fmt::Debug for PooledConnector<TC, K, IO> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "PooledConnector")
    }
}

impl<TC, K: 'static, IO: AsyncWriteRent + 'static> PooledConnector<TC, K, IO>
where TC : From<ConnectionConfig>
{
    pub fn new_default(global_config: ClientGlobalConfig, c_config: ConnectionConfig) -> Self {
        Self {
            global_config,
            transport_connector: TC::from(c_config.clone()),
            http_connector: HttpConnector::new(c_config),
            pool: ConnectionPool::default(),
        }
    }
}

impl<TC, K: 'static, IO: AsyncWriteRent + 'static> PooledConnector<TC, K, IO> {
    pub fn new(
        global_config: ClientGlobalConfig,
        c_config: ConnectionConfig,
        connector: TC,
    ) -> Self {
        Self {
            global_config,
            transport_connector: connector,
            http_connector: HttpConnector::new(c_config),
            pool: ConnectionPool::default(),
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

    async fn connect(&self, key: K) -> Result<Self::Connection, Self::Error> {
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
