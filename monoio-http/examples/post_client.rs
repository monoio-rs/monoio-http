//! Simple HTTP Post example with low level codec.
//! We use httpbin.org/post as target service.
//! Nearly equivalent to:
//! curl -X POST -H 'Content-Type: application/json' https://httpbin.org/post -d '{"key": "val"}'

use std::collections::HashMap;

use bytes::Bytes;
use http::{HeaderMap, HeaderValue, Method, Version};
use monoio::io::{sink::Sink, stream::Stream};
use monoio_codec::FramedRead;
use monoio_http::{
    common::request::{Request, RequestHead},
    h1::{
        codec::{
            decoder::{DecodeError, ResponseDecoder},
            encoder::ReqOrRespEncoder,
        },
        payload::{FixedPayload, Payload},
    },
};
use serde::Deserialize;

#[monoio::main]
async fn main() {
    let payload: Bytes = r#"{"key": "val"}"#.into();
    let mut headers = HeaderMap::new();
    headers.insert(http::header::HOST, "httpbin.org".parse().unwrap());
    headers.insert(http::header::ACCEPT, "*/*".parse().unwrap());
    headers.insert(http::header::USER_AGENT, "monoio-http".parse().unwrap());
    headers.insert(
        http::header::CONTENT_TYPE,
        "application/json".parse().unwrap(),
    );
    // TODO: Content-Length injection should be done in `send`.
    headers.insert(
        http::header::CONTENT_LENGTH,
        HeaderValue::from_str(&format!("{}", payload.len())).unwrap(),
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
    let mut sender = ReqOrRespEncoder::new(w);
    let mut receiver = FramedRead::new(r, ResponseDecoder::default());

    println!("Connected, will send request");
    sender.send(request).await.expect("unable to send request");
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
    monoio::pin! {
        let process = process_payload(payload);
    }
    loop {
        monoio::select! {
            _ = receiver.next() => {},
            _ = &mut process => return,
        }
    }
}

#[derive(Deserialize, Debug)]
struct HttpbinResponse {
    data: String,
    headers: HashMap<String, String>,
    url: String,
}

async fn process_payload(payload: FixedPayload<Bytes, DecodeError>) {
    let data = payload.get().await.expect("unable to read response body");
    let resp: HttpbinResponse = serde_json::from_slice(&data).expect("unable to parse json body");
    println!("Response json: {resp:?}");
    assert_eq!(resp.data.len(), 14); // {"key": "val"}
    assert_eq!(
        resp.headers.get("User-Agent").expect("header not exist"),
        "monoio-http"
    );
    assert_eq!(resp.url, "http://httpbin.org/post");
}
