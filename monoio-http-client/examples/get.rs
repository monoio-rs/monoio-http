use monoio_http_client::Client;

#[monoio::main]
async fn main() {
    let client = Client::new();
    let resp = client
        .get("http://captive.apple.com")
        .send()
        .await
        .expect("request fail");
    let http_resp = resp.bytes().await.unwrap();
    println!("{:?}", http_resp);
}
