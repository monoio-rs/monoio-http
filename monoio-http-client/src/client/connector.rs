use std::{fmt::Debug, future::Future, hash::Hash, io, net::ToSocketAddrs};

use monoio::net::TcpStream;
use monoio_http::h1::codec::ClientCodec;

use super::pool::{ConnectionPool, PooledConnection};

#[cfg(feature = "rustls")]
pub type TlsStream = monoio_rustls::ClientTlsStream<TcpStream>;

#[cfg(feature = "native-tls")]
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
    #[cfg(feature = "native-tls")]
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

    #[cfg(feature = "native-tls")]
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

#[cfg(feature = "native-tls")]
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

/// PooledConnector does 2 things:
/// 1. pool
/// 2. combine connection with codec(of cause with buffer)
pub struct PooledConnector<C, K: Hash + Eq, IO> {
    inner: C,
    pool: ConnectionPool<K, IO>,
}

impl<C, K: Hash + Eq, IO> Clone for PooledConnector<C, K, IO>
where
    C: Clone,
{
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            pool: self.pool.clone(),
        }
    }
}

impl<C, K: Hash + Eq + 'static, IO: 'static> Default for PooledConnector<C, K, IO>
where
    C: Default,
{
    fn default() -> Self {
        Self {
            inner: Default::default(),
            pool: Default::default(),
        }
    }
}

impl<C, T, IO> Connector<T> for PooledConnector<C, T, IO>
where
    T: ToSocketAddrs + Hash + Eq + Debug + ToOwned<Owned = T>,
    C: Connector<T, Connection = IO>,
{
    type Connection = PooledConnection<T, IO>;
    type Error = C::Error;
    type ConnectionFuture<'a> = impl Future<Output = Result<Self::Connection, Self::Error>> + 'a where Self: 'a;

    fn connect(&self, key: T) -> Self::ConnectionFuture<'_> {
        async move {
            if let Some(conn) = self.pool.get(&key) {
                return Ok(conn);
            }
            let key_owned = key.to_owned();
            let io = self.inner.connect(key).await?;
            let codec = ClientCodec::new(io);
            Ok(self.pool.link(key_owned, codec))
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use super::*;

    #[monoio::test_all(timer_enabled = true)]
    async fn connect_tcp() {
        let connector = DefaultTcpConnector::<&'static str>::default();
        let begin = Instant::now();
        let conn = connector
            .connect("captive.apple.com:80")
            .await
            .expect("unable to get connection");
        println!("First connection cost {}ms", begin.elapsed().as_millis());
        drop(conn);

        let begin = Instant::now();
        let _ = connector
            .connect("captive.apple.com:80")
            .await
            .expect("unable to get connection");
        let spent = begin.elapsed().as_millis();
        println!("Second connection cost {}ms", spent);
        assert!(
            spent <= 2,
            "second connect spend too much time, maybe not with pool?"
        );
    }
}
