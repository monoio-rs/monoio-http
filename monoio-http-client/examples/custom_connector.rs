#![feature(type_alias_impl_trait)]
#![feature(impl_trait_in_assoc_type)]

use std::future::Future;

use monoio_http_client::{
    unified::{UnifiedTransportAddr, UnifiedTransportConnection, UnifiedTransportConnector},
    Client, Connector, Error, Key,
};
use service_async::Param;

#[monoio::main(enable_timer = true)]
async fn main() {
    let client = Client::builder().build_with_connector(CloudflareConnector::default());
    let resp = client
        .get("https://ihc.im")
        .send()
        .await
        .expect("request fail");
    let http_resp = resp.bytes().await.unwrap();
    // Output is expected to be "error code: 1034"
    println!("{:?}", http_resp);

    // Run `socat UNIX-LISTEN:/tmp/proxy.sock TCP:1.1.1.1:443`
    let client = Client::builder().build_with_connector(CustomUnixConnector::default());
    let resp = client
        .get("https://ihc.im")
        .send()
        .await
        .expect("request fail");
    let http_resp = resp.bytes().await.unwrap();
    // Output is expected to be "error code: 1034"
    println!("{:?}", http_resp);
}

/// Force resolve to 1.1.1.1:443
#[derive(Default)]
struct CloudflareConnector {
    inner: UnifiedTransportConnector,
}

impl Connector<Key> for CloudflareConnector {
    type Connection = UnifiedTransportConnection;
    type Error = Error;

    async fn connect(&self, mut key: Key) -> Result<Self::Connection, Self::Error> {
        key.host = "1.1.1.1".into();
        key.port = 443;

        self.inner.connect(key).await
    }
}

/// Force resolve to unix /tmp/proxy.sock
#[derive(Default)]
struct CustomUnixConnector {
    inner: UnifiedTransportConnector,
}

impl Connector<Key> for CustomUnixConnector {
    type Connection = UnifiedTransportConnection;
    type Error = Error;

    async fn connect(&self, key: Key) -> Result<Self::Connection, Self::Error> {
        let modified = "/tmp/proxy.sock".into();
        let addr = match key.param() {
            UnifiedTransportAddr::Tcp(_, _) => UnifiedTransportAddr::Unix(modified),
            UnifiedTransportAddr::Unix(_) => UnifiedTransportAddr::Unix(modified),
            UnifiedTransportAddr::TcpTls(_, _, sn) => UnifiedTransportAddr::UnixTls(modified, sn),
            UnifiedTransportAddr::UnixTls(_, sn) => UnifiedTransportAddr::UnixTls(modified, sn),
        };

        self.inner.connect(addr).await
    }
}
