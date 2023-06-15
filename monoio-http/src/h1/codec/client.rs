use monoio::io::{
    sink::Sink, stream::Stream, AsyncReadRent, AsyncWriteRent, OwnedReadHalf, OwnedWriteHalf,
    Split, Splitable,
};
use monoio_codec::FramedRead;

use super::{decoder::ClientResponseDecoder, encoder::GenericEncoder};
use crate::h1::BorrowFramedRead;

pub struct ClientCodec<IO: AsyncWriteRent> {
    encoder: GenericEncoder<OwnedWriteHalf<IO>>,
    decoder: ClientResponseDecoder<OwnedReadHalf<IO>>,
}

impl<IO: Split + AsyncReadRent + AsyncWriteRent> ClientCodec<IO> {
    pub fn new(io: IO) -> Self {
        // # Safety: Since we will not use the encoder and decoder at once, we can split it safely.
        let (r, w) = io.into_split();
        Self {
            encoder: GenericEncoder::new(w),
            decoder: ClientResponseDecoder::new(r),
        }
    }
}

impl<IO: AsyncWriteRent> BorrowFramedRead for ClientCodec<IO>
where
    ClientResponseDecoder<OwnedReadHalf<IO>>: BorrowFramedRead,
{
    type IO = <ClientResponseDecoder<OwnedReadHalf<IO>> as BorrowFramedRead>::IO;
    type Codec = <ClientResponseDecoder<OwnedReadHalf<IO>> as BorrowFramedRead>::Codec;

    fn framed_mut(&mut self) -> &mut FramedRead<Self::IO, Self::Codec> {
        self.decoder.framed_mut()
    }
}

impl<IO: AsyncWriteRent, R> Sink<R> for ClientCodec<IO>
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

    #[inline]
    fn send<'a>(&'a mut self, item: R) -> Self::SendFuture<'a>
    where
        R: 'a,
    {
        self.encoder.send(item)
    }

    #[inline]
    fn flush(&mut self) -> Self::FlushFuture<'_> {
        self.encoder.flush()
    }

    #[inline]
    fn close(&mut self) -> Self::CloseFuture<'_> {
        self.encoder.close()
    }
}

impl<IO: AsyncReadRent + AsyncWriteRent> Stream for ClientCodec<IO>
where
    ClientResponseDecoder<OwnedReadHalf<IO>>: Stream,
{
    type Item = <ClientResponseDecoder<OwnedReadHalf<IO>> as Stream>::Item;

    type NextFuture<'a> = <ClientResponseDecoder<OwnedReadHalf<IO>> as Stream>::NextFuture<'a>
    where
        Self: 'a;

    #[inline]
    fn next(&mut self) -> Self::NextFuture<'_> {
        self.decoder.next()
    }
}
