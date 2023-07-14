use std::{
    cell::UnsafeCell,
    io::{self, Cursor},
    task::{Context, Poll},
};

use bytes::{Buf, BufMut, Bytes, BytesMut};
use monoio::{
    buf::{IoBufMut, IoVecBufMut},
    io::{AsyncReadRent, AsyncWriteRent, AsyncWriteRentExt},
    BufResult,
};
use monoio_compat::box_future::MaybeArmedBoxFuture;

use crate::h2::{
    codec::{UserError, UserError::*},
    frame::{self, Frame, FrameSize},
    hpack,
};

// A macro to get around a method needing to borrow &mut self
macro_rules! limited_write_buf {
    ($self:expr) => {{
        let limit = $self.max_frame_size() + frame::HEADER_LEN;
        $self.buf.get_mut().limit(limit)
    }};
}

#[derive(Debug)]
pub struct FramedWrite<T, B> {
    encoder: Encoder<B>,
    write_fut: MaybeArmedBoxFuture<BufResult<usize, Bytes>>,
    flush_fut: MaybeArmedBoxFuture<io::Result<()>>,
    shut_fut: MaybeArmedBoxFuture<io::Result<()>>,
    /// Upstream `AsyncWriteRent`
    // Put it at the last to make sure futures depending on it drop first.
    inner: UnsafeCell<T>,
}

#[derive(Debug)]
struct Encoder<B> {
    /// HPACK encoder
    hpack: hpack::Encoder,

    /// Write buffer
    ///
    /// TODO: Should this be a ring buffer?
    buf: Cursor<BytesMut>,

    /// Next frame to encode
    next: Option<Next<B>>,

    /// Last data frame
    last_data_frame: Option<frame::Data<B>>,

    /// Max frame size, this is specified by the peer
    max_frame_size: FrameSize,
}

#[derive(Debug)]
enum Next<B> {
    Data(frame::Data<B>),
    Continuation(frame::Continuation),
}

/// Initialize the connection with this amount of write buffer.
///
/// The minimum MAX_FRAME_SIZE is 16kb, so always be able to send a HEADERS
/// frame that big.
const DEFAULT_BUFFER_CAPACITY: usize = 16 * 1_024;

/// Min buffer required to attempt to write a frame
const MIN_BUFFER_CAPACITY: usize = frame::HEADER_LEN + CHAIN_THRESHOLD;

/// Chain payloads bigger than this. The remote will never advertise a max frame
/// size less than this (well, the spec says the max frame size can't be less
/// than 16kb, so not even close).
const CHAIN_THRESHOLD: usize = 256;

