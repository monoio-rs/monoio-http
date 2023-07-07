use std::{
    cell::UnsafeCell,
    collections::{HashMap, VecDeque},
    fmt::{Debug, Display},
    future::Future,
    hash::Hash,
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

use local_sync::oneshot;
use monoio_http::common::{body::HttpBody, request::Request};

use super::connection::{MultiSender, SingleSender, Transaction};

type Conns<K, B> = Rc<UnsafeCell<SharedInner<K, B>>>;
type WeakConns<K, B> = Weak<UnsafeCell<SharedInner<K, B>>>;

struct IdleConnection<K: Hash + Eq + Display, B> {
    pipe: PooledConnectionPipe<K, B>,
    idle_at: Instant,
}

struct SharedInner<K: Hash + Eq + Display, B> {
    mapping: HashMap<K, VecDeque<IdleConnection<K, B>>>,
    max_idle: usize,
    #[cfg(feature = "time")]
    _drop: local_sync::oneshot::Receiver<()>,
}

impl<K: Hash + Eq + Display, B> SharedInner<K, B> {
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
        Self { mapping, max_idle }
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
pub struct ConnectionPool<K: Hash + Eq + Display, B> {
    conns: Conns<K, B>,
}

impl<K: Hash + Eq + Display, B> Clone for ConnectionPool<K, B> {
    fn clone(&self) -> Self {
        Self {
            conns: self.conns.clone(),
        }
    }
}

impl<K: Hash + Eq + Display + 'static, B: 'static> ConnectionPool<K, B> {
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

impl<K: Hash + Eq + Display + 'static, B: 'static> Default for ConnectionPool<K, B> {
    #[cfg(feature = "time")]
    fn default() -> Self {
        Self::new(None, None)
    }

    #[cfg(not(feature = "time"))]
    fn default() -> Self {
        Self::new(None)
    }
}

pub struct PooledConnection<K, B>
where
    K: Hash + Eq + Display,
{
    // option is for take when drop
    key: Option<K>,
    pipe: Option<PooledConnectionPipe<K, B>>,
    pool: WeakConns<K, B>,
    reuseable: bool,
}

impl<K, B> PooledConnection<K, B>
where
    K: Hash + Eq + Display,
{
    pub async fn send_request(
        mut self,
        req: Request<B>,
    ) -> Result<http::Response<HttpBody>, crate::Error> {
        match self.pipe.take() {
            Some(mut pipe) => {
                self.pipe = Some(pipe.clone());
                match pipe.send_request(req, self).await {
                    Ok(resp) => Ok(resp),
                    Err(e) => {
                        #[cfg(feature = "logging")]
                        tracing::error!("Request failed: {:?}", e);
                        Err(e)
                    }
                }
            }
            None => {
                panic!()
            }
        }
    }

    pub fn set_reuseable(&mut self, set: bool) {
        self.reuseable = set;
    }
}

impl<K, IO> Drop for PooledConnection<K, IO>
where
    K: Hash + Eq + Display,
{
    fn drop(&mut self) {
        if !self.reuseable {
            #[cfg(feature = "logging")]
            tracing::debug!("connection dropped");
            return;
        }

        if let Some(pool) = self.pool.upgrade() {
            let key = self.key.take().expect("unable to take key");
            let pipe = self.pipe.take().expect("unable to take connection");
            let idle = IdleConnection {
                pipe,
                idle_at: Instant::now(),
            };

            let conns = unsafe { &mut *pool.get() };
            #[cfg(feature = "logging")]
            let key_str = key.to_string();
            let queue = conns
                .mapping
                .entry(key)
                .or_insert(VecDeque::with_capacity(conns.max_idle));

            #[cfg(feature = "logging")]
            tracing::debug!(
                "connection pool size: {:?} for key: {:?}",
                queue.len(),
                key_str
            );

            if queue.len() > conns.max_idle {
                #[cfg(feature = "logging")]
                tracing::info!("connection pool is full for key: {:?}", key_str);
                let _ = queue.pop_front();
            }

            queue.push_back(idle);

            #[cfg(feature = "logging")]
            tracing::debug!("connection recycled");
        }
    }
}

impl<K, B> ConnectionPool<K, B>
where
    K: Hash + Eq + ToOwned<Owned = K> + Display,
{
    pub fn get(&self, key: &K) -> Option<PooledConnection<K, B>> {
        let conns = unsafe { &mut *self.conns.get() };

        match conns.mapping.get_mut(key) {
            Some(v) => {
                #[cfg(feature = "logging")]
                tracing::debug!("connection got from pool for key: {:?} ", key.to_string());
                let mut pooled_conn = PooledConnection {
                    key: Some(key.to_owned()),
                    pipe: None,
                    pool: Rc::downgrade(&self.conns),
                    reuseable: true,
                };
                let idle_conn = v.pop_front();
                match idle_conn {
                    Some(mut idle) => {
                        let is_h2 = idle.pipe.is_http2();
                        if is_h2 {
                            pooled_conn.pipe = Some(idle.pipe.clone());
                            pooled_conn.reuseable = false;
                            idle.idle_at = Instant::now();
                            v.push_back(idle);
                        } else {
                            pooled_conn.pipe = Some(idle.pipe);
                        }
                        Some(pooled_conn)
                    }
                    None => None,
                }
            }
            None => {
                #[cfg(feature = "logging")]
                tracing::debug!("no connection in pool for key: {:?} ", key.to_string());
                None
            }
        }
    }

    pub fn link(&self, key: K, pipe: PooledConnectionPipe<K, B>) -> PooledConnection<K, B> {
        #[cfg(feature = "logging")]
        tracing::debug!("linked new connection to the pool");
        PooledConnection {
            key: Some(key),
            pipe: Some(pipe),
            pool: Rc::downgrade(&self.conns),
            reuseable: true,
        }
    }
}

// TODO: make interval not eq to idle_dur
struct IdleTask<K: Hash + Eq + Display, B> {
    tx: local_sync::oneshot::Sender<()>,
    conns: WeakConns<K, B>,
    interval: monoio::time::Interval,
    idle_dur: Duration,
}

impl<K: Hash + Eq + Display, B> Future for IdleTask<K, B> {
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
pub enum PooledConnectionPipe<K, B>
where
    K: Hash + Eq + Display,
{
    Http1(SingleSender<K, B>),
    Http2(MultiSender<K, B>),
}

impl<K: Hash + Eq + Display, B> PooledConnectionPipe<K, B> {
    pub async fn send_request(
        &mut self,
        req: Request<B>,
        conn: PooledConnection<K, B>,
    ) -> Result<http::Response<HttpBody>, crate::Error> {
        let (resp_tx, resp_rx) = oneshot::channel();

        let res = match self {
            Self::Http1(ref s) => s.send(Transaction { req, resp_tx, conn }),
            Self::Http2(ref s) => s.send(Transaction { req, resp_tx, conn }),
        };

        match res {
            Ok(_) => {}
            Err(_e) => {
                #[cfg(feature = "logging")]
                tracing::error!("Request send to conn manager failed {_e:?}");
                return Err(crate::error::Error::ConnManagerReqSendError);
            }
        }

        resp_rx.await?
    }

    pub fn is_http2(&self) -> bool {
        match self {
            Self::Http1(_) => false,
            Self::Http2(_) => true,
        }
    }
}

impl<K: Hash + Eq + Display, B> Clone for PooledConnectionPipe<K, B> {
    fn clone(&self) -> Self {
        match self {
            Self::Http1(tx) => Self::Http1(tx.temp_sender_clone()),
            Self::Http2(tx) => Self::Http2(tx.clone()),
        }
    }
}
