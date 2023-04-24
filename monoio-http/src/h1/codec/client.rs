use monoio::io::{sink::Sink, stream::Stream};

use super::{
    decoder::{FillPayload, ResponseDecoder},
    encoder::GenericEncoder,
};
use crate::util::split::{split, OwnedReadHalf, OwnedWriteHalf};

pub struct ClientCodec<IO> {
    encoder: GenericEncoder<OwnedWriteHalf<IO>>,
    decoder: ResponseDecoder<OwnedReadHalf<IO>>,
}

impl<IO> ClientCodec<IO> {
    pub fn new(io: IO) -> Self {
        // # Safety: Since we will not use the encoder and decoder at once, we can split it safely.
        let (r, w) = unsafe { split(io) };
        Self {
            encoder: GenericEncoder::new(w),
            decoder: ResponseDecoder::new(r),
        }
    }
}

impl<IO, R> Sink<R> for ClientCodec<IO>
where
    GenericEncoder<OwnedWriteHalf<IO>>: Sink<R>,
{
    type Error = <GenericEncoder<OwnedWriteHalf<IO>> as Sink<R>>::Error;

    type SendFuture<'a> = <GenericEncoder<OwnedWriteHalf<IO>> as Sink<R>>::SendFuture<'a>
    where
        Self: 'a, R: 'a;

    type FlushFuture<'a> = <GenericEncoder<OwnedWriteHalf<IO>> as Sink<R>>::FlushFuture<'a>
    where
        Self: 'a;

    type CloseFuture<'a> = <GenericEncoder<OwnedWriteHalf<IO>> as Sink<R>>::CloseFuture<'a>
    where
        Self: 'a;

    fn send<'a>(&'a mut self, item: R) -> Self::SendFuture<'a>
    where
        R: 'a,
    {
        self.encoder.send(item)
    }

    fn flush(&mut self) -> Self::FlushFuture<'_> {
        self.encoder.flush()
    }

    fn close(&mut self) -> Self::CloseFuture<'_> {
        self.encoder.close()
    }
}

impl<IO> FillPayload for ClientCodec<IO>
where
    ResponseDecoder<OwnedReadHalf<IO>>: FillPayload,
{
    type Error = <ResponseDecoder<OwnedReadHalf<IO>> as FillPayload>::Error;

    type FillPayloadFuture<'a> = <ResponseDecoder<OwnedReadHalf<IO>> as FillPayload>::FillPayloadFuture<'a>
    where
        Self: 'a;

    fn fill_payload(&mut self) -> Self::FillPayloadFuture<'_> {
        self.decoder.fill_payload()
    }
}

impl<IO> Stream for ClientCodec<IO>
where
    ResponseDecoder<OwnedReadHalf<IO>>: Stream,
{
    type Item = <ResponseDecoder<OwnedReadHalf<IO>> as Stream>::Item;

    type NextFuture<'a> = <ResponseDecoder<OwnedReadHalf<IO>> as Stream>::NextFuture<'a>
    where
        Self: 'a;

    fn next(&mut self) -> Self::NextFuture<'_> {
        self.decoder.next()
    }
}
