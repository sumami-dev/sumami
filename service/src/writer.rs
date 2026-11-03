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
//!     must not die.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use anyhow::Result;

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
        };

        let handle = tokio::task::spawn_blocking(move || {
            writer_loop(storage, buffer, flush_interval, capacity);
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

    /// Stop the writer thread.
    ///
    /// The thread flushes whatever is still buffered and returns, so the process (and
    /// the Tokio runtime) can shut down cleanly.
    pub fn shutdown(&self) {
        self.buffer.stop();
    }
}

/// The writer thread: wait for hits, drain, persist.
fn writer_loop(
    storage: Arc<dyn Storage>,
    buffer: Arc<Buffer>,
    flush_interval: Duration,
    _capacity: usize,
) {
    loop {
        if buffer.is_stopped() {
            // Flush what is left, then leave.
            let batch = buffer.drain();
            if !batch.is_empty() {
                let _ = storage.write_batch(&batch);
            }
            tracing::debug!("sumami writer stopped");
            break;
        }
        if let Err(err) = write_loop_step(&storage, &buffer, flush_interval) {
            tracing::error!(error = %err, "sumami writer thread stopped");
            break;
        }
    }
}

fn write_loop_step(
    storage: &Arc<dyn Storage>,
    buffer: &Arc<Buffer>,
    flush_interval: Duration,
) -> Result<()> {
    // Block until something arrives or the flush interval elapses.
    {
        let queue = buffer.queue.lock().unwrap_or_else(|e| e.into_inner());
        if queue.is_empty() {
            let _ = buffer
                .signal
                .wait_timeout(queue, flush_interval)
                .unwrap_or_else(|e| e.into_inner());
        }
    }

    let batch = buffer.drain();
    if batch.is_empty() {
        return Ok(());
    }

    let count = batch.len();
    let written = storage.write_batch(&batch);
    if let Err(err) = written {
        tracing::error!(error = %err, count, "failed to write batch");
        return Err(err);
    }
    tracing::debug!(count, "flushed batch");
    Ok(())
}
