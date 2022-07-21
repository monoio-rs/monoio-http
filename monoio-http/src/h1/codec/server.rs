use monoio::io::{sink::Sink, stream::Stream};

use crate::util::split::{split, OwnedReadHalf, OwnedWriteHalf};

use super::{
    decoder::{FillPayload, RequestDecoder},
    encoder::GenericEncoder,
};

pub struct ServerCodec<IO> {
    encoder: GenericEncoder<OwnedWriteHalf<IO>>,
    decoder: RequestDecoder<OwnedReadHalf<IO>>,
}

impl<IO> ServerCodec<IO> {
    pub fn new(io: IO) -> Self {
        // # Safety: Since we will not use the encoder and decoder at once, we can split it safely.
        let (r, w) = unsafe { split(io) };
        Self {
            encoder: GenericEncoder::new(w),
            decoder: RequestDecoder::new(r),
        }
    }
}

impl<IO, R> Sink<R> for ServerCodec<IO>
where
    GenericEncoder<OwnedWriteHalf<IO>>: Sink<R>,
{
    type Error = <GenericEncoder<OwnedWriteHalf<IO>> as Sink<R>>::Error;

    type SendFuture<'a> = <GenericEncoder<OwnedWriteHalf<IO>> as Sink<R>>::SendFuture<'a>
    where
        Self: 'a;

    type FlushFuture<'a> = <GenericEncoder<OwnedWriteHalf<IO>> as Sink<R>>::FlushFuture<'a>
    where
        Self: 'a;

    type CloseFuture<'a> = <GenericEncoder<OwnedWriteHalf<IO>> as Sink<R>>::CloseFuture<'a>
    where
        Self: 'a;

    fn send(&mut self, item: R) -> Self::SendFuture<'_> {
        self.encoder.send(item)
    }

    fn flush(&mut self) -> Self::FlushFuture<'_> {
        self.encoder.flush()
    }

    fn close(&mut self) -> Self::CloseFuture<'_> {
        self.encoder.close()
    }
}

impl<IO> FillPayload for ServerCodec<IO>
where
    RequestDecoder<OwnedReadHalf<IO>>: FillPayload,
{
    type Error = <RequestDecoder<OwnedReadHalf<IO>> as FillPayload>::Error;

    type FillPayloadFuture<'a> = <RequestDecoder<OwnedReadHalf<IO>> as FillPayload>::FillPayloadFuture<'a>
    where
        Self: 'a;

    fn fill_payload(&mut self) -> Self::FillPayloadFuture<'_> {
        self.decoder.fill_payload()
    }
}

impl<IO> Stream for ServerCodec<IO>
where
    RequestDecoder<OwnedReadHalf<IO>>: Stream,
{
    type Item = <RequestDecoder<OwnedReadHalf<IO>> as Stream>::Item;

    type NextFuture<'a> = <RequestDecoder<OwnedReadHalf<IO>> as Stream>::NextFuture<'a>
    where
        Self: 'a;

    fn next(&mut self) -> Self::NextFuture<'_> {
        self.decoder.next()
    }
}
