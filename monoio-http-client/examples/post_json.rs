use std::collections::HashMap;

use monoio_http_client::Client;

#[monoio::main]
async fn main() {
    let client = Client::new();
    let payload = JsonRequest{
        name: "ihciah".to_string(),
        age: 27,
    };
    let resp = client
        .post("https://httpbin.org/post")
        .send_json(&payload)
        .await
        .expect("request fail");
    let json_resp: JsonResp = resp.json().await.unwrap();
    println!("{:?}", json_resp);
}

#[derive(serde::Serialize, serde::Deserialize, Debug)]
struct JsonRequest {
    name: String,
    age: u8,
}

#[allow(unused)]
#[derive(serde::Deserialize, Debug)]
struct JsonResp {
    headers: HashMap<String, String>,
    url: String,
    origin: String,
    json: JsonRequest,
}