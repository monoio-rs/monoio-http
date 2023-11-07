use std::collections::HashMap;

use monoio_http::{common::body::Body, h1::payload::Payload};
use monoio_http_client::Client;

#[monoio::main(enable_timer = true)]
async fn main() {
    let client = Client::default();
    let resp = client
        .get("http://httpbin.org/get", Payload::None)
        .await
        .expect("request fail");
    let (_, mut body) = resp.into_parts();
    while let Some(Ok(data)) = body.next_data().await {
        println!("{:?}", data);
    }
}

#[allow(unused)]
#[derive(serde::Deserialize, Debug)]
struct JsonResp {
    headers: HashMap<String, String>,
    url: String,
    origin: String,
}
