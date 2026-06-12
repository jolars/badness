//! A minimal fixed-size worker thread pool, modeled on rust-analyzer's
//! `TaskPool`.
//!
//! EXTRACTION CANDIDATE: copied ~wholesale from arity's `src/lsp/task_pool.rs`.
//! Keep close to arity's version so the eventual shared-crate extraction stays
//! mechanical.
//!
//! The LSP keeps latency-sensitive reads (formatting, the parse-diagnostics
//! read-phase) on a dedicated [`TaskPool`] sized to the machine's parallelism,
//! off the single-writer worker thread.
//!
//! Jobs are fire-and-forget closures that post their own results through
//! whatever channels they capture (the LSP `out_tx`/`done_tx`), so the pool
//! needs no result channel of its own — just [`Spawner::spawn`].

use std::thread::JoinHandle;

use crossbeam_channel::Sender;

/// A boxed unit of work to run on a worker thread.
type Job = Box<dyn FnOnce() + Send + 'static>;

/// A fixed pool of worker threads consuming boxed closures.
///
/// Owns the worker [`JoinHandle`]s but never joins them: they exit on their own
/// once every [`Spawner`] (and the pool's own `job_tx`) drops and their receiver
/// disconnects. Detaching keeps shutdown from blocking on an in-flight job.
pub(crate) struct TaskPool {
    job_tx: Sender<Job>,
    _workers: Vec<JoinHandle<()>>,
}

impl TaskPool {
    /// Spawn `n` worker threads (clamped to at least 1), each named `name`.
    pub(crate) fn new(name: &'static str, n: usize) -> Self {
        let n = n.max(1);
        let (job_tx, job_rx) = crossbeam_channel::unbounded::<Job>();
        let workers = (0..n)
            .map(|_| {
                let job_rx = job_rx.clone();
                std::thread::Builder::new()
                    .name(name.to_owned())
                    .spawn(move || {
                        // Exits cleanly when all `job_tx` clones drop.
                        for job in job_rx {
                            // Catch genuine panics so one buggy job can't
                            // permanently take a worker out of rotation — rayon
                            // isolated panics per task, and raw threads don't.
                            // Salsa `Cancelled` never reaches here: the read
                            // helpers and the analyze site catch it upstream.
                            if let Err(panic) =
                                std::panic::catch_unwind(std::panic::AssertUnwindSafe(job))
                            {
                                let msg = panic
                                    .downcast_ref::<&'static str>()
                                    .copied()
                                    .or_else(|| panic.downcast_ref::<String>().map(String::as_str))
                                    .unwrap_or("<non-string panic payload>");
                                log::error!("LSP task pool worker caught panic: {msg}");
                            }
                        }
                    })
                    .expect("failed to spawn LSP worker thread")
            })
            .collect();
        Self {
            job_tx,
            _workers: workers,
        }
    }

    /// A cheap, cloneable handle for submitting work to this pool.
    pub(crate) fn spawner(&self) -> Spawner {
        Spawner(self.job_tx.clone())
    }
}

/// A cloneable submit-side handle onto a [`TaskPool`], shareable across the main
/// loop and the worker thread.
#[derive(Clone)]
pub(crate) struct Spawner(Sender<Job>);

impl Spawner {
    /// Hand a closure to the pool. It runs on some worker thread. Sending only
    /// fails once every worker has died, which we treat as shutdown.
    pub(crate) fn spawn(&self, f: impl FnOnce() + Send + 'static) {
        let _ = self.0.send(Box::new(f));
    }
}

/// Worker count for the read pool: the machine's available parallelism.
pub(crate) fn read_pool_size() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn every_spawned_job_runs() {
        let pool = TaskPool::new("test-pool", 4);
        let spawner = pool.spawner();
        let (tx, rx) = crossbeam_channel::unbounded::<usize>();
        const N: usize = 64;
        for i in 0..N {
            let tx = tx.clone();
            spawner.spawn(move || {
                let _ = tx.send(i);
            });
        }
        drop(tx);
        let mut seen: Vec<usize> = rx.iter().collect();
        seen.sort_unstable();
        assert_eq!(seen, (0..N).collect::<Vec<_>>());
    }

    #[test]
    fn panicking_job_does_not_kill_the_pool() {
        // A single worker runs jobs in submission order: the panic lands first,
        // then the survivor must still run on the same (only) worker. If the
        // panic took the worker out of rotation, the survivor never runs.
        let pool = TaskPool::new("test-pool-panic", 1);
        let spawner = pool.spawner();
        let ran = Arc::new(AtomicUsize::new(0));

        spawner.spawn(|| panic!("boom"));

        let ran2 = Arc::clone(&ran);
        let (done_tx, done_rx) = crossbeam_channel::bounded::<()>(1);
        spawner.spawn(move || {
            ran2.fetch_add(1, Ordering::SeqCst);
            let _ = done_tx.send(());
        });

        done_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("survivor job should run after a panicking job");
        assert_eq!(ran.load(Ordering::SeqCst), 1);
    }
}
