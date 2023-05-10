use std::{fmt::Display, hash::Hash, net::ToSocketAddrs};

use http::{uri::Authority, Uri};
use monoio_http::{Param, ParamMut, ParamRef};
use smol_str::SmolStr;
use thiserror::Error as ThisError;

pub struct Key {
    host: SmolStr,
    port: u16,
    #[cfg(feature = "tls")]
    server_name: rustls::ServerName,
}

impl Clone for Key {
    fn clone(&self) -> Self {
        Self {
            host: self.host.clone(),
            port: self.port,
            #[cfg(feature = "tls")]
            server_name: self.server_name.clone(),
        }
    }
}

impl Display for Key {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.host, self.port)
    }
}

impl PartialEq for Key {
    fn eq(&self, other: &Self) -> bool {
        self.host == other.host && self.port == other.port
    }
}

impl Eq for Key {}

impl Hash for Key {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.host.hash(state);
        self.port.hash(state);
    }
}

impl ToSocketAddrs for Key {
    type Iter = <(&'static str, u16) as ToSocketAddrs>::Iter;

    fn to_socket_addrs(&self) -> std::io::Result<Self::Iter> {
        (self.host.as_str(), self.port).to_socket_addrs()
    }
}

#[cfg(feature = "tls")]
impl Param<rustls::ServerName> for Key {
    fn param(&self) -> rustls::ServerName {
        self.server_name.clone()
    }
}

#[cfg(feature = "tls")]
impl ParamRef<rustls::ServerName> for Key {
    fn param_ref(&self) -> &rustls::ServerName {
        &self.server_name
    }
}

#[cfg(feature = "tls")]
impl ParamMut<rustls::ServerName> for Key {
    fn param_mut(&mut self) -> &mut rustls::ServerName {
        &mut self.server_name
    }
}

#[derive(ThisError, Debug)]
pub enum FromUriError {
    #[cfg(feature = "tls")]
    #[error("invalid dns name")]
    InvalidDnsName(#[from] rustls::client::InvalidDnsNameError),
    #[error("scheme not support")]
    UnsupportScheme,
    #[error("no authority in uri")]
    NoAuthority,
}

impl TryFrom<&Uri> for Key {
    type Error = FromUriError;

    fn try_from(uri: &Uri) -> Result<Self, Self::Error> {
        let default_port: u16 = match uri.scheme() {
            Some(scheme) if scheme == &http::uri::Scheme::HTTP => 80,
            Some(scheme) if scheme == &http::uri::Scheme::HTTPS => 443,
            _ => 0,
        };
        let authority = match uri.authority() {
            Some(a) => a,
            None => return Err(FromUriError::NoAuthority),
        };
        (authority, default_port).try_into().map_err(Into::into)
    }
}

impl TryFrom<(&Authority, u16)> for Key {
    #[cfg(feature = "tls")]
    type Error = rustls::client::InvalidDnsNameError;
    #[cfg(not(feature = "tls"))]
    type Error = std::convert::Infallible;

    fn try_from(a: (&Authority, u16)) -> Result<Self, Self::Error> {
        let (authority, default_port) = a;
        let host = authority.host();
        let port = authority.port_u16().unwrap_or(default_port);
        #[cfg(feature = "tls")]
        let server_name = rustls::ServerName::try_from(host)?;
        Ok(Self {
            host: host.into(),
            port,
            #[cfg(feature = "tls")]
            server_name,
        })
    }
}

impl TryFrom<(Authority, u16)> for Key {
    type Error = <Key as TryFrom<(&'static Authority, u16)>>::Error;

    fn try_from(a: (Authority, u16)) -> Result<Self, Self::Error> {
        let r = (&a.0, a.1);
        r.try_into()
    }
}

#[cfg(test)]
mod tests {
    use std::borrow::Borrow;

    use super::*;

    #[test]
    fn key_default_port() {
        let authority = Authority::try_from("bytedance.com").unwrap();
        let key: Key = (&authority, 80)
            .try_into()
            .expect("unable to convert to Key");
        assert_eq!(key.port, 80);
        assert_eq!(key.host, "bytedance.com");
        #[cfg(feature = "tls")]
        assert_eq!(key.server_name, "bytedance.com".try_into().unwrap());
    }

    #[test]
    fn key_specify_port() {
        let authority = Authority::try_from("bytedance.com:12345").unwrap();
        let key: Key = (&authority, 80)
            .try_into()
            .expect("unable to convert to Key");
        assert_eq!(key.port, 12345);
        assert_eq!(key.host, "bytedance.com");
        #[cfg(feature = "tls")]
        assert_eq!(key.server_name, "bytedance.com".try_into().unwrap());
    }

    #[test]
    fn key_ip() {
        let authority = Authority::try_from("1.1.1.1").unwrap();
        let key: Key = (&authority, 443)
            .try_into()
            .expect("unable to convert to Key");
        assert_eq!(key.port, 443);
        assert_eq!(key.host, "1.1.1.1");
        #[cfg(feature = "tls")]
        assert_eq!(key.server_name, "1.1.1.1".try_into().unwrap());
    }

    #[test]
    fn key_uri() {
        let uri = Uri::try_from("https://bytedance.com").unwrap();
        let key: Key = uri.borrow().try_into().expect("unable to convert to Key");
        assert_eq!(key.port, 443);
        assert_eq!(key.host, "bytedance.com");
        #[cfg(feature = "tls")]
        assert_eq!(key.server_name, "bytedance.com".try_into().unwrap());
    }
}
