/// General payload representation.
use std::{
    cell::UnsafeCell,
    collections::VecDeque,
    io,
    rc::{Rc, Weak},
    task::Waker,
};

use bytes::Bytes;
use monoio::{io::stream::Stream, macros::support::poll_fn};
use thiserror::Error as ThisError;

#[derive(ThisError, Debug)]
pub enum PayloadError {
    #[error("unexpected eof")]
    UnexpectedEof,
    #[error("decode failed")]
    Decode,
    #[error("io error {0}")]
    Io(#[from] io::Error),
}

pub enum Payload<D = Bytes, E = PayloadError> {
    None,
    Fixed(FixedPayload<D, E>),
    Stream(StreamPayload<D, E>),
}

pub enum PayloadSender<D, E> {
    None,
    Fixed(FixedPayloadSender<D, E>, usize),
    Stream(StreamPayloadSender<D, E>),
}

impl<D, E> From<(FixedPayloadSender<D, E>, usize)> for PayloadSender<D, E> {
    fn from(inner: (FixedPayloadSender<D, E>, usize)) -> Self {
        Self::Fixed(inner.0, inner.1)
    }
}

impl<D, E> From<StreamPayloadSender<D, E>> for PayloadSender<D, E> {
    fn from(inner: StreamPayloadSender<D, E>) -> Self {
        Self::Stream(inner)
    }
}

impl<D, E> From<FixedPayload<D, E>> for Payload<D, E> {
    fn from(inner: FixedPayload<D, E>) -> Self {
        Self::Fixed(inner)
    }
}

impl<D, E> From<StreamPayload<D, E>> for Payload<D, E> {
    fn from(inner: StreamPayload<D, E>) -> Self {
        Self::Stream(inner)
    }
}

pub fn fixed_payload_pair<D, E>() -> (FixedPayload<D, E>, FixedPayloadSender<D, E>) {
    let inner = Rc::new(UnsafeCell::new(FixedInner::default()));
    let sender = FixedPayloadSender {
        inner: Rc::downgrade(&inner),
    };
    (FixedPayload { inner }, sender)
}

pub fn stream_payload_pair<D, E>() -> (StreamPayload<D, E>, StreamPayloadSender<D, E>) {
    let inner = Rc::new(UnsafeCell::new(StreamInner::default()));
    let sender = StreamPayloadSender {
        inner: Rc::downgrade(&inner),
    };
    (StreamPayload { inner }, sender)
}

/// Fixed Payload
#[derive(Debug)]
pub struct FixedPayload<D = Bytes, E = PayloadError> {
    inner: Rc<UnsafeCell<FixedInner<D, E>>>,
}

/// Sender part of the fixed payload
pub struct FixedPayloadSender<D = Bytes, E = PayloadError> {
    inner: Weak<UnsafeCell<FixedInner<D, E>>>,
}

#[derive(Debug)]
struct FixedInner<D, E> {
    item: Option<Result<D, E>>,
    task: Option<Waker>,
}

impl<D, E> Default for FixedInner<D, E> {
    fn default() -> Self {
        Self {
            item: None,
            task: None,
        }
    }
}

impl<D, E> FixedInner<D, E> {
    fn wake(&mut self) {
        if let Some(waker) = self.task.take() {
            waker.wake();
        }
    }
}

impl<D, E> FixedPayload<D, E> {
    pub fn new(data: D) -> Self {
        Self {
            inner: Rc::new(UnsafeCell::new(FixedInner {
                item: Some(Ok(data)),
                task: None,
            })),
        }
    }

