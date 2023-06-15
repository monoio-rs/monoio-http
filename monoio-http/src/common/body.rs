use monoio::buf::IoBuf;

pub enum StreamHint {
    None,
    Fixed,
    Stream,
}

pub trait Body {
    type Data: IoBuf;
    type Error;
    type DataFuture<'a>: std::future::Future<Output = Option<Result<Self::Data, Self::Error>>>
    where
        Self: 'a;

    fn next_data(&mut self) -> Self::DataFuture<'_>;
    fn stream_hint(&self) -> StreamHint;
}
