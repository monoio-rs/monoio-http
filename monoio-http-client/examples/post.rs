use std::collections::HashMap;

use bytes::Bytes;
use monoio_http_client::Client;

#[monoio::main]
async fn main() {
    let client = Client::new();
    let payload = Bytes::from_static(b"my_payload_data_balabala");
    let resp = client
        .post("https://httpbin.org/post")
        .send_body(payload)
        .await
        .expect("request fail");
    let json_resp: JsonResp = resp.json().await.unwrap();
    println!("{:?}", json_resp);
}

#[allow(unused)]
#[derive(serde::Deserialize, Debug)]
struct JsonResp {
    headers: HashMap<String, String>,
    url: String,
    origin: String,
    data: String,
}