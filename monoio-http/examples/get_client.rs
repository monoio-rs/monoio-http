//! Simple HTTP Get example with low level codec.
//! We use captive.apple.com as target service.

use http::{request::Builder, Method, Version};
use monoio::io::{sink::SinkExt, stream::Stream};
use monoio_http::h1::{
    codec::{decoder::ResponseDecoder, encoder::GenericEncoder},
    payload::{FixedPayload, Payload},
};

#[monoio::main]
async fn main() {
    let request = Builder::new()
        .method(Method::GET)
        .uri("/")
        .version(Version::HTTP_11)
        .header(http::header::HOST, "captive.apple.com")
        .header(http::header::ACCEPT, "*/*")
        .header(http::header::USER_AGENT, "monoio-http")
        .body(Payload::None)
        .unwrap();

    println!("Request constructed, will connect");
    let conn = monoio::net::TcpStream::connect("captive.apple.com:80")
        .await
        .expect("unable to connect");
    let (r, w) = conn.into_split();
    let mut sender = GenericEncoder::new(w);
    let mut receiver = ResponseDecoder::new(r);

    println!("Connected, will send request");
    sender
        .send_and_flush(request)
        .await
        .expect("unable to send request");
    println!("Request send, will wait for response");
    let resp = receiver
        .next()
        .await
        .expect("disconnected")
        .expect("parse response failed");

    println!("Status code: {}", resp.status());
    let payload = match resp.into_body() {
        Payload::Fixed(payload) => payload,
        _ => panic!("unexpected payload type"),
    };
    receiver
        .fill_payload()
        .await
        .expect("unable to get payload");
    process_payload(payload).await;
}

async fn process_payload(payload: FixedPayload) {
    let data = payload.get().await.expect("unable to read response body");
    println!("Response body: {}", String::from_utf8_lossy(&data));
}
