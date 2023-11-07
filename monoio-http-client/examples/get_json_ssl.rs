use std::collections::HashMap;

use monoio_http::{common::body::Body, h1::payload::Payload};
use monoio_http_client::Client;

#[monoio::main(enable_timer = true)]
async fn main() {
    let client = Client::builder().http1_client().build();
    let resp = client
        .get("https://httpbin.org/get", Payload::None)
        .await
        .expect("request fail");
    let (_, mut payload) = resp.into_parts();
    let json_resp = payload.next_data().await.expect("unable to parse json");
    println!("{:?}", json_resp);
}

#[allow(unused)]
#[derive(serde::Deserialize, Debug)]
struct JsonResp {
    headers: HashMap<String, String>,
    url: String,
    origin: String,
}
