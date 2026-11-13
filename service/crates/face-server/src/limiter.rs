//! Admission control: at most `max_concurrency` requests hold a processing slot; up to
//! `queue_limit` more wait for one; the rest are turned away (`503 overloaded`).
//!
//! A request takes its slot before reading the upload, so memory is bounded by
//! `max_concurrency` request bodies. The slot moves into the blocking task and is released only
//! when the CPU work ends, even if the request timed out in the meantime.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use prometheus::IntGauge;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

/// The queue is full.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Overloaded;

pub struct Limiter {
    slots: Arc<Semaphore>,
    queue_limit: usize,
    waiting: AtomicUsize,
    inflight: IntGauge,
    queue_depth: IntGauge,
}

/// A processing slot. Released on drop.
pub struct Permit {
    _permit: OwnedSemaphorePermit,
    inflight: IntGauge,
}

impl Drop for Permit {
    fn drop(&mut self) {
        self.inflight.dec();
    }
}

/// A reserved place in the queue. Released on drop, including when the waiting request is
/// cancelled (timeout, client gone).
struct QueueSlot<'a> {
    waiting: &'a AtomicUsize,
    queue_depth: &'a IntGauge,
}

impl Drop for QueueSlot<'_> {
    fn drop(&mut self) {
        self.waiting.fetch_sub(1, Ordering::AcqRel);
        self.queue_depth.dec();
    }
}

impl Limiter {
    pub fn new(
        max_concurrency: usize,
        queue_limit: usize,
        inflight: IntGauge,
        queue_depth: IntGauge,
    ) -> Self {
        Limiter {
            slots: Arc::new(Semaphore::new(max_concurrency.max(1))),
            queue_limit,
            waiting: AtomicUsize::new(0),
            inflight,
            queue_depth,
        }
    }

    /// Take a slot now, or wait in the queue when there is room in it.
    pub async fn acquire(&self) -> Result<Permit, Overloaded> {
        if let Ok(permit) = self.slots.clone().try_acquire_owned() {
            return Ok(self.permit(permit));
        }
        self.waiting
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |w| {
                (w < self.queue_limit).then_some(w + 1)
            })
            .map_err(|_| Overloaded)?;
        self.queue_depth.inc();
        let slot = QueueSlot {
            waiting: &self.waiting,
            queue_depth: &self.queue_depth,
        };
        // The semaphore is never closed.
        let permit = self
            .slots
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| Overloaded)?;
        drop(slot);
        Ok(self.permit(permit))
    }

    fn permit(&self, permit: OwnedSemaphorePermit) -> Permit {
        self.inflight.inc();
        Permit {
            _permit: permit,
            inflight: self.inflight.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limiter(slots: usize, queue: usize) -> Limiter {
        Limiter::new(
            slots,
            queue,
            IntGauge::new("i", "i").unwrap(),
            IntGauge::new("q", "q").unwrap(),
        )
    }

    #[tokio::test]
    async fn queues_up_to_the_limit_then_refuses() {
        let l = Arc::new(limiter(1, 1));
        let first = l.acquire().await.unwrap();
        assert_eq!(l.inflight.get(), 1);

        let waiter = {
            let l = l.clone();
            tokio::spawn(async move { l.acquire().await.map(|_| ()) })
        };
        while l.queue_depth.get() == 0 {
            tokio::task::yield_now().await;
        }
        assert_eq!(l.acquire().await.err(), Some(Overloaded));

        drop(first);
        assert_eq!(waiter.await.unwrap(), Ok(()));
        assert_eq!((l.inflight.get(), l.queue_depth.get()), (0, 0));
    }

    #[tokio::test]
    async fn zero_queue_refuses_immediately_and_cancelled_waiters_free_their_place() {
        let l = limiter(1, 0);
        let held = l.acquire().await.unwrap();
        assert_eq!(l.acquire().await.err(), Some(Overloaded));
        drop(held);

        let l = limiter(1, 1);
        let held = l.acquire().await.unwrap();
        let cancelled =
            tokio::time::timeout(std::time::Duration::from_millis(10), l.acquire()).await;
        assert!(cancelled.is_err());
        assert_eq!(
            l.queue_depth.get(),
            0,
            "the timed-out waiter left the queue"
        );
        drop(held);
        assert!(l.acquire().await.is_ok());
    }
}
