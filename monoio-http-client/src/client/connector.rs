use std::{
    fmt::{Debug, Display},
    future::Future,
    hash::Hash,
    io,
    net::ToSocketAddrs,
    sync::Arc,
};

use monoio::net::TcpStream;
use monoio_http::{h1::codec::ClientCodec, Param};
use rustls::client::ServerCertVerifier;

use super::pool::{ConnectionPool, PooledConnection};

pub type TlsStream = monoio_rustls::ClientTlsStream<TcpStream>;

pub type DefaultTcpConnector<T> = PooledConnector<TcpConnector, T, TcpStream>;
#[cfg(feature = "tls")]
pub type DefaultTlsConnector<T> = PooledConnector<TlsConnector, T, TlsStream>;

pub trait Connector<K> {
    type Connection;
    type Error;
    type ConnectionFuture<'a>: Future<Output = Result<Self::Connection, Self::Error>>
    where
        Self: 'a;
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
    type ConnectionFuture<'a> = impl Future<Output = Result<Self::Connection, Self::Error>>;

    fn connect(&self, key: T) -> Self::ConnectionFuture<'_> {
        async move { TcpStream::connect(key).await }
    }
}

#[cfg(feature = "tls")]
#[derive(Clone)]
pub struct TlsConnector {
    tcp_connector: TcpConnector,
    tls_connector: monoio_rustls::TlsConnector,
}

struct CustomServerCertVerifier;

impl ServerCertVerifier for CustomServerCertVerifier {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls::Certificate,
        _intermediates: &[rustls::Certificate],
        _server_name: &rustls::ServerName,
        _scts: &mut dyn Iterator<Item = &[u8]>,
        _ocsp_response: &[u8],
        _now: std::time::SystemTime,
    ) -> Result<rustls::client::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::ServerCertVerified::assertion())
    }
}

#[cfg(feature = "tls")]
impl Default for TlsConnector {
    fn default() -> Self {
        let mut root_store = rustls::RootCertStore::empty();
        root_store.add_server_trust_anchors(webpki_roots::TLS_SERVER_ROOTS.0.iter().map(|ta| {
            rustls::OwnedTrustAnchor::from_subject_spki_name_constraints(
                ta.subject,
                ta.spki,
                ta.name_constraints,
            )
        }));

        // let cfg = rustls::ClientConfig::builder()
        //     .with_safe_defaults()
        //     .with_root_certificates(root_store)
        //     .with_no_client_auth();

        // allow server self-signed cert
        let cfg = rustls::ClientConfig::builder()
            .with_safe_defaults()
            .with_custom_certificate_verifier(Arc::new(CustomServerCertVerifier))
            .with_no_client_auth();

        Self {
            tcp_connector: TcpConnector,
            tls_connector: cfg.into(),
        }
    }
}

#[cfg(feature = "tls")]
impl<T> Connector<T> for TlsConnector
where
    // TODO: remove 'static
    T: ToSocketAddrs + Param<rustls::ServerName> + 'static,
{
    type Connection = TlsStream;
    type Error = monoio_rustls::TlsError;
    type ConnectionFuture<'a> = impl Future<Output = Result<Self::Connection, Self::Error>> + 'a;

    fn connect(&self, key: T) -> Self::ConnectionFuture<'_> {
        async move {
            let server_name = key.param();
            let stream = self.tcp_connector.connect(key).await?;
            let tls_stream = self.tls_connector.connect(server_name, stream).await?;
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
    T: ToSocketAddrs + Hash + Eq + ToOwned<Owned = T> + Display,
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

    #[monoio::test_all]
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
