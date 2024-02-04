use std::time::Duration;

use monoio::io::{
    sink::Sink, stream::Stream, AsyncReadRent, AsyncWriteRent, OwnedReadHalf, OwnedWriteHalf,
    Split, Splitable,
};

use super::{
    decoder::{FillPayload, RequestDecoder},
    encoder::GenericEncoder,
};

pub struct ServerCodec<IO: AsyncWriteRent> {
    encoder: GenericEncoder<OwnedWriteHalf<IO>>,
    decoder: RequestDecoder<OwnedReadHalf<IO>>,
}

impl<IO: Split + AsyncReadRent + AsyncWriteRent> ServerCodec<IO> {
    #[inline]
    pub fn new(io: IO) -> Self {
        // # Safety: Since we will not use the encoder and decoder at once, we can split it safely.
        let (r, w) = io.into_split();
        Self {
            encoder: GenericEncoder::new(w),
            decoder: RequestDecoder::new(r),
        }
    }

    #[inline]
    pub fn new_with_timeout(io: IO, timeout: Duration) -> Self {
        // # Safety: Since we will not use the encoder and decoder at once, we can split it safely.
        let (r, w) = io.into_split();
        Self {
            encoder: GenericEncoder::new(w),
            decoder: RequestDecoder::new_with_timeout(r, timeout),
        }
    }
}

impl<IO: AsyncWriteRent, R> Sink<R> for ServerCodec<IO>
where
    GenericEncoder<OwnedWriteHalf<IO>>: Sink<R>,
{
    type Error = <GenericEncoder<OwnedWriteHalf<IO>> as Sink<R>>::Error;

    async fn send(&mut self, item: R) -> Result<(), Self::Error> {
        self.encoder.send(item).await
    }

    async fn flush(&mut self) -> Result<(), Self::Error> {
        self.encoder.flush().await
    }

    async fn close(&mut self) -> Result<(), Self::Error> {
        self.encoder.close().await
    }
}

impl<IO: AsyncWriteRent> FillPayload for ServerCodec<IO>
where
    RequestDecoder<OwnedReadHalf<IO>>: FillPayload,
{
    type Error = <RequestDecoder<OwnedReadHalf<IO>> as FillPayload>::Error;

    async fn fill_payload(&mut self) -> Result<(), Self::Error> {
        self.decoder.fill_payload().await
    }
}

impl<IO: AsyncWriteRent> Stream for ServerCodec<IO>
where
    RequestDecoder<OwnedReadHalf<IO>>: Stream,
{
    type Item = <RequestDecoder<OwnedReadHalf<IO>> as Stream>::Item;

    async fn next(&mut self) -> Option<Self::Item> {
        self.decoder.next().await
    }
}
