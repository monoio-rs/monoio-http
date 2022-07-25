pub mod connector;
pub mod key;
pub mod pool;

use std::rc::Rc;

use http::{uri::Scheme, HeaderMap};
use monoio::io::sink::SinkExt;
use monoio::io::stream::Stream;
use monoio_http::h1::codec::decoder::FillPayload;
use monoio_http::h1::payload::Payload;

use self::connector::Connector;
use crate::request::ClientRequest;

use self::{
    connector::{DefaultTcpConnector, DefaultTlsConnector},
    key::Key,
};

// TODO: ClientBuilder
pub struct ClientInner<C, #[cfg(feature = "tls")] CS> {
    cfg: ClientConfig,
    http_connector: C,
    #[cfg(feature = "tls")]
    https_connector: CS,
}

pub struct Client<
    C = DefaultTcpConnector<Key>,
    #[cfg(feature = "tls")] CS = DefaultTlsConnector<Key>,
> {
    #[cfg(feature = "tls")]
    shared: Rc<ClientInner<C, CS>>,
    #[cfg(not(feature = "tls"))]
    shared: Rc<ClientInner<C>>,
}

#[cfg(feature = "tls")]
impl<C, CS> Clone for Client<C, CS> {
    fn clone(&self) -> Self {
        Self {
            shared: self.shared.clone(),
        }
    }
}

#[cfg(not(feature = "tls"))]
impl<C> Clone for Client<C> {
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
        Self::new()
    }
}

macro_rules! http_method {
    ($fn: ident, $method: expr) => {
        pub fn $fn<U>(&self, uri: U) -> ClientRequest
        where
            http::Uri: TryFrom<U>,
            <http::Uri as TryFrom<U>>::Error: Into<http::Error>,
        {
            self.request($method, uri)
        }
    };
}

impl Client {
    pub fn new() -> Self {
        let shared = Rc::new(ClientInner {
            cfg: ClientConfig::default(),
            http_connector: Default::default(),
            #[cfg(feature = "tls")]
            https_connector: Default::default(),
        });
        Self { shared }
    }

    http_method!(get, http::Method::GET);
    http_method!(post, http::Method::POST);
    http_method!(put, http::Method::PUT);
    http_method!(patch, http::Method::PATCH);
    http_method!(delete, http::Method::DELETE);
    http_method!(head, http::Method::HEAD);

    // TODO: allow other connector impl.
    pub fn request<M, U>(&self, method: M, uri: U) -> ClientRequest
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

    pub async fn send(
        &self,
        request: http::Request<Payload>,
    ) -> Result<http::Response<Payload>, crate::Error> {
        let uri = request.uri();
        let key = uri.try_into()?;
        #[cfg(feature = "tls")]
        if uri
            .scheme()
            .map(|scheme| scheme == &Scheme::HTTPS)
            .unwrap_or(false)
        {
            let mut codec = self.shared.https_connector.connect(key).await?;
            codec.send_and_flush(request).await?;
            let resp = codec.next().await.ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "unexpected eof when read response",
                )
            })??;
            codec.fill_payload().await?;
            return Ok(resp);
        }
        let mut codec = self.shared.http_connector.connect(key).await?;
        codec.send_and_flush(request).await?;
        let resp = codec.next().await.ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "unexpected eof when read response",
            )
        })??;
        codec.fill_payload().await?;
        // for http/1.1, if remote reply closed, we should mark the connection as not reuseable.
        // TODO: handle Keep-Alive
        if resp.version() == http::Version::HTTP_11
            && matches!(
                resp.headers()
                    .get(http::header::CONNECTION)
                    .map(|x| x.as_bytes()),
                Some(b"closed")
            )
        {
            codec.set_reuseable(false);
        }
        // for http/1.0, if remote not set Connection or set it as close, we should mark
        // the connection as not reuseable.
        if resp.version() == http::Version::HTTP_10
            && matches!(
                resp.headers()
                    .get(http::header::CONNECTION)
                    .map(|x| x.as_bytes()),
                Some(b"closed") | None
            )
        {
            codec.set_reuseable(false);
        }

        Ok(resp)
    }
}
