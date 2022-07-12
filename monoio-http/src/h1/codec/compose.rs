use std::hint::unreachable_unchecked;

use monoio_codec::Decoder;

use crate::h1::payload::{FixedPayloadSender, StreamPayloadSender};

/// NextDecoder maybe None, Fixed or Streamed.
/// Mainly designed for no body, fixed-length body and chunked body.
/// But generally, NextDecoder can be used to represent 0, 1, or more
/// than 1 things to decode.
pub enum NextDecoder<FDE, SDE, BI>
where
    FDE: Decoder<Item = BI>,
    SDE: Decoder<Item = Option<BI>>,
{
    None,
    Fixed(FDE, FixedPayloadSender<BI, FDE::Error>),
    Streamed(SDE, StreamPayloadSender<BI, SDE::Error>),
}

impl<FDE, SDE, BI> Default for NextDecoder<FDE, SDE, BI>
where
    FDE: Decoder<Item = BI>,
    SDE: Decoder<Item = Option<BI>>,
{
    fn default() -> Self {
        NextDecoder::None
    }
}

/// ComposeDecoder is a wrapper of 3 codecs(Main decoder, fixed decoder
/// and streamed decoder).
/// It will first use the main codec. If the decoded result suggests that
/// we should use another decoder to create a single thing or stream, we
/// will do it later. When the streamed decoder returns EOF, or fixed
/// decoder returns something, we will mark the stream end and go back
/// to use the main codec.
///
/// Note: Streamed decoder's Item should be an Option which None means EOF.
/// Main codec Item should be (Item, NextDecoder<FDE, SDE, BI>).
/// And the last but least, the decode or next is the only reader of the
/// io, if you do not call framed.next, the payload.next will be blocked.
/// Basically there are two ways:
/// 1. Manually create and poll future(like with select!). Make sure you
/// poll the future till the Ready.
/// 2. Spawn the framed.next task and use channel. Then you can select!
/// rx.recv() and request handler.
pub struct ComposeDecoder<DE, I, FDE, SDE, BI>
where
    DE: Decoder<Item = (I, NextDecoder<FDE, SDE, BI>)>,
    FDE: Decoder<Item = BI>,
    SDE: Decoder<Item = Option<BI>>,
{
    decoder: DE,
    next_decoder: NextDecoder<FDE, SDE, BI>,
}

pub enum DecodeItem<T> {
    /// Head is the http header.
    Head(T),
    /// Body means we have got the last body(ok or error) and sent.
    Body,
}

impl<DE, I, FDE, SDE, BI> Decoder for ComposeDecoder<DE, I, FDE, SDE, BI>
where
    DE: Decoder<Item = (I, NextDecoder<FDE, SDE, BI>)>,
    FDE: Decoder<Item = BI>,
    SDE: Decoder<Item = Option<BI>>,
{
    type Item = DecodeItem<I>;
    type Error = DE::Error;

    fn decode(&mut self, src: &mut bytes::BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        loop {
            match &mut self.next_decoder {
                NextDecoder::None => break,
                NextDecoder::Fixed(decoder, _) => match decoder.decode(src) {
                    Ok(Some(item)) => {
                        let sender =
                            match std::mem::replace(&mut self.next_decoder, NextDecoder::None) {
                                NextDecoder::Fixed(_, s) => s,
                                // # Safety: We already make sure the branch is Fixed
                                _ => unsafe { unreachable_unchecked() },
                            };
                        sender.feed(Ok(item));
                        return Ok(Some(DecodeItem::Body));
                    }
                    Ok(None) => {
                        return Ok(None);
                    }
                    Err(e) => {
                        let sender =
                            match std::mem::replace(&mut self.next_decoder, NextDecoder::None) {
                                NextDecoder::Fixed(_, s) => s,
                                // # Safety: We already make sure the branch is Fixed
                                _ => unsafe { unreachable_unchecked() },
                            };
                        sender.feed(Err(e));
                        return Ok(Some(DecodeItem::Body));
                    }
                },
                NextDecoder::Streamed(decoder, sender) => match decoder.decode(src) {
                    Ok(Some(maybe_item)) => match maybe_item {
                        Some(item) => {
                            sender.feed_data(Some(item));
                        }
                        None => {
                            sender.feed_data(None);
                            self.next_decoder = NextDecoder::None;
                            return Ok(Some(DecodeItem::Body));
                        }
                    },
                    Ok(None) => {
                        return Ok(None);
                    }
                    Err(e) => {
                        sender.feed_error(e);
                        self.next_decoder = NextDecoder::None;
                        return Ok(Some(DecodeItem::Body));
                    }
                },
            }
        }

        // All foreign data has been processed now.
        let (item, next) = match self.decoder.decode(src) {
            Ok(Some(r)) => r,
            Ok(None) => return Ok(None),
            Err(e) => return Err(e),
        };
        self.next_decoder = next;
        Ok(Some(DecodeItem::Head(item)))
    }
}

