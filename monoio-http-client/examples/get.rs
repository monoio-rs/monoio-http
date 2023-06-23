use monoio_http_client::Client;

#[monoio::main(enable_timer = true)]
async fn main() {
    let client = Client::default();
    let resp = client
        .get("https://httpbin.org/get")
        .send()
        .await
        .expect("request fail");
    let http_resp = resp.bytes().await.unwrap();
    println!("{:?}", http_resp);
}
