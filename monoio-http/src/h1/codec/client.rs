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

    #[inline]
    async fn send(&mut self, item: R) -> Result<(), Self::Error> {
        self.encoder.send(item).await
    }

    #[inline]
    async fn flush(&mut self) -> Result<(), Self::Error> {
        self.encoder.flush().await
    }

    #[inline]
    async fn close(&mut self) -> Result<(), Self::Error> {
        self.encoder.close().await
    }
}

impl<IO: AsyncReadRent + AsyncWriteRent> Stream for ClientCodec<IO>
where
    ClientResponseDecoder<OwnedReadHalf<IO>>: Stream,
{
    type Item = <ClientResponseDecoder<OwnedReadHalf<IO>> as Stream>::Item;

    #[inline]
    async fn next(&mut self) -> Option<Self::Item> {
        self.decoder.next().await
    }
}
