use std::{
    cell::UnsafeCell,
    future::Future,
    pin::Pin,
    rc::Rc,
    task::{Context, Poll, Waker},
};

macro_rules! set_waker {
    ($slot: expr, $waker: expr) => {
        match $slot {
            Some(existing) => {
                if !existing.will_wake($waker) {
                    *existing = $waker.clone();
                }
            }
            None => {
                *$slot = Some($waker.clone());
            }
        }
    };
}

/// SPSCSender
pub struct SPSCSender<T> {
    inner: Rc<UnsafeCell<Shared<T>>>,
}

/// SPSCReceiver
pub struct SPSCReceiver<T> {
    inner: Rc<UnsafeCell<Shared<T>>>,
}

pub fn spsc_pair<T>() -> (SPSCSender<T>, SPSCReceiver<T>) {
    let inner = Rc::new(UnsafeCell::new(Shared::default()));
    (
        SPSCSender {
            inner: inner.clone(),
        },
        SPSCReceiver { inner },
    )
}

struct Shared<T> {
    slot: Option<T>,
    closed: bool,
    receiver_waker: Option<Waker>,
    sender_waker: Option<Waker>,
    close_waker: Option<Waker>,
}

impl<T> Default for Shared<T> {
    fn default() -> Self {
        Self {
            slot: None,
            closed: false,
            receiver_waker: None,
            sender_waker: None,
            close_waker: None,
        }
    }
}

impl<T> SPSCReceiver<T> {
    /// None: closed; Some: item
    pub fn poll_recv(&mut self, cx: &Context<'_>) -> Poll<Option<T>> {
        let inner = unsafe { &mut *self.inner.get() };
        // If we can get something from slot:
        //  1. take it and clear receiver_waker
        //  2. wake sender_waker.
        if let Some(item) = inner.slot.take() {
            inner.receiver_waker = None;
            if let Some(sender_waker) = inner.sender_waker.take() {
                sender_waker.wake();
            }
            return Poll::Ready(Some(item));
        }
        // If closed:
        //  1. return None
        if inner.closed {
            return Poll::Ready(None);
        }
        // Wait
        set_waker!(&mut inner.receiver_waker, cx.waker());
        Poll::Pending
    }

    pub fn recv(&mut self) -> Recv<'_, T> {
        Recv { receiver: self }
    }
}

impl<T> Drop for SPSCReceiver<T> {
    fn drop(&mut self) {
        let inner = unsafe { &mut *self.inner.get() };
        inner.closed = true;
        inner.receiver_waker = None;
        if let Some(waker) = inner.sender_waker.take() {
            waker.wake();
        }
        if let Some(waker) = inner.close_waker.take() {
            waker.wake();
        }
    }
}

pub struct Recv<'a, T> {
    receiver: &'a mut SPSCReceiver<T>,
}

impl<T> Future for Recv<'_, T> {
    type Output = Option<T>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        Pin::into_inner(self).receiver.poll_recv(cx)
    }
}

impl<T> SPSCSender<T> {
    /// None: closed; Some: the slot is empty
    pub fn poll_ready(&mut self, cx: &Context<'_>) -> Poll<Option<()>> {
        let inner = unsafe { &mut *self.inner.get() };
        // If closed:
        //  1. return None
        if inner.closed {
            return Poll::Ready(None);
        }
        // If slot is full:
        //  1. Wait
        if inner.slot.is_some() {
            set_waker!(&mut inner.sender_waker, cx.waker());
            return Poll::Pending;
        }
        // Slot now is empty
        //  1. Poll::Ready(Some(()))
        Poll::Ready(Some(()))
    }

    pub fn poll_closed(&mut self, cx: &Context<'_>) -> Poll<()> {
        let inner = unsafe { &mut *self.inner.get() };
        // If closed:
        //  1. return
        if inner.closed {
            return Poll::Ready(());
        }
        set_waker!(&mut inner.close_waker, cx.waker());
        Poll::Pending
    }

    /// Send item.
    /// You should always poll_ready, when Poll::Ready(Some(())) is returned
    /// user can then call send.
    /// Note: without calling poll_ready, the behavior is undefined.
    pub fn do_send(&mut self, item: T) {
        let inner = unsafe { &mut *self.inner.get() };
        // Put item into slot and wake receiver_waker.
        inner.slot = Some(item);
        if let Some(waker) = inner.receiver_waker.take() {
            waker.wake();
        }
    }

    pub fn send(&mut self, item: T) -> Send<'_, T> {
        Send {
            sender: self,
            item: Some(item),
        }
    }

    pub fn closed(&mut self) -> Closed<'_, T> {
        Closed { sender: self }
    }
}

impl<T> Drop for SPSCSender<T> {
    fn drop(&mut self) {
        let inner = unsafe { &mut *self.inner.get() };
        inner.closed = true;
        inner.sender_waker = None;
        if let Some(waker) = inner.receiver_waker.take() {
            waker.wake();
        }
        if let Some(waker) = inner.close_waker.take() {
            waker.wake();
        }
    }
}

pub struct Send<'a, T> {
    sender: &'a mut SPSCSender<T>,
    item: Option<T>,
}

impl<T: Unpin> Future for Send<'_, T> {
    type Output = Result<(), T>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let send = Pin::into_inner(self);
        match send.sender.poll_ready(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(None) => {
                let item = send.item.take().expect("poll ready future multiple times");
                Poll::Ready(Err(item))
            }
            Poll::Ready(Some(_)) => {
                let item = send.item.take().expect("poll ready future multiple times");
                send.sender.do_send(item);
                Poll::Ready(Ok(()))
            }
        }
    }
}

pub struct Closed<'a, T> {
    sender: &'a mut SPSCSender<T>,
}

impl<T> Future for Closed<'_, T> {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let closed = Pin::into_inner(self);
        closed.sender.poll_closed(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[monoio::test]
    async fn send_recv() {
        let (mut tx, mut rx) = spsc_pair::<u8>();
        tx.send(24).await.expect("receiver should not be closed");
        assert_eq!(rx.recv().await, Some(24));
        tx.send(42).await.expect("receiver should not be closed");
        assert_eq!(rx.recv().await, Some(42));
    }
}
