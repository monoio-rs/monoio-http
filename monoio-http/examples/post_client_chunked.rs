//! Simple HTTP Post(chunked body) example with low level codec.
//! We use httpbin.org/post as target service.

use std::collections::HashMap;

use http::{request::Builder, Method, Version};
use monoio::io::{sink::SinkExt, stream::Stream};
use monoio_http::h1::{
    codec::{decoder::ResponseDecoder, encoder::GenericEncoder},
    payload::{stream_payload_pair, FixedPayload, Payload},
};
use serde::Deserialize;

const TEST_DATA: [&str; 3] = [r#"{"ke"#, r#"y": "v"#, r#"al"}"#];
const FULL_DATA: &str = r#"{"key": "val"}"#;

#[monoio::main(enable_timer = true)]
async fn main() {
    let (payload, mut payload_sender) = stream_payload_pair();

    let request = Builder::new()
        .method(Method::POST)
        .uri("/post")
        .version(Version::HTTP_11)
        .header(http::header::HOST, "httpbin.org")
        .header(http::header::ACCEPT, "*/*")
        .header(http::header::USER_AGENT, "monoio-http")
        .body(Payload::Stream(payload))
        .unwrap();

    println!("Request constructed, will connect");
    let conn = monoio::net::TcpStream::connect("httpbin.org:80")
        .await
        .expect("unable to connect");
    let (r, w) = conn.into_split();
    let mut sender = GenericEncoder::new(w);
    let mut receiver = ResponseDecoder::new(r);
    println!("Connected, will send request");

    monoio::spawn(async move {
        println!("Will feed {} data blocks at interval 0.5s", TEST_DATA.len());
        for data in TEST_DATA.iter() {
            monoio::time::sleep(std::time::Duration::from_millis(500)).await;
            payload_sender.feed_data(Some((*data).into()));
            println!("Feed data: {}", data);
        }
        println!("Feeding data finished.");
        payload_sender.feed_data(None);
    });
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

#[derive(Deserialize, Debug)]
struct HttpbinResponse {
    data: String,
    headers: HashMap<String, String>,
    url: String,
}

async fn process_payload(payload: FixedPayload) {
    let data = payload.get().await.expect("unable to read response body");
    println!("{:?}", data);
    let resp: HttpbinResponse = serde_json::from_slice(&data).expect("unable to parse json body");
    println!("Response json: {resp:?}");
    assert_eq!(resp.data, FULL_DATA);
    assert_eq!(
        resp.headers.get("User-Agent").expect("header not exist"),
        "monoio-http"
    );
    assert_eq!(resp.url, "http://httpbin.org/post");
}
