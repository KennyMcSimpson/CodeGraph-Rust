//! Cooperative deadlines shared by the query engine and its transports. Cancellation is
//! scoped to a worker, so concurrent requests never cancel one another.

use std::cell::RefCell;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::{Duration, Instant};

#[derive(Clone, Debug)]
pub struct Cancellation {
    cancelled: Arc<AtomicBool>,
    deadline: Instant,
}

#[derive(Debug)]
pub struct Cancelled;

thread_local! {
    static CURRENT: RefCell<Option<(Cancellation, u16)>> = const { RefCell::new(None) };
}

impl Cancellation {
    pub fn new(timeout: Duration) -> Self {
        Self { cancelled: Arc::new(AtomicBool::new(false)), deadline: Instant::now() + timeout }
    }
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed) || Instant::now() >= self.deadline
    }
    pub fn run<T>(&self, f: impl FnOnce() -> T) -> T {
        struct Restore(Option<(Cancellation, u16)>);
        impl Drop for Restore {
            fn drop(&mut self) {
                CURRENT.with(|c| *c.borrow_mut() = self.0.take());
            }
        }
        let _restore = Restore(CURRENT.with(|c| c.replace(Some((self.clone(), 0)))));
        check_now();
        f()
    }
}

/// Cheap checkpoints may be placed in hot graph accessors. Read the clock every 128
/// accesses, rather than adding a clock read to every edge.
#[inline]
pub fn check() {
    let cancelled = CURRENT.with(|c| {
        let mut c = c.borrow_mut();
        match c.as_mut() {
            Some((token, remaining)) if *remaining == 0 => {
                *remaining = 127;
                token.is_cancelled()
            }
            Some((_, remaining)) => {
                *remaining -= 1;
                false
            }
            None => false,
        }
    });
    if cancelled {
        std::panic::resume_unwind(Box::new(Cancelled));
    }
}

pub fn check_now() {
    let cancelled = CURRENT.with(|c| c.borrow().as_ref().is_some_and(|(t, _)| t.is_cancelled()));
    if cancelled {
        std::panic::resume_unwind(Box::new(Cancelled));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cancelled_work_never_starts_and_scope_is_restored() {
        let token = Cancellation::new(Duration::from_secs(5));
        token.cancel();
        let mut ran = false;
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| token.run(|| ran = true)));
        assert!(r.unwrap_err().is::<Cancelled>());
        assert!(!ran);
        check_now();
        Cancellation::new(Duration::from_secs(5)).run(check_now);
    }
    #[test]
    fn deadline_is_checked_without_a_watchdog() {
        let token = Cancellation::new(Duration::ZERO);
        assert!(std::panic::catch_unwind(|| token.run(|| 1)).unwrap_err().is::<Cancelled>());
    }
    #[test]
    fn running_worker_observes_external_cancellation() {
        let token = Cancellation::new(Duration::from_secs(5));
        let worker = token.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        let thread = std::thread::spawn(move || {
            std::panic::catch_unwind(|| {
                worker.run(|| {
                    tx.send(()).unwrap();
                    loop {
                        check();
                    }
                })
            })
        });
        rx.recv_timeout(Duration::from_secs(2)).unwrap();
        token.cancel();
        assert!(thread.join().unwrap().unwrap_err().is::<Cancelled>());
    }
}
