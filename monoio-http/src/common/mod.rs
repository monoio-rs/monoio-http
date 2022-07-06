pub mod request;
pub mod response;

pub struct ReqOrResp<H, P> {
    pub head: H,
    pub payload: P,
}
