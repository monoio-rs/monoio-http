use std::{
    pin::Pin,
    task::{Context, Poll},
};

use futures_core::Future;
use monoio::{buf::IoBuf, macros::support::poll_fn};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamHint {
    None,
    Fixed,
    Stream,
}

pub trait Body {
    type Data: IoBuf;
    type Error;
    type DataFuture<'a>: Future<Output = Option<Result<Self::Data, Self::Error>>>
    where
        Self: 'a;

    fn next_data(&mut self) -> Self::DataFuture<'_>;
    fn stream_hint(&self) -> StreamHint;
}

pub trait OwnedBody {
    type Data: IoBuf;
    type Error;

    fn poll_next_data(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Self::Data, Self::Error>>>;
    fn stream_hint(&self) -> StreamHint;
}

impl<T: OwnedBody + Unpin> Body for T {
    type Data = <T as OwnedBody>::Data;
    type Error = <T as OwnedBody>::Error;
    type DataFuture<'a> = impl Future<Output = Option<Result<Self::Data, Self::Error>>> + 'a
    where
        Self: 'a;

    fn next_data(&mut self) -> Self::DataFuture<'_> {
        let mut pinned = Pin::new(self);
        poll_fn(move |cx| pinned.as_mut().poll_next_data(cx))
    }

    fn stream_hint(&self) -> StreamHint {
        <T as OwnedBody>::stream_hint(self)
    }
}
