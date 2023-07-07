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

use bytes::Bytes;
use monoio::io::{AsyncReadRent, AsyncWriteRent, Split};
use monoio_http::common::{
    body::{Body, HttpBody},
    error::HttpError,
    request::Request, response::Response,
};

use super::connection::HttpConnection;


const CONN_CLOSE: &[u8] = b"close";

type Conns<K, IO> = Rc<UnsafeCell<SharedInner<K, IO>>>;
type WeakConns<K, IO> = Weak<UnsafeCell<SharedInner<K, IO>>>;

struct IdleConnection<IO: AsyncWriteRent + AsyncReadRent + Split> {
    conn: HttpConnection<IO>,
    idle_at: Instant,
}

impl<IO: AsyncWriteRent + AsyncReadRent + Split> IdleConnection<IO> {
    fn reserve(mut self) -> (HttpConnection<IO>, Option<Self>) {
        let is_h2 = self.conn.is_http2();

        if is_h2 {
            let conn = self.conn.http2_conn_clone();
            self.idle_at = Instant::now();
            (conn, Some(self))
        } else {
            (self.conn, None)
        }
    }
}

struct SharedInner<K: Hash + Eq + Display, IO: AsyncWriteRent + AsyncReadRent + Split> {
    mapping: HashMap<K, VecDeque<IdleConnection<IO>>>,
    max_idle: usize,
    #[cfg(feature = "time")]
    _drop: local_sync::oneshot::Receiver<()>,
}

impl<K: Hash + Eq + Display, IO: AsyncWriteRent + AsyncReadRent + Split> SharedInner<K, IO> {
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
pub struct ConnectionPool<K: Hash + Eq + Display, IO: AsyncWriteRent + AsyncReadRent + Split> {
    conns: Conns<K, IO>,
}

impl<K: Hash + Eq + Display, IO: AsyncWriteRent + AsyncReadRent + Split> Clone
    for ConnectionPool<K, IO>
{
    fn clone(&self) -> Self {
        Self {
            conns: self.conns.clone(),
        }
    }
}

impl<K: Hash + Eq + Display + 'static, IO: AsyncWriteRent + AsyncReadRent + Split + 'static>
    ConnectionPool<K, IO>
{
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

impl<K: Hash + Eq + Display + 'static, IO: AsyncWriteRent + AsyncReadRent + Split + 'static> Default
    for ConnectionPool<K, IO>
{
    #[cfg(feature = "time")]
    fn default() -> Self {
        Self::new(None, None)
    }

    #[cfg(not(feature = "time"))]
    fn default() -> Self {
        Self::new(None)
    }
}

pub struct PooledConnection<K, IO>
where
    K: Hash + Eq + Display,
    IO: AsyncWriteRent + AsyncReadRent + Split,
{
    // option is for take when drop
    key: Option<K>,
    conn: Option<HttpConnection<IO>>,
    pool: WeakConns<K, IO>,
    reusable: bool,
    remove_h2: bool, // Remove H2 Connection
}


impl<K, IO> PooledConnection<K, IO>
where
    K: Hash + Eq + Display,
    IO: AsyncWriteRent + AsyncReadRent + Split,
{
    pub async fn send_request<B: Body<Data = Bytes, Error = HttpError> + 'static>(
        mut self,
        req: Request<B>,
    ) -> Result<Response<HttpBody>, crate::Error> {
        match self.conn.as_mut() {
            Some(conn) => {
                let (result, remove) = conn.send_request(req).await;
                self.remove_h2 = remove;

                match result {
                    Ok(resp) => {
                        let header_value = resp.headers().get(http::header::CONNECTION);
                        if let Some(v) = header_value {
                            self.set_reusable(!v.as_bytes().eq_ignore_ascii_case(CONN_CLOSE))
                        }
                        Ok(resp)
                    },
                    Err(e) => Err(e)
                }
            }
            None => Err(crate::Error::MissingCodec),
        }
    }

    pub fn set_reusable(&mut self, set: bool) {
        self.reusable = set;
    }
}

impl<K, IO> Drop for PooledConnection<K, IO>
where
    K: Hash + Eq + Display,
    IO: AsyncWriteRent + AsyncReadRent + Split,
{
    fn drop(&mut self) {
        if !self.reusable && !self.remove_h2 {
            #[cfg(feature = "logging")]
            tracing::debug!("connection dropped");
            return;
        }

        if let Some(pool) = self.pool.upgrade() {
            let key = self.key.take().expect("unable to take key");
            let conn = self.conn.take().expect("unable to take connection");
            let idle = IdleConnection {
                conn,
                idle_at: Instant::now(),
            };

            let conns = unsafe { &mut *pool.get() };
            #[cfg(feature = "logging")]
            let key_str = key.to_string();

            if self.remove_h2 {
                // There should be only one H2 connection per host
                let _queue = conns.mapping.remove(&key);

                #[cfg(feature = "logging")]
                tracing::debug!("Removed H2 connection for key: {:?}", key_str);
            }

            if self.reusable {
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
}

impl<K, IO> ConnectionPool<K, IO>
where
    K: Hash + Eq + ToOwned<Owned = K> + Display,
    IO: AsyncWriteRent + AsyncReadRent + Split,
{
    pub fn get(&self, key: &K) -> Option<PooledConnection<K, IO>> {
        let conns = unsafe { &mut *self.conns.get() };

        match conns.mapping.get_mut(key) {
            Some(v) => {
                #[cfg(feature = "logging")]
                tracing::debug!("connection got from pool for key: {:?} ", key.to_string());
                let mut pooled_conn = PooledConnection {
                    key: Some(key.to_owned()),
                    conn: None,
                    pool: Rc::downgrade(&self.conns),
                    reusable: true,
                    remove_h2: false,
                };
                let idle_conn = v.pop_front();

                match idle_conn {
                    Some(idle) => {
                        let (checkout_conn, readd_conn) = idle.reserve();

                        // Add back the H2Connection so other request's can clone it
                        if let Some(idle) = readd_conn {
                            v.push_back(idle);
                            // No need to add back the H2Connection to the Pool
                            pooled_conn.reusable = false;
                        }

                        pooled_conn.conn = Some(checkout_conn);
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

    pub fn link(&self, key: K, conn: HttpConnection<IO>) -> PooledConnection<K, IO> {
        #[cfg(feature = "logging")]
        tracing::debug!("linked new connection to the pool");

        PooledConnection {
            key: Some(key),
            conn: Some(conn),
            pool: Rc::downgrade(&self.conns),
            reusable: true,
            remove_h2: false,
        }
    }
}

// TODO: make interval not eq to idle_dur
struct IdleTask<K: Hash + Eq + Display, IO: AsyncWriteRent + AsyncReadRent + Split> {
    tx: local_sync::oneshot::Sender<()>,
    conns: WeakConns<K, IO>,
    interval: monoio::time::Interval,
    idle_dur: Duration,
}

impl<K: Hash + Eq + Display, IO: AsyncWriteRent + AsyncReadRent + Split> Future
    for IdleTask<K, IO>
{
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
