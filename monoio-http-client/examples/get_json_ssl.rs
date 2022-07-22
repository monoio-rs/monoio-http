use std::collections::HashMap;

use monoio_http_client::Client;

#[monoio::main]
async fn main() {
    let client = Client::new();
    let resp = client
        .get("https://httpbin.org/get")
        .send()
        .await
        .expect("request fail");
    let json_resp: JsonResp = resp.json().await.expect("unable to parse json");
    println!("{:?}", json_resp);
}

#[allow(unused)]
#[derive(serde::Deserialize, Debug)]
struct JsonResp {
    headers: HashMap<String, String>,
    url: String,
    origin: String,
}
