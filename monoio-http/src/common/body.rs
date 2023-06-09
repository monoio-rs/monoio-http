use monoio::buf::IoBuf;

pub enum StreamHint {
    None,
    Fixed,
    Stream,
}

pub trait Body {
    type Data: IoBuf;
    type Error;
    type DataFuture<'a>: std::future::Future<Output = Result<Option<Self::Data>, Self::Error>>
    where
        Self: 'a;

    fn data(&mut self) -> Self::DataFuture<'_>;
    // fn end_of_stream(&self) -> bool;
    fn stream_hint(&self) -> StreamHint;
}
