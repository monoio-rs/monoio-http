use std::collections::HashMap;

use monoio_http_client::Client;

#[monoio::main(enable_timer = true)]
async fn main() {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .init();

    let client = Client::default();
    let payload = JsonRequest {
        name: "ihciah".to_string(),
        age: 27,
    };
    for i in 0..5 {
        let resp = client
            .post("https://httpbin.org/post")
            .send_json(&payload)
            .await
            .expect("request fail");
        let json_resp: JsonResp = resp.json().await.unwrap();
        println!("The {i}th response: {:?}", json_resp);
    }
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
