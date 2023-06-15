use std::{
    cell::UnsafeCell,
    collections::{HashMap, VecDeque},
    fmt::Debug,
    future::Future,
    hash::Hash,
    ops::{Deref, DerefMut},
    rc::{Rc, Weak},
    task::ready,
    time::{Duration, Instant},
};

#[cfg(feature = "time")]
const DEFAULT_IDLE_INTERVAL: Duration = Duration::from_secs(60);
const DEFAULT_KEEPALIVE_CONNS: usize = 256;
const DEFAULT_POOL_SIZE: usize = 32;
// https://datatracker.ietf.org/doc/html/rfc6335
const MAX_KEEPALIVE_CONNS: usize = 16384;

use monoio::io::{sink::Sink, AsyncWriteRent};
use monoio_http::h1::{codec::ClientCodec, BorrowFramedRead, FramedRead};

type Conns<K, IO> = Rc<UnsafeCell<SharedInner<K, IO>>>;
type WeakConns<K, IO> = Weak<UnsafeCell<SharedInner<K, IO>>>;

struct IdleCodec<IO: AsyncWriteRent> {
    codec: ClientCodec<IO>,
    idle_at: Instant,
}

struct SharedInner<K, IO: AsyncWriteRent> {
    mapping: HashMap<K, VecDeque<IdleCodec<IO>>>,
    max_idle: usize,
    #[cfg(feature = "time")]
    _drop: local_sync::oneshot::Receiver<()>,
}

impl<K, IO: AsyncWriteRent> SharedInner<K, IO> {
    #[cfg(feature = "time")]
    fn new(max_idle: Option<usize>) -> (local_sync::oneshot::Sender<()>, Self) {
        let mapping = HashMap::with_capacity(DEFAULT_POOL_SIZE);
        let max_idle = max_idle
            .map(|n| n.min(MAX_KEEPALIVE_CONNS))
            .unwrap_or(DEFAULT_KEEPALIVE_CONNS);

        let (tx, _drop) = local_sync::oneshot::channel();
        (
            tx,
            Self {
                mapping,
                _drop,
                max_idle,
            },
        )
    }

    #[cfg(not(feature = "time"))]
    fn new(max_idle: Option<usize>) -> Self {
        let mapping = HashMap::with_capacity(DEFAULT_POOL_SIZE);
        let max_idle = max_idle
            .map(|n| n.min(MAX_KEEPALIVE_CONNS))
            .unwrap_or(DEFAULT_KEEPALIVE_CONNS);
        Self {
            mapping,
            keepalive_conns,
        }
    }

    fn clear_expired(&mut self, dur: Duration) {
        self.mapping.retain(|_, values| {
            values.retain(|entry| entry.idle_at.elapsed() <= dur);
            !values.is_empty()
        });
    }
}

// TODO: Connection leak? Maybe remove expired connection periodically.
#[derive(Debug)]
pub struct ConnectionPool<K: Hash + Eq, IO: AsyncWriteRent> {
    conns: Conns<K, IO>,
}

impl<K: Hash + Eq, IO: AsyncWriteRent> Clone for ConnectionPool<K, IO> {
    fn clone(&self) -> Self {
        Self {
            conns: self.conns.clone(),
        }
    }
}

pub struct PooledConnection<K, IO: AsyncWriteRent>
where
    K: Hash + Eq + Debug,
{
    // option is for take when drop
    key: Option<K>,
    // option is for take when drop
    codec: Option<ClientCodec<IO>>,

    pool: WeakConns<K, IO>,
    reuseable: bool,
}

impl<K: Hash + Eq + 'static, IO: AsyncWriteRent + 'static> ConnectionPool<K, IO> {
    #[cfg(feature = "time")]
    fn new(idle_interval: Option<Duration>, max_idle: Option<usize>) -> Self {
        let (tx, inner) = SharedInner::new(max_idle);
        let conns = Rc::new(UnsafeCell::new(inner));
        let idle_interval = idle_interval.unwrap_or(DEFAULT_IDLE_INTERVAL);
        monoio::spawn(IdleTask {
            tx,
            conns: Rc::downgrade(&conns),
            interval: monoio::time::interval(idle_interval),
            idle_dur: idle_interval,
        });

        Self { conns }
    }

    #[cfg(not(feature = "time"))]
    fn new(max_idle: Option<usize>) -> Self {
        let conns = Rc::new(UnsafeCell::new(SharedInner::new(max_idle)));
        Self { conns }
    }
}

impl<K: Hash + Eq + 'static, IO: AsyncWriteRent + 'static> Default for ConnectionPool<K, IO> {
    #[cfg(feature = "time")]
    fn default() -> Self {
        Self::new(None, None)
    }

    #[cfg(not(feature = "time"))]
    fn default() -> Self {
        Self::new(None)
    }
}

impl<K, IO: AsyncWriteRent> PooledConnection<K, IO>
where
    K: Hash + Eq + Debug,
{
    pub fn set_reuseable(&mut self, reuseable: bool) {
        self.reuseable = reuseable;
    }
}

impl<K: Hash + Eq + Debug, IO: AsyncWriteRent> BorrowFramedRead for PooledConnection<K, IO>
where
    ClientCodec<IO>: BorrowFramedRead,
{
    type IO = <ClientCodec<IO> as BorrowFramedRead>::IO;
    type Codec = <ClientCodec<IO> as BorrowFramedRead>::Codec;

    fn framed_mut(&mut self) -> &mut FramedRead<Self::IO, Self::Codec> {
        self.codec
            .as_mut()
            .expect("connection should be present")
            .framed_mut()
    }
}

