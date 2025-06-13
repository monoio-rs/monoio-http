use std::{
    io,
    net::ToSocketAddrs,
    path::{Path, PathBuf},
};

use monoio::{
    buf::{IoBuf, IoBufMut, IoVecBuf, IoVecBufMut},
    io::{AsyncReadRent, AsyncWriteRent, Split},
    net::{TcpStream, UnixStream},
    BufResult,
};
use service_async::Param;
use smol_str::SmolStr;

use super::{connector::{TcpConnector, TlsConnector, TlsStream, UnixConnector}, ConnectionConfig};
use crate::Connector;

// TODO: make its PathBuf and SmolStr to ref
#[derive(Clone)]
pub enum UnifiedTransportAddr {
    Tcp(SmolStr, u16),
    Unix(PathBuf),
    TcpTls(SmolStr, u16, super::key::ServerName),
    UnixTls(PathBuf, super::key::ServerName),
}

struct TcpTlsAddr<'a>(&'a SmolStr, u16, &'a super::key::ServerName);
struct UnixTlsAddr<'a>(&'a PathBuf, &'a super::key::ServerName);
impl ToSocketAddrs for TcpTlsAddr<'_> {
    type Iter = <(&'static str, u16) as ToSocketAddrs>::Iter;
    fn to_socket_addrs(&self) -> io::Result<Self::Iter> {
        (self.0.as_str(), self.1).to_socket_addrs()
    }
}
impl service_async::Param<super::key::ServerName> for TcpTlsAddr<'_> {
    fn param(&self) -> super::key::ServerName {
        self.2.clone()
    }
}
impl AsRef<Path> for UnixTlsAddr<'_> {
    fn as_ref(&self) -> &Path {
        self.0
    }
}
impl service_async::Param<super::key::ServerName> for UnixTlsAddr<'_> {
    fn param(&self) -> super::key::ServerName {
        self.1.clone()
    }
}

#[derive(Default, Clone, Debug)]
pub struct UnifiedTransportConnector {
    raw_tcp: TcpConnector,
    raw_unix: UnixConnector,
    tcp_tls: TlsConnector<TcpConnector>,
    unix_tls: TlsConnector<UnixConnector>,
}

impl From<ConnectionConfig> for UnifiedTransportConnector{
    fn from(config: ConnectionConfig) -> Self {
        UnifiedTransportConnector{
            tcp_tls: TlsConnector::<TcpConnector>::new(&config),
            unix_tls: TlsConnector::<UnixConnector>::new(&config),
            raw_tcp: Default::default(),
            raw_unix: Default::default()
        }
    }
}


pub enum UnifiedTransportConnection {
    Tcp(TcpStream),
    Unix(UnixStream),
    TcpTls(TlsStream<TcpStream>),
    UnixTls(TlsStream<UnixStream>),
    // TODO
    // Custom(Box<dyn ...>)
}

impl std::fmt::Debug for UnifiedTransportConnection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Tcp(_) => write!(f, "Tcp"),
            Self::Unix(_) => write!(f, "Unix"),
            Self::TcpTls(_) => write!(f, "TcpTls"),
            Self::UnixTls(_) => write!(f, "UnixTls"),
        }
    }
}

impl<T> Connector<T> for UnifiedTransportConnector
where
    T: Param<UnifiedTransportAddr>,
{
    type Connection = UnifiedTransportConnection;
    type Error = crate::Error;

    async fn connect(&self, key: T) -> Result<Self::Connection, Self::Error> {
        let unified_addr = key.param();
        match &unified_addr {
            UnifiedTransportAddr::Tcp(addr, port) => self
                .raw_tcp
                .connect((addr.as_str(), *port))
                .await
                .map_err(Into::into)
                .map(UnifiedTransportConnection::Tcp),
            UnifiedTransportAddr::Unix(path) => self
                .raw_unix
                .connect(path)
                .await
                .map_err(Into::into)
                .map(UnifiedTransportConnection::Unix),
            UnifiedTransportAddr::TcpTls(addr, port, tls) => self
                .tcp_tls
                .connect(TcpTlsAddr(addr, *port, tls))
                .await
                .map_err(Into::into)
                .map(UnifiedTransportConnection::TcpTls),
            UnifiedTransportAddr::UnixTls(path, tls) => self
                .unix_tls
                .connect(UnixTlsAddr(path, tls))
                .await
                .map_err(Into::into)
                .map(UnifiedTransportConnection::UnixTls),
        }
    }
}

impl AsyncReadRent for UnifiedTransportConnection {
    async fn read<T: IoBufMut>(&mut self, buf: T) -> BufResult<usize, T> {
        match self {
            UnifiedTransportConnection::Tcp(s) => s.read(buf).await,
            UnifiedTransportConnection::Unix(s) => s.read(buf).await,
            UnifiedTransportConnection::TcpTls(s) => s.read(buf).await,
            UnifiedTransportConnection::UnixTls(s) => s.read(buf).await,
        }
    }

    async fn readv<T: IoVecBufMut>(&mut self, buf: T) -> BufResult<usize, T> {
        match self {
            UnifiedTransportConnection::Tcp(s) => s.readv(buf).await,
            UnifiedTransportConnection::Unix(s) => s.readv(buf).await,
            UnifiedTransportConnection::TcpTls(s) => s.readv(buf).await,
            UnifiedTransportConnection::UnixTls(s) => s.readv(buf).await,
        }
    }
}

impl AsyncWriteRent for UnifiedTransportConnection {
    async fn write<T: IoBuf>(&mut self, buf: T) -> BufResult<usize, T> {
        match self {
            UnifiedTransportConnection::Tcp(s) => s.write(buf).await,
            UnifiedTransportConnection::Unix(s) => s.write(buf).await,
            UnifiedTransportConnection::TcpTls(s) => s.write(buf).await,
            UnifiedTransportConnection::UnixTls(s) => s.write(buf).await,
        }
    }

    async fn writev<T: IoVecBuf>(&mut self, buf: T) -> BufResult<usize, T> {
        match self {
            UnifiedTransportConnection::Tcp(s) => s.writev(buf).await,
            UnifiedTransportConnection::Unix(s) => s.writev(buf).await,
            UnifiedTransportConnection::TcpTls(s) => s.writev(buf).await,
            UnifiedTransportConnection::UnixTls(s) => s.writev(buf).await,
        }
    }

    async fn flush(&mut self) -> io::Result<()> {
        match self {
            UnifiedTransportConnection::Tcp(s) => s.flush().await,
            UnifiedTransportConnection::Unix(s) => s.flush().await,
            UnifiedTransportConnection::TcpTls(s) => s.flush().await,
            UnifiedTransportConnection::UnixTls(s) => s.flush().await,
        }
    }

    async fn shutdown(&mut self) -> io::Result<()> {
        match self {
            UnifiedTransportConnection::Tcp(s) => s.shutdown().await,
            UnifiedTransportConnection::Unix(s) => s.shutdown().await,
            UnifiedTransportConnection::TcpTls(s) => s.shutdown().await,
            UnifiedTransportConnection::UnixTls(s) => s.shutdown().await,
        }
    }
}

unsafe impl Split for UnifiedTransportConnection {}
