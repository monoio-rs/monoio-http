use std::time::Duration;

use http::{request::Builder, Method, Version};
use monoio::time::sleep;
use monoio_http::common::body::{Body, FixedBody, HttpBody};
use tracing_subscriber::FmtSubscriber;

#[monoio::main(enable_timer = true)]
async fn main() {
    let subscriber = FmtSubscriber::builder()
        .with_max_level(tracing::Level::DEBUG)
        .finish();
    // Initialize the tracing subscriber
    tracing::subscriber::set_global_default(subscriber)
        .expect("Failed to set up the tracing subscriber");

    let h2_client = monoio_http_client::Builder::new().build_http2();
    let mut first = true;

    for _ in 0..6 {
        if first {
            sleep(Duration::from_millis(1000)).await;
            first = false;
        }
        let body = HttpBody::fixed_body(None);

        let request = Builder::new()
            .method(Method::GET)
            // HTTP Upgrade not supported, requires
            // a HTTP2 server
            .uri("http://127.0.0.1:8080/")
            .version(Version::HTTP_2)
            .body(body)
            .unwrap();

        tracing::debug!("starting request");

        let resp = h2_client
            .send_request(request)
            .await
            .expect("Sending request");
        let (parts, mut body) = resp.into_parts();
        println!("{:?}", parts);
        while let Some(Ok(data)) = body.next_data().await {
            println!("{:?}", data);
        }
    }
}