impl<K, IO: AsyncWriteRent> Deref for PooledConnection<K, IO>
where
    K: Hash + Eq + Debug,
{
    type Target = ClientCodec<IO>;

    fn deref(&self) -> &Self::Target {
        self.codec.as_ref().expect("connection should be present")
    }
}

impl<K, IO: AsyncWriteRent> DerefMut for PooledConnection<K, IO>
where
    K: Hash + Eq + Debug,
{
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.codec.as_mut().expect("connection should be present")
    }
}

impl<K: Hash + Eq + Debug, IO: AsyncWriteRent, R> Sink<R> for PooledConnection<K, IO>
where
    ClientCodec<IO>: Sink<R>,
{
    type Error = <ClientCodec<IO> as Sink<R>>::Error;

    type SendFuture<'a> = <ClientCodec<IO> as Sink<R>>::SendFuture<'a>
    where
        Self: 'a, R: 'a;

    type FlushFuture<'a> = <ClientCodec<IO> as Sink<R>>::FlushFuture<'a>
    where
        Self: 'a;

    type CloseFuture<'a> = <ClientCodec<IO> as Sink<R>>::CloseFuture<'a>
    where
        Self: 'a;

    fn send<'a>(&'a mut self, item: R) -> Self::SendFuture<'a>
    where
        R: 'a,
    {
        self.codec
            .as_mut()
            .expect("connection should be present")
            .send(item)
    }

    fn flush(&mut self) -> Self::FlushFuture<'_> {
        self.codec
            .as_mut()
            .expect("connection should be present")
            .flush()
    }

    fn close(&mut self) -> Self::CloseFuture<'_> {
        self.codec
            .as_mut()
            .expect("connection should be present")
            .close()
    }
}

impl<K, IO: AsyncWriteRent> Drop for PooledConnection<K, IO>
where
    K: Hash + Eq + Debug,
{
    fn drop(&mut self) {
        if !self.reuseable {
            #[cfg(feature = "logging")]
            tracing::debug!("connection dropped");
            return;
        }

        if let Some(pool) = self.pool.upgrade() {
            let key = self.key.take().expect("unable to take key");
            let codec = self.codec.take().expect("unable to take connection");
            let idle = IdleCodec {
                codec,
                idle_at: Instant::now(),
            };

            let conns = unsafe { &mut *pool.get() };
            #[cfg(feature = "logging")]
            let key_debug = format!("{key:?}");

            let queue = conns
                .mapping
                .entry(key)
                .or_insert(VecDeque::with_capacity(conns.max_idle));

            #[cfg(feature = "logging")]
            tracing::debug!(
                "connection pool size: {:?} for key: {key_debug}",
                queue.len(),
            );

            if queue.len() > conns.max_idle {
                #[cfg(feature = "logging")]
                tracing::info!("connection pool is full for key: {key_debug}");
                let _ = queue.pop_front();
            }

            queue.push_back(idle);

            #[cfg(feature = "logging")]
            tracing::debug!("connection recycled");
        }
    }
}

impl<K, IO: AsyncWriteRent> ConnectionPool<K, IO>
where
    K: Hash + Eq + ToOwned<Owned = K> + Debug,
{
    pub fn get(&self, key: &K) -> Option<PooledConnection<K, IO>> {
        let conns = unsafe { &mut *self.conns.get() };

        match conns.mapping.get_mut(key) {
            Some(v) => {
                #[cfg(feature = "logging")]
                tracing::debug!("connection got from pool for key: {key:?}");
                v.pop_front().map(|idle| PooledConnection {
                    key: Some(key.to_owned()),
                    codec: Some(idle.codec),
                    pool: Rc::downgrade(&self.conns),
                    reuseable: true,
                })
            }
            None => {
                #[cfg(feature = "logging")]
                tracing::debug!("no connection in pool for key: {key:?}");
                None
            }
        }
    }

    pub fn link(&self, key: K, io: ClientCodec<IO>) -> PooledConnection<K, IO> {
        #[cfg(feature = "logging")]
        tracing::debug!("linked new connection to the pool");
        PooledConnection {
            key: Some(key),
            codec: Some(io),
            pool: Rc::downgrade(&self.conns),
            reuseable: true,
        }
    }
}

// TODO: make interval not eq to idle_dur
struct IdleTask<K, IO: AsyncWriteRent> {
    tx: local_sync::oneshot::Sender<()>,
    conns: WeakConns<K, IO>,
    interval: monoio::time::Interval,
    idle_dur: Duration,
}

impl<K, IO: AsyncWriteRent> Future for IdleTask<K, IO> {
    type Output = ();

    fn poll(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        let this = self.get_mut();
        loop {
            match this.tx.poll_closed(cx) {
                std::task::Poll::Ready(_) => {
                    #[cfg(feature = "logging")]
                    tracing::debug!("pool rx dropped, idle task exit");
                    return std::task::Poll::Ready(());
                }
                std::task::Poll::Pending => (),
            }

            ready!(this.interval.poll_tick(cx));
            if let Some(inner) = this.conns.upgrade() {
                let inner_mut = unsafe { &mut *inner.get() };
                inner_mut.clear_expired(this.idle_dur);
                #[cfg(feature = "logging")]
                tracing::debug!("pool clear expired");
                continue;
            }
            #[cfg(feature = "logging")]
            tracing::debug!("pool upgrade failed, idle task exit");
            return std::task::Poll::Ready(());
        }
    }
}
