use std::io;

use bytes::Bytes;
use http::{HeaderMap, StatusCode, Version};
use monoio::{
    io::{
        sink::{Sink, SinkExt},
        stream::Stream,
    },
    net::{TcpListener, TcpStream},
};
use monoio_codec::FramedRead;
use monoio_http::{
    common::{
        request::Request,
        response::{Response, ResponseHead},
        ReqOrResp,
    },
    h1::{
        codec::{
            decoder::{DecodeError, RequestDecoder},
            encoder::ReqOrRespEncoder,
        },
        payload::{FixedPayload, Payload},
    },
    util::spsc::{spsc_pair, SPSCReceiver},
};

#[monoio::main]
async fn main() {
    let listener = TcpListener::bind("127.0.0.1:50002").unwrap();
    println!("Listening");
    loop {
        let incoming = listener.accept().await;
        match incoming {
            Ok((stream, addr)) => {
                println!("accepted a connection from {}", addr);
                monoio::spawn(handle_connection(stream));
            }
            Err(e) => {
                println!("accepted connection failed: {}", e);
            }
        }
    }
}

async fn handle_connection(stream: TcpStream) {
    let (r, w) = stream.into_split();
    let sender = ReqOrRespEncoder::new(w);
    let mut receiver = FramedRead::new(r, RequestDecoder::default());
    let (mut tx, rx) = spsc_pair();
    monoio::spawn(handle_task(rx, sender));

    loop {
        match receiver.next().await {
            None => {
                println!("connection closed, connection handler exit");
                return;
            }
            Some(Err(_)) => {
                println!("receive request failed, connection handler exit");
                return;
            }
            Some(Ok(item)) => match tx.send(item).await {
                Err(_) => {
                    println!("request handler dropped, connection handler exit");
                    return;
                }
                Ok(_) => {
                    println!("request handled success");
                }
            },
        }
    }
}

async fn handle_task(
    mut receiver: SPSCReceiver<Request<Payload<Bytes, DecodeError>>>,
    mut sender: impl Sink<
        ReqOrResp<ResponseHead, Payload<Bytes, io::Error>>,
        Error = impl Into<io::Error>,
    >,
) -> Result<(), io::Error> {
    loop {
        let request = match receiver.recv().await {
            Some(r) => r,
            None => {
                return Ok(());
            }
        };
        let resp = handle_request(request).await;
        sender.send_and_flush(resp).await.map_err(Into::into)?;
    }
}

async fn handle_request(
    req: Request<Payload<Bytes, DecodeError>>,
) -> Response<Payload<Bytes, io::Error>> {
    let mut headers = HeaderMap::new();
    headers.insert("Server", "monoio-http-demo".parse().unwrap());
    let mut has_error = false;
    let mut has_payload = false;
    let payload = match req.payload {
        Payload::None => Payload::None,
        Payload::Fixed(p) => match p.get().await {
            Ok(data) => {
                has_payload = true;
                Payload::Fixed(FixedPayload::new(data))
            }
            Err(_) => {
                has_error = true;
                Payload::None
            }
        },
        Payload::Stream(_) => unimplemented!(),
    };

    let status = if has_error {
        StatusCode::INTERNAL_SERVER_ERROR
    } else if has_payload {
        StatusCode::OK
    } else {
        StatusCode::NO_CONTENT
    };
    Response {
        head: ResponseHead {
            version: Version::HTTP_11,
            status,
            reason: None,
            headers,
        },
        payload,
    }
}
