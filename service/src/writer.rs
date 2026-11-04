//! The single-writer pipeline.
//!
//! HTTP handlers never touch the database. They hand a [`Hit`] to the [`Writer`], which
//! buffers it in memory and hands batches to one dedicated thread that owns the DuckDB
//! connection. Batches are flushed every few seconds inside a single transaction.
//!
//! Two properties matter:
//!   * **One writer.** DuckDB has a single-writer model anyway; owning the connection in
//!     one thread removes all locking from the request path.
//!   * **Bounded memory.** If the buffer overflows (a traffic spike, a slow disk), the
//!     *oldest* hits are dropped and counted. Analytics may lose a few samples; the server
//!     must not die — and it says so in the log, because a lost hit is a lost hit.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use crate::models::Hit;
use crate::storage::Storage;

/// Shared, bounded buffer between the request handlers and the writer thread.
#[derive(Debug)]
struct Buffer {
    queue: Mutex<VecDeque<Hit>>,
    signal: Condvar,
    dropped: AtomicU64,
    /// Set when the writer should stop.
    stopped: AtomicBool,
}

impl Buffer {
    fn new() -> Self {
        Self {
            queue: Mutex::new(VecDeque::new()),
            signal: Condvar::new(),
            dropped: AtomicU64::new(0),
            stopped: AtomicBool::new(false),
        }
    }

    /// Ask the writer to stop (it flushes what it has and returns).
    fn stop(&self) {
        self.stopped.store(true, Ordering::Relaxed);
        self.signal.notify_all();
    }

    /// Has the writer been asked to stop?
    fn is_stopped(&self) -> bool {
        self.stopped.load(Ordering::Relaxed)
    }

    /// Push a hit, dropping the oldest one when the buffer is over capacity.
    fn push(&self, hit: Hit, capacity: usize) {
        let mut queue = self.queue.lock().unwrap_or_else(|e| e.into_inner());
        queue.push_back(hit);
        while queue.len() > capacity {
            queue.pop_front();
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
        self.signal.notify_one();
    }

    /// Take everything currently buffered.
    fn drain(&self) -> Vec<Hit> {
        let mut queue = self.queue.lock().unwrap_or_else(|e| e.into_inner());
        queue.drain(..).collect()
    }
}

/// The writer handle handed to the HTTP layer.
#[derive(Clone)]
pub struct Writer {
    buffer: Arc<Buffer>,
    capacity: usize,
    /// The version of the data.
    ///
    /// It changes whenever a batch is written, and the report cache carries it:
    /// a cached answer from an older version is a miss. That is what keeps the
    /// dashboard from showing numbers from before the last write, while two
    /// people looking at the same dashboard only run the query once.
    version: Arc<AtomicU64>,
}

impl Writer {
    /// Spawn the writer thread and return the handle.
    pub fn spawn(
        storage: Arc<dyn Storage>,
        flush_interval: Duration,
        capacity: usize,
    ) -> (Writer, tokio::task::JoinHandle<()>) {
        let buffer = Arc::new(Buffer::new());
        let writer = Writer {
            buffer: buffer.clone(),
            capacity,
            // The clock, so a restart invalidates the cache that is still on disk.
            version: Arc::new(AtomicU64::new(
                chrono::Utc::now().timestamp_millis().max(0) as u64,
            )),
        };

        let handle = tokio::task::spawn_blocking({
            let writer = writer.clone();
            move || {
                writer_loop(storage, buffer, flush_interval, capacity, writer);
            }
        });

        (writer, handle)
    }

    /// Queue a hit for the next flush.
    pub fn push(&self, hit: Hit) {
        self.buffer.push(hit, self.capacity);
    }

    /// Number of hits waiting in memory.
    pub fn queued(&self) -> u64 {
        self.buffer.queue.lock().unwrap_or_else(|e| e.into_inner()).len() as u64
    }

    /// Number of hits dropped because the buffer was full.
    pub fn dropped(&self) -> u64 {
        self.buffer.dropped.load(Ordering::Relaxed)
    }

    /// The version of the data.
    ///
    /// It changes whenever a batch is written.
    pub fn version(&self) -> &AtomicU64 {
        &self.version
    }

    /// Stop the writer thread.
    ///
    /// The thread flushes whatever is still buffered and returns, so the process (and
    /// the Tokio runtime) can shut down cleanly.
    pub fn shutdown(&self) {
        self.buffer.stop();
    }
}

/// The writer thread: wait for hits, drain, persist.
///
/// The event table is **append only**, so this is a bulk append — the fastest path the
/// engine has (measured 641x faster than an upsert, see issue #17).
///
/// **A failed write does not end the thread.** A hiccup — a locked file, a disk that
/// hiccups, a database that is briefly busy — must not cost the site owner the
/// traffic. The batch that failed is **kept** and written again, with a pause
/// between the attempts. The thread only gives up when it is told to stop, and
/// then it writes what is left before it leaves.
fn writer_loop(
    storage: Arc<dyn Storage>,
    buffer: Arc<Buffer>,
    flush_interval: Duration,
    _capacity: usize,
    writer: Writer,
) {
    // The batch that could not be written yet. It stays here until it is.
    let mut pending: Vec<Hit> = Vec::new();
    let mut failures = 0u32;
    let mut reported_dropped = 0u64;

    loop {
        if buffer.is_stopped() {
            // Flush what is left — and what could not be written — then leave.
            let mut batch = std::mem::take(&mut pending);
            batch.extend(buffer.drain());
            if !batch.is_empty() {
                if let Err(err) = storage.write_batch(&batch) {
                    tracing::error!(
                        error = %err,
                        count = batch.len(),
                        "the last batch could not be written — it is lost"
                    );
                }
            }
            tracing::debug!("sumimi writer stopped");
            break;
        }

        // Block until something arrives or the flush interval elapses.
        {
            let queue = buffer.queue.lock().unwrap_or_else(|e| e.into_inner());
            if queue.is_empty() && pending.is_empty() {
                let _ = buffer
                    .signal
                    .wait_timeout(queue, flush_interval)
                    .unwrap_or_else(|e| e.into_inner());
            }
        }

        let mut batch = std::mem::take(&mut pending);
        batch.extend(buffer.drain());
        if batch.is_empty() {
            continue;
        }

        let count = batch.len();
        match storage.write_batch(&batch) {
            Ok(()) => {
                failures = 0;
                // The data changed: every cached report is stale now.
                writer.version().fetch_add(1, Ordering::Relaxed);
                tracing::debug!(count, "flushed batch");
            }
            Err(err) => {
                // Keep the batch and try again. The pause grows with the number of
                // failures, so a database that is down is not hammered.
                failures += 1;
                let pause = Duration::from_millis(250u64.saturating_mul(failures as u64).min(8_000));
                tracing::error!(
                    error = %err,
                    count,
                    failures,
                    "failed to write batch — keeping it and trying again"
                );
                pending = batch;
                std::thread::sleep(pause);
            }
        }

        // Say it when hits are dropped: the buffer was full, so the **oldest**
        // hits are gone. That is data the owner cannot get back — it is said
        // once per batch, not once per hit.
        let dropped = writer.dropped();
        if dropped > reported_dropped {
            tracing::warn!(
                dropped,
                "hits were dropped — the queue was full (raise SUMIMI_MAX_QUEUE)"
            );
            reported_dropped = dropped;
        }
    }
}
