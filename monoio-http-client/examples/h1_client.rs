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

    let h1_client = monoio_http_client::Builder::new().build_http1();
    let client1 = h1_client.clone();
    let client2 = h1_client.clone();

    let h = monoio::spawn(async move {
        let body = HttpBody::fixed_body(None);
        let request = Builder::new()
            .method(Method::GET)
            .uri("https://httpbin.org/get")
            .version(Version::HTTP_11)
            .header(http::header::HOST, "captive.apple.com")
            .header(http::header::ACCEPT, "*/*")
            .header(http::header::USER_AGENT, "monoio-http")
            .body(body)
            .unwrap();

        tracing::debug!("starting request");

        let resp = client1
            .send_request(request)
            .await
            .expect("Request send failed");
        let (parts, mut body) = resp.into_parts();
        println!("{:?}", parts);
        while let Some(Ok(data)) = body.next_data().await {
            println!("{:?}", data);
        }
    });

    for _ in 0..3 {
        let body = HttpBody::fixed_body(None);

        let request = Builder::new()
            .method(Method::GET)
            .uri("https://httpbin.org/html")
            .version(Version::HTTP_11)
            .header(http::header::HOST, "captive.apple.com")
            .header(http::header::ACCEPT, "*/*")
            .header(http::header::USER_AGENT, "monoio-http")
            .body(body)
            .unwrap();

        let resp = client2
            .send_request(request)
            .await
            .expect("Request send failed");
        let (parts, mut body) = resp.into_parts();
        println!("{:?}", parts);
        while let Some(Ok(data)) = body.next_data().await {
            println!("{:?}", data);
        }
    }

    h.await;
}
