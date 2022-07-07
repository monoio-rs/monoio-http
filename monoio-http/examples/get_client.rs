//! Simple HTTP Get example with low level codec.
//! We use captive.apple.com as target service.

use std::io;

use bytes::Bytes;
use http::{HeaderMap, Method, Version};
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

#[monoio::main]
async fn main() {
    let mut headers = HeaderMap::new();
    headers.insert(http::header::HOST, "captive.apple.com".parse().unwrap());
    headers.insert(http::header::ACCEPT, "*/*".parse().unwrap());
    headers.insert(http::header::USER_AGENT, "monoio-http".parse().unwrap());
    let request = Request {
        head: RequestHead {
            method: Method::GET,
            uri: "/".parse().unwrap(),
            version: Version::HTTP_11,
            headers,
        },
        payload: Payload::<Bytes, io::Error>::None,
    };

    println!("Request constructed, will connect");
    let conn = monoio::net::TcpStream::connect("captive.apple.com:80")
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
    monoio::select! {
        _ = receiver.next() => {},
        _ = process_payload(payload) => {},
    }
}

async fn process_payload(payload: FixedPayload<Bytes, DecodeError>) {
    let data = payload.get().await.expect("unable to read response body");
    println!("Response body: {}", String::from_utf8_lossy(&data));
}