    pub async fn get(self) -> Result<D, E> {
        loop {
            let inner = unsafe { &mut *self.inner.get() };
            if let Some(item) = inner.item.take() {
                return item;
            }
            poll_fn(|cx| {
                let inner = unsafe { &mut *self.inner.get() };
                if inner.item.is_some() {
                    std::task::Poll::Ready(())
                } else {
                    if !matches!(inner.task, Some(ref waker) if waker.will_wake(cx.waker())) {
                        inner.task = Some(cx.waker().clone());
                    }
                    std::task::Poll::Pending
                }
            })
            .await;
        }
    }
}

impl<D, E> FixedPayloadSender<D, E> {
    pub fn feed(self, item: Result<D, E>) {
        if let Some(shared) = self.inner.upgrade() {
            let inner = unsafe { &mut *shared.get() };
            inner.item = Some(item);
            inner.wake();
        }
    }
}

/// Stream Payload
#[derive(Debug)]
pub struct StreamPayload<D = Bytes, E = PayloadError> {
    inner: Rc<UnsafeCell<StreamInner<D, E>>>,
}

/// Sender part of the stream payload
pub struct StreamPayloadSender<D = Bytes, E = PayloadError> {
    inner: Weak<UnsafeCell<StreamInner<D, E>>>,
}

#[derive(Debug)]
struct StreamInner<D, E> {
    eof: bool,
    items: VecDeque<Result<D, E>>,
    task: Option<Waker>,
}

impl<D, E> Default for StreamInner<D, E> {
    fn default() -> Self {
        Self {
            eof: false,
            items: VecDeque::new(),
            task: None,
        }
    }
}

impl<D, E> StreamInner<D, E> {
    fn wake(&mut self) {
        if let Some(waker) = self.task.take() {
            waker.wake();
        }
    }
}

impl<D, E> Stream for StreamPayload<D, E> {
    type Item = Result<D, E>;

    type NextFuture<'a> = impl std::future::Future<Output = Option<Self::Item>> + 'a where Self:'a;

    fn next(&mut self) -> Self::NextFuture<'_> {
        async move {
            loop {
                let inner = unsafe { &mut *self.inner.get() };
                if let Some(data) = inner.items.pop_front() {
                    return Some(data);
                }
                if inner.eof {
                    return None;
                }
                poll_fn(|cx| {
                    let inner = unsafe { &mut *self.inner.get() };
                    if inner.eof || !inner.items.is_empty() {
                        std::task::Poll::Ready(())
                    } else {
                        if !matches!(inner.task, Some(ref waker) if waker.will_wake(cx.waker())) {
                            inner.task = Some(cx.waker().clone());
                        }
                        std::task::Poll::Pending
                    }
                })
                .await;
            }
        }
    }
}

impl<D, E> StreamPayloadSender<D, E> {
    pub fn feed_data(&mut self, data: Option<D>) {
        if let Some(shared) = self.inner.upgrade() {
            let inner = unsafe { &mut *shared.get() };
            match data {
                Some(d) => inner.items.push_back(Ok(d)),
                None => inner.eof = true,
            }
            inner.wake();
        }
    }

    pub fn feed_error(&mut self, err: E) {
        if let Some(shared) = self.inner.upgrade() {
            let inner = unsafe { &mut *shared.get() };
            inner.items.push_back(Err(err));
            inner.wake();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{convert::Infallible, io, time::Duration};

    use bytes::Bytes;

    use super::*;

    #[monoio::test_all(enable_timer = true)]
    async fn stream_payload() {
        let (mut payload, mut payload_sender) = stream_payload_pair();
        monoio::spawn(async move {
            monoio::time::sleep(Duration::from_millis(2)).await;
            payload_sender.feed_data(Some(Bytes::from_static(b"Hello")));
            monoio::time::sleep(Duration::from_millis(2)).await;
            payload_sender.feed_data(Some(Bytes::from_static(b"World")));
            payload_sender.feed_error(io::Error::new(io::ErrorKind::Other, "oops"));
            payload_sender.feed_data(None);
        });
        assert_eq!(
            payload.next().await.unwrap().unwrap(),
            Bytes::from_static(b"Hello")
        );
        assert_eq!(
            payload.next().await.unwrap().unwrap(),
            Bytes::from_static(b"World")
        );
        assert!(payload.next().await.unwrap().is_err());
        assert!(payload.next().await.is_none());
        assert!(payload.next().await.is_none());
    }

    #[monoio::test_all(enable_timer = true)]
    async fn fixed_payload() {
        let (payload, payload_sender) = fixed_payload_pair::<_, Infallible>();
        monoio::spawn(async move {
            monoio::time::sleep(Duration::from_millis(2)).await;
            payload_sender.feed(Ok(Bytes::from_static(b"Hello")));
        });
        assert_eq!(payload.get().await.unwrap(), Bytes::from_static(b"Hello"));

        let (payload, payload_sender) = fixed_payload_pair::<_, Infallible>();
        payload_sender.feed(Ok(Bytes::from_static(b"World")));
        assert_eq!(payload.get().await.unwrap(), Bytes::from_static(b"World"));
    }
}