// TODO: Make generic
impl<T, B> FramedWrite<T, B>
where
    T: AsyncWriteRent + Unpin + 'static,
    B: Buf,
{
    pub fn new(inner: T) -> FramedWrite<T, B> {
        FramedWrite {
            inner: UnsafeCell::new(inner),
            encoder: Encoder {
                hpack: hpack::Encoder::default(),
                buf: Cursor::new(BytesMut::with_capacity(DEFAULT_BUFFER_CAPACITY)),
                next: None,
                last_data_frame: None,
                max_frame_size: frame::DEFAULT_MAX_FRAME_SIZE,
                // is_write_vectored,
            },
            write_fut: Default::default(),
            flush_fut: Default::default(),
            shut_fut: Default::default(),
        }
    }

    /// Returns `Ready` when `send` is able to accept a frame
    ///
    /// Calling this function may result in the current contents of the buffer
    /// to be flushed to `T`.
    pub fn poll_ready(&mut self, cx: &mut Context) -> Poll<io::Result<()>> {
        if !self.encoder.has_capacity() {
            // Try flushing
            ready!(self.flush(cx))?;

            if !self.encoder.has_capacity() {
                return Poll::Pending;
            }
        }

        Poll::Ready(Ok(()))
    }

    /// Buffer a frame.
    ///
    /// `poll_ready` must be called first to ensure that a frame may be
    /// accepted.
    pub fn buffer(&mut self, item: Frame<B>) -> Result<(), UserError> {
        self.encoder.buffer(item)
    }

    /// Flush buffered data to the wire
    pub fn flush(&mut self, cx: &mut Context) -> Poll<io::Result<()>> {
        let span = tracing::trace_span!("FramedWrite::flush");
        let _e = span.enter();

        if self.write_fut.armed() {
            let (result, _) = ready!(self.write_fut.poll(cx));
            tracing::trace!("Framed write bytes written = {}", result?);
        }

        loop {
            tracing::trace!("Entering loop::{}", self.encoder.is_empty());

            while !self.encoder.is_empty() {
                match self.encoder.next {
                    Some(Next::Data(ref mut frame)) => {
                        tracing::trace!(queued_data_frame = true);

                        // Take ownership of the encoder's buffer
                        let encoder_buf = self.encoder.buf.get_mut();
                        let owned_encoder_buf = std::mem::replace(encoder_buf, BytesMut::new());
                        let encoder_buf_bytes = owned_encoder_buf.freeze();

                        // Not a deep copy, just creates a new Bytes instance
                        let frame_payload = frame.payload_mut();
                        let frame_payload_bytes =
                            frame_payload.copy_to_bytes(frame_payload.remaining());

                        let total_bytes =
                            frame_payload_bytes.remaining() + encoder_buf_bytes.remaining();
                        tracing::trace!(
                            "Total bytes to be written for data frame:: {}",
                            total_bytes
                        );

                        let io = unsafe { &mut *(self.inner.get()) };
                        let write_fut = async move {
                            let res1 = io.write_all(encoder_buf_bytes).await;
                            let res2 = io.write_all(frame_payload_bytes).await;

                            match (res1, res2) {
                                ((Ok(w1), buf1), (Ok(w2), _buf2)) => (Ok(w1 + w2), buf1),
                                ((Err(err), buf), (_, _)) | ((_, _), (Err(err), buf)) => {
                                    (Err(err), buf)
                                }
                            }
                        };

                        self.write_fut.arm_future(write_fut);

                        let (result, _) = ready!(self.write_fut.poll(cx));
                        result?;
                    }
                    _ => {
                        tracing::trace!(queued_data_frame = false);

                        let encoder_buf = self.encoder.buf.get_mut();
                        let owned_encoder_buf = std::mem::replace(encoder_buf, BytesMut::new());
                        let encoder_buf_bytes = owned_encoder_buf.freeze();

                        tracing::trace!(
                            "Total bytes to be written for non data frame:: {}",
                            encoder_buf_bytes.remaining()
                        );

                        let io = unsafe { &mut *(self.inner.get()) };
                        self.write_fut.arm_future(io.write_all(encoder_buf_bytes));

                        let (result, _) = ready!(self.write_fut.poll(cx));
                        result?;
                    }
                }
            }

            match self.encoder.unset_frame() {
                ControlFlow::Continue => (),
                ControlFlow::Break => break,
            }
        }

        tracing::trace!("flushing buffer io");
        if !self.flush_fut.armed() {
            let io = unsafe { &mut *(self.inner.get()) };
            tracing::trace!("Framed write Flush returning");
            self.flush_fut.arm_future(io.flush());
        }

        ready!(self.flush_fut.poll(cx))?;

        Poll::Ready(Ok(()))
    }

    /// Close the codec
    pub fn shutdown(&mut self, cx: &mut Context) -> Poll<io::Result<()>> {
        ready!(self.flush(cx))?;

        if !self.shut_fut.armed() {
            let io = unsafe { &mut *(self.inner.get()) };
            self.flush_fut.arm_future(io.shutdown());
        }

        ready!(self.shut_fut.poll(cx))?;

        Poll::Ready(Ok(()))
    }
}

#[must_use]
enum ControlFlow {
    Continue,
    Break,
}

