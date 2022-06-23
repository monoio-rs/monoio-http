use http::{HeaderMap, Method, Uri, Version};

#[derive(Debug, Clone)]
pub struct RequestHead {
    pub method: Method,
    pub uri: Uri,
    pub version: Version,
    pub headers: HeaderMap,
}

pub struct Request<P> {
    pub head: RequestHead,
    pub payload: P,
}
