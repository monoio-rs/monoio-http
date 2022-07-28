use std::{
    cell::UnsafeCell,
    collections::{HashMap, VecDeque},
    future::Future,
    hash::Hash,
    ops::{Deref, DerefMut},
    rc::{Rc, Weak},
    task::ready,
    time::{Duration, Instant},
};

const DEFAULT_IDLE_INTERVAL: Duration = Duration::from_secs(10);

use monoio_http::h1::codec::ClientCodec;

type Conns<K, IO> = Rc<UnsafeCell<SharedInner<K, IO>>>;
type WeakConns<K, IO> = Weak<UnsafeCell<SharedInner<K, IO>>>;

struct IdleCodec<IO> {
    codec: ClientCodec<IO>,
    idle_at: Instant,
}

struct SharedInner<K, IO> {
    mapping: HashMap<K, VecDeque<IdleCodec<IO>>>,
    _drop: local_sync::oneshot::Receiver<()>,
}

impl<K, IO> SharedInner<K, IO> {
    fn new() -> (local_sync::oneshot::Sender<()>, Self) {
        let mapping = HashMap::new();
        let (tx, _drop) = local_sync::oneshot::channel();
        (tx, Self { mapping, _drop })
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
pub struct ConnectionPool<K: Hash + Eq, IO> {
    conns: Conns<K, IO>,
}

impl<K: Hash + Eq, IO> Clone for ConnectionPool<K, IO> {
    fn clone(&self) -> Self {
        Self {
            conns: self.conns.clone(),
        }
    }
}

pub struct PooledConnection<K, IO>
where
    K: Hash + Eq,
{
    // option is for take when drop
    key: Option<K>,
    // option is for take when drop
    codec: Option<ClientCodec<IO>>,

    pool: WeakConns<K, IO>,
    reuseable: bool,
}

impl<K: Hash + Eq + 'static, IO: 'static> ConnectionPool<K, IO> {
    fn new(idle_interval: Option<Duration>) -> Self {
        let (tx, inner) = SharedInner::new();
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
}

impl<K: Hash + Eq + 'static, IO: 'static> Default for ConnectionPool<K, IO> {
    fn default() -> Self {
        Self::new(None)
    }
}

impl<K, IO> PooledConnection<K, IO>
where
    K: Hash + Eq,
{
    pub fn set_reuseable(&mut self, reuseable: bool) {
        self.reuseable = reuseable;
    }
}

impl<K, IO> Deref for PooledConnection<K, IO>
where
    K: Hash + Eq,
{
    type Target = ClientCodec<IO>;

    fn deref(&self) -> &Self::Target {
        self.codec.as_ref().expect("connection should be present")
    }
}

impl<K, IO> DerefMut for PooledConnection<K, IO>
where
    K: Hash + Eq,
{
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.codec.as_mut().expect("connection should be present")
    }
}

impl<K, IO> Drop for PooledConnection<K, IO>
where
    K: Hash + Eq,
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
            unsafe { &mut *pool.get() }
                .mapping
                .entry(key)
                .or_default()
                .push_back(idle);
            #[cfg(feature = "logging")]
            tracing::debug!("connection recycled");
        }
    }
}

impl<K, IO> ConnectionPool<K, IO>
where
    K: Hash + Eq + ToOwned<Owned = K>,
{
    pub fn get(&self, key: &K) -> Option<PooledConnection<K, IO>> {
        if let Some(v) = unsafe { &mut *self.conns.get() }.mapping.get_mut(key) {
            #[cfg(feature = "logging")]
            tracing::debug!("connection got from pool");
            return v.pop_front().map(|idle| PooledConnection {
                key: Some(key.to_owned()),
                codec: Some(idle.codec),
                pool: Rc::downgrade(&self.conns),
                reuseable: true,
            });
        }
        None
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
struct IdleTask<K, IO> {
    tx: local_sync::oneshot::Sender<()>,
    conns: WeakConns<K, IO>,
    interval: monoio::time::Interval,
    idle_dur: Duration,
}

impl<K, IO> Future for IdleTask<K, IO> {
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
