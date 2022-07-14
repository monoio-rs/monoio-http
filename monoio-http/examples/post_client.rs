//! Simple HTTP Post example with low level codec.
//! We use httpbin.org/post as target service.
//! Nearly equivalent to:
//! curl -X POST -H 'Content-Type: application/json' https://httpbin.org/post -d '{"key": "val"}'

use std::collections::HashMap;

use bytes::Bytes;
use http::{HeaderMap, Method, Version};
use monoio::io::{sink::SinkExt, stream::Stream};
use monoio_http::{
    common::request::{Request, RequestHead},
    h1::{
        codec::{decoder::ResponseDecoder, encoder::GenericEncoder},
        payload::{FixedPayload, Payload},
    },
};
use serde::Deserialize;

const TEST_DATA: &str = r#"{"key": "val"}"#;

#[monoio::main]
async fn main() {
    let payload: Bytes = TEST_DATA.into();
    let mut headers = HeaderMap::new();
    headers.insert(http::header::HOST, "httpbin.org".parse().unwrap());
    headers.insert(http::header::ACCEPT, "*/*".parse().unwrap());
    headers.insert(http::header::USER_AGENT, "monoio-http".parse().unwrap());
    headers.insert(
        http::header::CONTENT_TYPE,
        "application/json".parse().unwrap(),
    );
    let request = Request {
        head: RequestHead {
            method: Method::POST,
            uri: "/post".parse().unwrap(),
            version: Version::HTTP_11,
            headers,
        },
        payload: Payload::Fixed(FixedPayload::new(payload)),
    };

    println!("Request constructed, will connect");
    let conn = monoio::net::TcpStream::connect("httpbin.org:80")
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
    println!("Status code: {}", resp.head.status);
    let payload = match resp.payload {
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
    let resp: HttpbinResponse = serde_json::from_slice(&data).expect("unable to parse json body");
    println!("Response json: {resp:?}");
    assert_eq!(resp.data, TEST_DATA);
    assert_eq!(
        resp.headers.get("User-Agent").expect("header not exist"),
        "monoio-http"
    );
    assert_eq!(resp.url, "http://httpbin.org/post");
}