impl<B> Encoder<B>
where
    B: Buf,
{
    fn unset_frame(&mut self) -> ControlFlow {
        // Clear internal buffer
        self.buf.set_position(0);
        self.buf.get_mut().clear();

        // The data frame has been written, so unset it
        match self.next.take() {
            Some(Next::Data(frame)) => {
                self.last_data_frame = Some(frame);
                debug_assert!(self.is_empty());
                ControlFlow::Break
            }
            Some(Next::Continuation(frame)) => {
                // Buffer the continuation frame, then try to write again
                let mut buf = limited_write_buf!(self);
                if let Some(continuation) = frame.encode(&mut buf) {
                    self.next = Some(Next::Continuation(continuation));
                }
                ControlFlow::Continue
            }
            None => ControlFlow::Break,
        }
    }

    fn buffer(&mut self, item: Frame<B>) -> Result<(), UserError> {
        // Ensure that we have enough capacity to accept the write.
        assert!(self.has_capacity());
        let span = tracing::trace_span!("FramedWrite::buffer", frame = ?item);
        let _e = span.enter();

        tracing::debug!(frame = ?item, "send");

        match item {
            Frame::Data(mut v) => {
                // Ensure that the payload is not greater than the max frame.
                let len = v.payload().remaining();

                if len > self.max_frame_size() {
                    return Err(PayloadTooBig);
                }

                if len >= CHAIN_THRESHOLD {
                    let head = v.head();

                    // Encode the frame head to the buffer
                    head.encode(len, self.buf.get_mut());

                    // Save the data frame
                    self.next = Some(Next::Data(v));
                } else {
                    v.encode_chunk(self.buf.get_mut());

                    // The chunk has been fully encoded, so there is no need to
                    // keep it around
                    assert_eq!(v.payload().remaining(), 0, "chunk not fully encoded");

                    // Save off the last frame...
                    self.last_data_frame = Some(v);
                }
            }
            Frame::Headers(v) => {
                let mut buf = limited_write_buf!(self);
                if let Some(continuation) = v.encode(&mut self.hpack, &mut buf) {
                    self.next = Some(Next::Continuation(continuation));
                }
            }
            Frame::PushPromise(v) => {
                let mut buf = limited_write_buf!(self);
                if let Some(continuation) = v.encode(&mut self.hpack, &mut buf) {
                    self.next = Some(Next::Continuation(continuation));
                }
            }
            Frame::Settings(v) => {
                v.encode(self.buf.get_mut());
                tracing::trace!(rem = self.buf.remaining(), "encoded settings");
            }
            Frame::GoAway(v) => {
                v.encode(self.buf.get_mut());
                tracing::trace!(rem = self.buf.remaining(), "encoded go_away");
            }
            Frame::Ping(v) => {
                v.encode(self.buf.get_mut());
                tracing::trace!(rem = self.buf.remaining(), "encoded ping");
            }
            Frame::WindowUpdate(v) => {
                v.encode(self.buf.get_mut());
                tracing::trace!(rem = self.buf.remaining(), "encoded window_update");
            }

            Frame::Priority(_) => {
                // v.encode(self.buf.get_mut());
                // tracing::trace!("encoded priority; rem={:?}", self.buf.remaining());
                unimplemented!();
            }
            Frame::Reset(v) => {
                v.encode(self.buf.get_mut());
                tracing::trace!(rem = self.buf.remaining(), "encoded reset");
            }
        }

        Ok(())
    }

    fn has_capacity(&self) -> bool {
        self.next.is_none() && self.buf.get_ref().remaining_mut() >= MIN_BUFFER_CAPACITY
    }

    fn is_empty(&self) -> bool {
        match self.next {
            Some(Next::Data(ref frame)) => !frame.payload().has_remaining(),
            _ => !self.buf.has_remaining(),
        }
    }
}

impl<B> Encoder<B> {
    fn max_frame_size(&self) -> usize {
        self.max_frame_size as usize
    }
}

impl<T, B> FramedWrite<T, B> {
    /// Returns the max frame size that can be sent
    pub fn max_frame_size(&self) -> usize {
        self.encoder.max_frame_size()
    }

    /// Set the peer's max frame size.
    pub fn set_max_frame_size(&mut self, val: usize) {
        assert!(val <= frame::MAX_MAX_FRAME_SIZE as usize);
        self.encoder.max_frame_size = val as FrameSize;
    }

    /// Set the peer's header table size.
    pub fn set_header_table_size(&mut self, val: usize) {
        self.encoder.hpack.update_max_size(val);
    }

    /// Retrieve the last data frame that has been sent
    pub fn take_last_data_frame(&mut self) -> Option<frame::Data<B>> {
        self.encoder.last_data_frame.take()
    }

    pub fn get_mut(&mut self) -> &mut T {
        unsafe { &mut *(self.inner.get()) }
    }
}

impl<T: AsyncReadRent + Unpin, B> AsyncReadRent for FramedWrite<T, B> {
    type ReadFuture<'a, E> = impl std::future::Future<Output = monoio::BufResult<usize, E>> + 'a where
    Self: 'a,
    E: IoBufMut + 'a;
    type ReadvFuture<'a, E> = impl std::future::Future<Output = monoio::BufResult<usize, E>> + 'a where
    Self: 'a,
    E: IoVecBufMut + 'a;

    fn read<F: IoBufMut>(&mut self, buf: F) -> Self::ReadFuture<'_, F> {
        async move { self.inner.get_mut().read(buf).await }
    }

    fn readv<F: IoVecBufMut>(&mut self, buf: F) -> Self::ReadvFuture<'_, F> {
        async move { self.inner.get_mut().readv(buf).await }
    }
}

// We never project the Pin to `B`.
impl<T: Unpin, B> Unpin for FramedWrite<T, B> {}

#[cfg(feature = "unstable")]
mod unstable {
    use super::*;

    impl<T, B> FramedWrite<T, B> {
        pub fn get_ref(&self) -> &T {
            unsafe { &*self.inner.get() }
        }
    }
}