impl<DE, I, FDE, SDE, BI> ComposeDecoder<DE, I, FDE, SDE, BI>
where
    DE: Decoder<Item = (I, NextDecoder<FDE, SDE, BI>)>,
    FDE: Decoder<Item = BI>,
    SDE: Decoder<Item = Option<BI>>,
{
    pub fn new(decoder: DE) -> Self {
        Self {
            decoder,
            next_decoder: NextDecoder::None,
        }
    }
}

impl<DE, I, FDE, SDE, BI> Default for ComposeDecoder<DE, I, FDE, SDE, BI>
where
    DE: Decoder<Item = (I, NextDecoder<FDE, SDE, BI>)> + Default,
    FDE: Decoder<Item = BI>,
    SDE: Decoder<Item = Option<BI>>,
{
    fn default() -> Self {
        Self::new(Default::default())
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::VecDeque, io};

    use bytes::{Buf, BytesMut};
    use monoio::{
        buf::IoVecWrapperMut,
        io::{stream::Stream, AsyncReadRent},
    };
    use monoio_codec::{Decoder, FramedRead};

    use crate::h1::payload::{fixed_payload_pair, stream_payload_pair, Payload};

    use super::*;

    macro_rules! mock {
        ($($x:expr),*) => {{
            let mut v = VecDeque::new();
            v.extend(vec![$($x),*]);
            Mock { calls: v }
        }};
    }

    // In our test, we will simulate http verb:
    // G for get, without body(for simple, the header data length must be 2)
    // P for post(for simple, the header data length must be 2),
    //  if the first byte is 'c', the body is in chunked, otherwise means fixed 4 byte body.
    // C for chunk with data(for simple, the data length must be 4)
    // E for eof chunk(no data)
    #[monoio::test_all]
    async fn dispatch() {
        let mock = mock! {Ok(b"GzzPc.C....C....EG..P..BODY".to_vec())};
        let codec = ComposeDecoder::new(HeaderDecoder);
        let mut framed = FramedRead::new(mock, codec);

        // Get
        assert!(matches!(
            framed.next().await.unwrap().unwrap(),
            DecodeItem::Head(Request::Get([b'z', b'z']))
        ));

        // Post chunked
        let post = framed.next().await.unwrap().unwrap();
        let post = match post {
            DecodeItem::Head(h) => h,
            DecodeItem::Body => panic!("unexpected type"),
        };
        let task = match post {
            Request::Get(_) => panic!("unexpected"),
            Request::Post(data, payload) => {
                // We must spawn here since if we do not call next, the await
                // here will always blocked.
                monoio::spawn(async move {
                    let mut payload = match payload {
                        Payload::None => panic!("payload should not be None"),
                        Payload::Fixed(_) => panic!("payload should not be Fixed"),
                        Payload::Stream(s) => s,
                    };
                    assert_eq!(data, [b'c', b'.']);
                    let chunk1 = payload.next().await.unwrap().unwrap();
                    assert_eq!(chunk1, [b'.', b'.', b'.', b'.']);
                    let chunk2 = payload.next().await.unwrap().unwrap();
                    assert_eq!(chunk2, [b'.', b'.', b'.', b'.']);
                    let chunk3 = payload.next().await;
                    assert!(chunk3.is_none());
                })
            }
        };
        assert!(matches!(
            framed.next().await.unwrap().unwrap(),
            DecodeItem::Body
        ));

        // Get again
        assert!(matches!(
            framed.next().await.unwrap().unwrap(),
            DecodeItem::Head(Request::Get([b'.', b'.']))
        ));
        task.await;

        // Post fixed
        let post = framed.next().await.unwrap().unwrap();
        let post = match post {
            DecodeItem::Head(h) => h,
            DecodeItem::Body => panic!("unexpected type"),
        };
        let task = match post {
            Request::Get(_) => panic!("unexpected"),
            Request::Post(data, payload) => {
                // We must spawn here since if we do not call next, the await
                // here will always blocked.
                monoio::spawn(async move {
                    let payload = match payload {
                        Payload::None => panic!("payload should not be None"),
                        Payload::Fixed(f) => f,
                        Payload::Stream(_) => panic!("payload should not be Stream"),
                    };
                    assert_eq!(data, [b'.', b'.']);
                    let payload = payload.get().await.unwrap();
                    assert_eq!(payload, [b'B', b'O', b'D', b'Y']);
                })
            }
        };
        assert!(matches!(
            framed.next().await.unwrap().unwrap(),
            DecodeItem::Body
        ));
        assert!(framed.next().await.is_none());
        task.await;
    }

    enum Request {
        Get([u8; 2]),
        Post([u8; 2], Payload<[u8; 4], io::Error>),
    }

    struct HeaderDecoder;

    impl Decoder for HeaderDecoder {
        type Item = (Request, NextDecoder<FixedDecoder, ChunkedDecoder, [u8; 4]>);
        type Error = io::Error;

        fn decode(&mut self, buf: &mut BytesMut) -> io::Result<Option<Self::Item>> {
            if buf.len() < 3 {
                return Ok(None);
            }

            let mut data: [u8; 2] = [0, 0];
            data.copy_from_slice(&buf[1..3]);
            match buf[0] {
                b'G' => {
                    buf.advance(3);
                    Ok(Some((Request::Get(data), NextDecoder::None)))
                }
                b'P' => {
                    buf.advance(3);
                    if data[0] == b'c' {
                        // chunked
                        let (payload, payload_sender) = stream_payload_pair();
                        Ok(Some((
                            Request::Post(data, Payload::Stream(payload)),
                            NextDecoder::Streamed(ChunkedDecoder, payload_sender),
                        )))
                    } else {
                        let (payload, payload_sender) = fixed_payload_pair();
                        Ok(Some((
                            Request::Post(data, Payload::Fixed(payload)),
                            NextDecoder::Fixed(FixedDecoder(4), payload_sender),
                        )))
                    }
                }
                _ => Err(io::Error::new(
                    io::ErrorKind::Other,
                    "unexpected magic number",
                )),
            }
        }
    }

    struct FixedDecoder(usize);

    impl Decoder for FixedDecoder {
        type Item = [u8; 4];
        type Error = io::Error;

        fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
            if src.len() < 4 {
                return Ok(None);
            }
            let mut data: [u8; 4] = [0, 0, 0, 0];
            data.copy_from_slice(&src[..4]);
            src.advance(4);
            Ok(Some(data))
        }
    }

    struct ChunkedDecoder;

    impl Decoder for ChunkedDecoder {
        type Item = Option<[u8; 4]>;
        type Error = io::Error;

        fn decode(&mut self, src: &mut BytesMut) -> io::Result<Option<Self::Item>> {
            if src.is_empty() {
                return Ok(None);
            }
            match src[0] {
                b'C' => {
                    if src.len() < 5 {
                        return Ok(None);
                    }
                    let mut data: [u8; 4] = [0, 0, 0, 0];
                    data.copy_from_slice(&src[1..5]);
                    src.advance(5);
                    Ok(Some(Some(data)))
                }
                b'E' => {
                    src.advance(1);
                    Ok(Some(None))
                }
                _ => Err(io::Error::new(
                    io::ErrorKind::Other,
                    "unexpected magic number",
                )),
            }
        }
    }

    // Mock struct copied from monoio-codec and tokio-util.
    struct Mock {
        calls: VecDeque<io::Result<Vec<u8>>>,
    }

    impl AsyncReadRent for Mock {
        type ReadFuture<'a, B> = impl std::future::Future<Output = monoio::BufResult<usize, B>> where
            B: 'a;
        type ReadvFuture<'a, B> = impl std::future::Future<Output = monoio::BufResult<usize, B>> where
            B: 'a;

        fn read<T: monoio::buf::IoBufMut>(&mut self, mut buf: T) -> Self::ReadFuture<'_, T> {
            async {
                match self.calls.pop_front() {
                    Some(Ok(data)) => {
                        let n = data.len();
                        debug_assert!(buf.bytes_total() >= n);
                        unsafe {
                            buf.write_ptr().copy_from_nonoverlapping(data.as_ptr(), n);
                            buf.set_init(n)
                        }
                        (Ok(n), buf)
                    }
                    Some(Err(e)) => (Err(e), buf),
                    None => (Ok(0), buf),
                }
            }
        }

        fn readv<T: monoio::buf::IoVecBufMut>(&mut self, mut buf: T) -> Self::ReadvFuture<'_, T> {
            async move {
                let slice = match IoVecWrapperMut::new(buf) {
                    Ok(slice) => slice,
                    Err(buf) => return (Ok(0), buf),
                };

                let (result, slice) = self.read(slice).await;
                buf = slice.into_inner();
                if let Ok(n) = result {
                    unsafe { buf.set_init(n) };
                }
                (result, buf)
            }
        }
    }
}
