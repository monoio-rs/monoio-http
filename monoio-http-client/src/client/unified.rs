use std::{
    future::Future,
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

use super::connector::{TcpConnector, TlsConnector, TlsStream, UnixConnector};
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
impl<'a> ToSocketAddrs for TcpTlsAddr<'a> {
    type Iter = <(&'static str, u16) as ToSocketAddrs>::Iter;
    fn to_socket_addrs(&self) -> io::Result<Self::Iter> {
        (self.0.as_str(), self.1).to_socket_addrs()
    }
}
impl<'a> service_async::Param<super::key::ServerName> for TcpTlsAddr<'a> {
    fn param(&self) -> super::key::ServerName {
        self.2.clone()
    }
}
impl<'a> AsRef<Path> for UnixTlsAddr<'a> {
    fn as_ref(&self) -> &Path {
        self.0
    }
}
impl<'a> service_async::Param<super::key::ServerName> for UnixTlsAddr<'a> {
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

    type ConnectionFuture<'a> = impl Future<Output = Result<Self::Connection, Self::Error>> + 'a where T: 'a;

    fn connect(&self, key: T) -> Self::ConnectionFuture<'_> {
        async move {
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
}

impl AsyncReadRent for UnifiedTransportConnection {
    type ReadFuture<'a, T> = impl Future<Output = BufResult<usize, T>> + 'a
    where
        Self: 'a,
        T: IoBufMut + 'a;

    type ReadvFuture<'a, T> = impl Future<Output = BufResult<usize, T>> + 'a
    where
        Self: 'a,
        T: IoVecBufMut + 'a;

    fn read<T: IoBufMut>(&mut self, buf: T) -> Self::ReadFuture<'_, T> {
        async move {
            match self {
                UnifiedTransportConnection::Tcp(s) => s.read(buf).await,
                UnifiedTransportConnection::Unix(s) => s.read(buf).await,
                UnifiedTransportConnection::TcpTls(s) => s.read(buf).await,
                UnifiedTransportConnection::UnixTls(s) => s.read(buf).await,
            }
        }
    }

    fn readv<T: IoVecBufMut>(&mut self, buf: T) -> Self::ReadvFuture<'_, T> {
        async move {
            match self {
                UnifiedTransportConnection::Tcp(s) => s.readv(buf).await,
                UnifiedTransportConnection::Unix(s) => s.readv(buf).await,
                UnifiedTransportConnection::TcpTls(s) => s.readv(buf).await,
                UnifiedTransportConnection::UnixTls(s) => s.readv(buf).await,
            }
        }
    }
}

impl AsyncWriteRent for UnifiedTransportConnection {
    type WriteFuture<'a, T> = impl Future<Output = BufResult<usize, T>> + 'a
    where
        Self: 'a,
        T: IoBuf + 'a;

    type WritevFuture<'a, T> = impl Future<Output = BufResult<usize, T>> + 'a
    where
        Self: 'a,
        T: IoVecBuf + 'a;

    type FlushFuture<'a> = impl Future<Output = io::Result<()>> + 'a
    where
        Self: 'a;

    type ShutdownFuture<'a> = impl Future<Output = io::Result<()>> + 'a
    where
        Self: 'a;

    fn write<T: IoBuf>(&mut self, buf: T) -> Self::WriteFuture<'_, T> {
        async move {
            match self {
                UnifiedTransportConnection::Tcp(s) => s.write(buf).await,
                UnifiedTransportConnection::Unix(s) => s.write(buf).await,
                UnifiedTransportConnection::TcpTls(s) => s.write(buf).await,
                UnifiedTransportConnection::UnixTls(s) => s.write(buf).await,
            }
        }
    }

    fn writev<T: IoVecBuf>(&mut self, buf: T) -> Self::WritevFuture<'_, T> {
        async move {
            match self {
                UnifiedTransportConnection::Tcp(s) => s.writev(buf).await,
                UnifiedTransportConnection::Unix(s) => s.writev(buf).await,
                UnifiedTransportConnection::TcpTls(s) => s.writev(buf).await,
                UnifiedTransportConnection::UnixTls(s) => s.writev(buf).await,
            }
        }
    }

    fn flush(&mut self) -> Self::FlushFuture<'_> {
        async move {
            match self {
                UnifiedTransportConnection::Tcp(s) => s.flush().await,
                UnifiedTransportConnection::Unix(s) => s.flush().await,
                UnifiedTransportConnection::TcpTls(s) => s.flush().await,
                UnifiedTransportConnection::UnixTls(s) => s.flush().await,
            }
        }
    }

    fn shutdown(&mut self) -> Self::ShutdownFuture<'_> {
        async move {
            match self {
                UnifiedTransportConnection::Tcp(s) => s.shutdown().await,
                UnifiedTransportConnection::Unix(s) => s.shutdown().await,
                UnifiedTransportConnection::TcpTls(s) => s.shutdown().await,
                UnifiedTransportConnection::UnixTls(s) => s.shutdown().await,
            }
        }
    }
}

unsafe impl Split for UnifiedTransportConnection {}
