use std::{cell::UnsafeCell, fmt, task::Waker};

/// LocalWaker is a thread-local waker that duplicates futures_util::task::AtomicWaker.
pub struct LocalWaker {
    waker: UnsafeCell<Option<Waker>>,
}

impl LocalWaker {
    pub const fn new() -> Self {
        Self {
            waker: UnsafeCell::new(None),
        }
    }

    pub fn register(&self, waker: &Waker) {
        unsafe {
            match &*self.waker.get() {
                Some(old_waker) if old_waker.will_wake(waker) => (),
                _ => *self.waker.get() = Some(waker.clone()),
            }
        }
    }

    pub fn wake(&self) {
        let waker = unsafe { &mut *self.waker.get() };
        if let Some(waker) = waker.take() {
            waker.wake();
        }
    }
}

impl Default for LocalWaker {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for LocalWaker {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "LocalWaker")
    }
}
