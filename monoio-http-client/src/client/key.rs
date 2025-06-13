use std::{convert::Infallible, hash::Hash, net::ToSocketAddrs};

use http::{Uri, Version};
use service_async::{Param, ParamMut, ParamRef};
use smol_str::SmolStr;
use thiserror::Error as ThisError;

#[cfg(feature = "native-tls")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerName(pub SmolStr);
#[cfg(not(feature = "native-tls"))]
pub use rustls::ServerName;

use super::unified::UnifiedTransportAddr;

#[cfg(feature = "native-tls")]
impl<T: Into<SmolStr>> From<T> for ServerName {
    fn from(value: T) -> Self {
        Self(value.into())
    }
}

pub struct Key {
    pub host: SmolStr,
    pub port: u16,
    pub version: Version,

    pub server_name: Option<ServerName>,
}

pub trait HttpVersion {
    fn get_version(&self) -> Version;
}

impl HttpVersion for Key {
    fn get_version(&self) -> Version {
        self.version
    }
}

impl Clone for Key {
    fn clone(&self) -> Self {
        Self {
            host: self.host.clone(),
            port: self.port,
            server_name: self.server_name.clone(),
            version: self.version,
        }
    }
}

impl std::fmt::Debug for Key {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}:{:?}", self.host, self.port, self.version)
    }
}

impl std::fmt::Display for Key {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}:{:?}", self.host, self.port, self.version)
    }
}

impl PartialEq for Key {
    fn eq(&self, other: &Self) -> bool {
        self.host == other.host && self.port == other.port && self.version == other.version
    }
}

impl Eq for Key {}

impl Hash for Key {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.host.hash(state);
        self.port.hash(state);
        self.version.hash(state);
    }
}

impl ToSocketAddrs for Key {
    type Iter = <(&'static str, u16) as ToSocketAddrs>::Iter;

    fn to_socket_addrs(&self) -> std::io::Result<Self::Iter> {
        (self.host.as_str(), self.port).to_socket_addrs()
    }
}

impl Param<Option<ServerName>> for Key {
    fn param(&self) -> Option<ServerName> {
        self.server_name.clone()
    }
}

impl ParamRef<Option<ServerName>> for Key {
    fn param_ref(&self) -> &Option<ServerName> {
        &self.server_name
    }
}

impl ParamMut<Option<ServerName>> for Key {
    fn param_mut(&mut self) -> &mut Option<ServerName> {
        &mut self.server_name
    }
}

impl Param<UnifiedTransportAddr> for Key {
    fn param(&self) -> UnifiedTransportAddr {
        if let Some(sn) = self.server_name.clone() {
            UnifiedTransportAddr::TcpTls(self.host.clone(), self.port, sn)
        } else {
            UnifiedTransportAddr::Tcp(self.host.clone(), self.port)
        }
    }
}

#[derive(ThisError, Debug)]
pub enum FromUriError {
    #[error("Invalid dns name")]
    InvalidDnsName(#[from] rustls::client::InvalidDnsNameError),
    #[error("Scheme not supported")]
    UnsupportScheme,
    #[error("Missing authority in uri")]
    NoAuthority,
}

impl From<Infallible> for FromUriError {
    fn from(_: Infallible) -> Self {
        unsafe { std::hint::unreachable_unchecked() }
    }
}

impl TryFrom<&Uri> for Key {
    type Error = FromUriError;

    fn try_from(uri: &Uri) -> Result<Self, Self::Error> {
        let (tls, default_port) = match uri.scheme() {
            Some(scheme) if scheme == &http::uri::Scheme::HTTP => (false, 80),
            Some(scheme) if scheme == &http::uri::Scheme::HTTPS => (true, 443),
            _ => (false, 0),
        };
        let port = uri.port_u16().unwrap_or(default_port);

        let host = match uri.host() {
            Some(a) => {
                if a.starts_with('[') && a.ends_with(']') {
                    &a[1..a.len() - 1]
                } else {
                    a
                }
            }
            None => return Err(FromUriError::NoAuthority),
        };
        let sni = if tls { Some(host) } else { None };
        (host, sni, port).try_into().map_err(Into::into)
    }
}

impl TryFrom<Uri> for Key {
    type Error = FromUriError;

    fn try_from(value: Uri) -> Result<Self, Self::Error> {
        Self::try_from(&value)
    }
}

// host, sni, port
impl TryFrom<(&str, Option<&str>, u16)> for Key {
    #[cfg(not(feature = "native-tls"))]
    type Error = rustls::client::InvalidDnsNameError;
    #[cfg(feature = "native-tls")]
    type Error = std::convert::Infallible;

    fn try_from((host, server_name, port): (&str, Option<&str>, u16)) -> Result<Self, Self::Error> {
        let server_name = match server_name {
            Some(s) => s,
            None => {
                return Ok(Self {
                    host: host.into(),
                    port,
                    version: http::version::Version::HTTP_11,
                    server_name: None,
                })
            }
        };

        #[cfg(not(feature = "native-tls"))]
        let server_name = Some(ServerName::try_from(server_name)?);
        #[cfg(feature = "native-tls")]
        let server_name = Some(ServerName(server_name.into()));

        Ok(Self {
            host: host.into(),
            port,
            version: http::version::Version::HTTP_11,
            server_name,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_default_port() {
        let key: Key = ("bytedance.com", Some("bytedance.com"), 80)
            .try_into()
            .expect("unable to convert to Key");
        assert_eq!(key.port, 80);
        assert_eq!(key.host, "bytedance.com");
        // #[cfg(feature = "rustls")]
        // assert_eq!(key.server_name, Some("bytedance.com".try_into().unwrap()));
        // #[cfg(all(feature = "native-tls", not(feature = "rustls")))]
        // assert_eq!(key.server_name, Some("bytedance.com".into()));
    }

    #[test]
    fn key_specify_port() {
        let uri = Uri::try_from("https://bytedance.com:12345").unwrap();
        let key: Key = uri.try_into().expect("unable to convert to Key");
        assert_eq!(key.port, 12345);
        assert_eq!(key.host, "bytedance.com");
        // #[cfg(feature = "rustls")]
        // assert_eq!(key.server_name, Some("bytedance.com".try_into().unwrap()));
        // #[cfg(all(feature = "native-tls", not(feature = "rustls")))]
        // assert_eq!(key.server_name, Some("bytedance.com".into()));
    }

    #[test]
    fn key_ip_http() {
        let uri = Uri::try_from("http://1.1.1.1:443").unwrap();
        let key: Key = uri.try_into().expect("unable to convert to Key");
        assert_eq!(key.port, 443);
        assert_eq!(key.host, "1.1.1.1");
        assert_eq!(key.server_name, None);
    }

    #[test]
    fn key_uri() {
        let uri = Uri::try_from("https://bytedance.com").unwrap();
        let key: Key = (&uri).try_into().expect("unable to convert to Key");
        assert_eq!(key.port, 443);
        assert_eq!(key.host, "bytedance.com");
        #[cfg(feature = "default")]
        assert_eq!(key.server_name, Some("bytedance.com".try_into().unwrap()));
        #[cfg(all(feature = "native-tls", not(feature = "default")))]
        assert_eq!(key.server_name, Some("bytedance.com".into()));
    }
}
