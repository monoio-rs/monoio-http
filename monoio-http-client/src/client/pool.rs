use std::{
    cell::UnsafeCell,
    collections::{HashMap, VecDeque},
    hash::Hash,
    ops::{Deref, DerefMut},
    rc::{Rc, Weak},
};

use monoio_http::h1::codec::ClientCodec;

type Conns<K, IO> = Rc<UnsafeCell<HashMap<K, VecDeque<ClientCodec<IO>>>>>;
type WeakConns<K, IO> = Weak<UnsafeCell<HashMap<K, VecDeque<ClientCodec<IO>>>>>;

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
    io: Option<ClientCodec<IO>>,

    pool: WeakConns<K, IO>,
    reuseable: bool,
}

impl<K: Hash + Eq, IO> Default for ConnectionPool<K, IO> {
    fn default() -> Self {
        let conns = Rc::new(UnsafeCell::new(HashMap::new()));
        Self { conns }
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
        self.io.as_ref().expect("connection should be present")
    }
}

impl<K, IO> DerefMut for PooledConnection<K, IO>
where
    K: Hash + Eq,
{
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.io.as_mut().expect("connection should be present")
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
            let io = self.io.take().expect("unable to take connection");
            unsafe { &mut *pool.get() }
                .entry(key)
                .or_default()
                .push_back(io);
            #[cfg(feature = "logging")]
            tracing::debug!("connection reused");
        }
    }
}

impl<K, IO> ConnectionPool<K, IO>
where
    K: Hash + Eq + ToOwned<Owned = K>,
{
    pub fn get(&self, key: &K) -> Option<PooledConnection<K, IO>> {
        if let Some(v) = unsafe { &mut *self.conns.get() }.get_mut(key) {
            return v.pop_front().map(|io| PooledConnection {
                key: Some(key.to_owned()),
                io: Some(io),
                pool: Rc::downgrade(&self.conns),
                reuseable: true,
            });
        }
        None
    }

    pub fn link(&self, key: K, io: ClientCodec<IO>) -> PooledConnection<K, IO> {
        PooledConnection {
            key: Some(key),
            io: Some(io),
            pool: Rc::downgrade(&self.conns),
            reuseable: true,
        }
    }
}
