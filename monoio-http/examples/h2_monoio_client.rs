use http::{HeaderMap, Request};
use monoio::net::TcpStream;
use monoio_http::h2::client;

#[monoio::main]
async fn main() {
    let tcp = TcpStream::connect("127.0.0.1:59288").await.unwrap();
    let (mut client, h2) = client::handshake(tcp).await.unwrap();

    // Spawn a task to run the conn...
    monoio::spawn(async move {
        if let Err(e) = h2.await {
            println!("GOT ERR={:?}", e);
        }
    });

    let request = Request::builder()
        .uri("https://http2.akamai.com/")
        .body(())
        .unwrap();

    let mut trailers = HeaderMap::new();
    trailers.insert("zomg", "hello".parse().unwrap());

    let (response, mut stream) = client
        .send_request(request, false)
        .expect("Request send failed");

    // send trailers
    stream.send_trailers(trailers).unwrap();

    let response = response.await.unwrap();
    println!("GOT RESPONSE: {:?}", response);

    // Get the body
    let mut body = response.into_body();

    while let Some(chunk) = body.data().await {
        println!("GOT CHUNK = {:?}", chunk);
    }

    if let Some(trailers) = body.trailers().await.unwrap() {
        println!("GOT TRAILERS: {:?}", trailers);
    }
}
