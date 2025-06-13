use http::{request::Builder, Method, Version};
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

    let h2_client = monoio_http_client::Builder::new()
        .http2_client()
        .build();

    let body = HttpBody::fixed_body(None);

    let request = Builder::new()
        .method(Method::GET)
        .uri("https://httpbin.org/get")
        .version(Version::HTTP_2)
        .header(http::header::USER_AGENT, "monoio-http")
        .header(http::header::ACCEPT, "*/*")
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
