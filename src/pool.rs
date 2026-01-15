use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

/// Statistics about the thread pool's current state.
#[derive(Debug, Clone, Copy)]
#[allow(dead_code)]
pub struct PoolStats {
    /// Total number of worker slots.
    pub capacity: usize,
    /// Number of tasks currently executing.
    pub active: usize,
    /// Number of tasks waiting for a slot.
    pub queued: usize,
    /// Number of available slots.
    pub available: usize,
}

/// A fixed-size thread pool backed by a tokio semaphore.
///
/// All jobs (checks, solvers) share this pool. When all slots are
/// occupied, new jobs wait until a slot becomes available.
#[derive(Clone)]
pub struct Pool {
    semaphore: Arc<Semaphore>,
    capacity: usize,
    active: Arc<AtomicUsize>,
    queued: Arc<AtomicUsize>,
    cancel: CancellationToken,
}

impl Pool {
    /// Create a new pool with the given number of worker slots.
    /// If `workers` is 0, defaults to the number of CPU cores.
    pub fn new(workers: usize) -> Self {
        let capacity = if workers == 0 {
            num_cpus::get().max(1)
        } else {
            workers
        };
        Self {
            semaphore: Arc::new(Semaphore::new(capacity)),
            capacity,
            active: Arc::new(AtomicUsize::new(0)),
            queued: Arc::new(AtomicUsize::new(0)),
            cancel: CancellationToken::new(),
        }
    }

    /// Spawn a task that will run when a slot is available.
    ///
    /// The task waits in the queue until a permit is acquired, then executes.
    /// Returns a JoinHandle that can be awaited for the result.
    pub fn spawn<F, T>(&self, task: F) -> tokio::task::JoinHandle<T>
    where
        F: Future<Output = T> + Send + 'static,
        T: Send + 'static,
    {
        let semaphore = self.semaphore.clone();
        let active = self.active.clone();
        let queued = self.queued.clone();

        tokio::spawn(async move {
            queued.fetch_add(1, Ordering::SeqCst);
            let permit = semaphore.acquire_owned().await.unwrap();
            queued.fetch_sub(1, Ordering::SeqCst);
            active.fetch_add(1, Ordering::SeqCst);

            let result = task.await;

            drop(permit);
            active.fetch_sub(1, Ordering::SeqCst);
            result
        })
    }

    /// Get current pool statistics.
    pub fn stats(&self) -> PoolStats {
        PoolStats {
            capacity: self.capacity,
            active: self.active.load(Ordering::SeqCst),
            queued: self.queued.load(Ordering::SeqCst),
            available: self.semaphore.available_permits(),
        }
    }

    /// Get the cancellation token for this pool.
    #[allow(dead_code)]
    pub fn cancel_token(&self) -> CancellationToken {
        self.cancel.clone()
    }

    /// Signal cancellation to all tasks that check the token.
    #[allow(dead_code)]
    pub fn cancel(&self) {
        self.cancel.cancel();
    }

    /// Check if cancellation has been requested.
    #[allow(dead_code)]
    pub fn is_cancelled(&self) -> bool {
        self.cancel.is_cancelled()
    }

    /// Get the pool capacity.
    #[allow(dead_code)]
    pub fn capacity(&self) -> usize {
        self.capacity
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use std::time::Duration;

    #[tokio::test]
    async fn pool_respects_capacity() {
        let pool = Pool::new(2);
        assert_eq!(pool.capacity(), 2);

        let stats = pool.stats();
        assert_eq!(stats.capacity, 2);
        assert_eq!(stats.active, 0);
        assert_eq!(stats.queued, 0);
    }

    #[tokio::test]
    async fn pool_tracks_active_tasks() {
        let pool = Pool::new(4);
        let counter = Arc::new(AtomicUsize::new(0));

        let mut handles = Vec::new();
        for _ in 0..3 {
            let c = counter.clone();
            handles.push(pool.spawn(async move {
                c.fetch_add(1, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(10)).await;
            }));
        }

        // Give tasks a moment to start
        tokio::time::sleep(Duration::from_millis(5)).await;

        for h in handles {
            let _ = h.await;
        }

        assert_eq!(counter.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn pool_returns_task_result() {
        let pool = Pool::new(2);

        let handle = pool.spawn(async { 42 });
        let result = handle.await.unwrap();

        assert_eq!(result, 42);
    }

    #[tokio::test]
    async fn pool_default_uses_cpu_count() {
        let pool = Pool::new(0);
        assert!(pool.capacity() >= 1);
    }

    #[tokio::test]
    async fn pool_cancellation_token_works() {
        let pool = Pool::new(2);

        assert!(!pool.is_cancelled());
        pool.cancel();
        assert!(pool.is_cancelled());
    }
}
