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
use chrono::NaiveDateTime;
use duckdb::Connection;

use crate::models::{Hit, HitKind};
use crate::pool::Db;

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
        db: Arc<Db>,
        flush_interval: Duration,
        capacity: usize,
    ) -> (Writer, tokio::task::JoinHandle<()>) {
        let buffer = Arc::new(Buffer::new());
        let writer = Writer {
            buffer: buffer.clone(),
            capacity,
        };

        let handle = tokio::task::spawn_blocking(move || {
            writer_loop(db, buffer, flush_interval, capacity);
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
    db: Arc<Db>,
    buffer: Arc<Buffer>,
    flush_interval: Duration,
    _capacity: usize,
) {
    loop {
        if buffer.is_stopped() {
            // Flush what is left, then leave.
            let batch = buffer.drain();
            if !batch.is_empty() {
                let _ = db.with_write(|conn| write_batch(conn, &batch));
            }
            tracing::debug!("sumami writer stopped");
            break;
        }
        if let Err(err) = write_loop_step(&db, &buffer, flush_interval) {
            tracing::error!(error = %err, "sumami writer thread stopped");
            break;
        }
    }
}

fn write_loop_step(
    db: &Arc<Db>,
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
    let written = db.with_write(|conn| write_batch(conn, &batch));
    if let Err(err) = written {
        tracing::error!(error = %err, count, "failed to write batch");
        return Err(err);
    }
    tracing::debug!(count, "flushed batch");
    Ok(())
}

/// Persist a batch of hits in one transaction.
pub fn write_batch(conn: &Connection, batch: &[Hit]) -> Result<()> {
    conn.execute_batch("BEGIN TRANSACTION")?;

    let result = (|| -> Result<()> {
        // Group pageviews by session so each session is upserted once.
        let mut sessions: Vec<SessionAggregate> = Vec::new();
        let mut events: Vec<&Hit> = Vec::new();

        let now = crate::db::now();

        for hit in batch {
            // Retention: drop hits that are older than the website's window.
            if hit.retention_days > 0 {
                let cutoff = now - chrono::Duration::days(hit.retention_days as i64);
                if hit.created_at < cutoff {
                    continue;
                }
            }

            match hit.kind {
                HitKind::Pageview => match sessions.iter_mut().find(|s| s.id == hit.session_id) {
                    Some(existing) => {
                        existing.merge(hit);
                    }
                    None => sessions.push(SessionAggregate::from(hit)),
                },
                HitKind::Event | HitKind::Screenview => events.push(hit),
                HitKind::Identify => {
                    if let Some(existing) = sessions.iter_mut().find(|s| s.id == hit.session_id) {
                        existing.data = hit.data.clone();
                    } else {
                        let mut agg = SessionAggregate::from(hit);
                        agg.views = 0;
                        sessions.push(agg);
                    }
                }
            }
        }

        for session in &sessions {
            upsert_session(conn, session)?;
        }

        for hit in events {
            insert_event(conn, hit)?;
        }

        Ok(())
    })();

    match result {
        Ok(()) => {
            conn.execute_batch("COMMIT")?;
            Ok(())
        }
        Err(err) => {
            let _ = conn.execute_batch("ROLLBACK");
            Err(err)
        }
    }
}

/// Aggregated view of a session within one batch.
struct SessionAggregate {
    id: String,
    website_id: String,
    visitor_id: Option<String>,
    created_at: NaiveDateTime,
    updated_at: NaiveDateTime,
    hostname: Option<String>,
    url: Option<String>,
    title: Option<String>,
    referrer: Option<String>,
    entry_url: Option<String>,
    browser: Option<String>,
    browser_version: Option<String>,
    os: Option<String>,
    os_version: Option<String>,
    device: Option<String>,
    country: Option<String>,
    region: Option<String>,
    city: Option<String>,
    language: Option<String>,
    screen: Option<String>,
    data: Option<String>,
    views: i64,
}

impl SessionAggregate {
    fn from(hit: &Hit) -> Self {
        Self {
            id: hit.session_id.clone(),
            website_id: hit.website_id.clone(),
            visitor_id: Some(hit.visitor_id.clone()),
            created_at: hit.created_at,
            updated_at: hit.created_at,
            hostname: hit.hostname.clone(),
            url: hit.url.clone(),
            title: hit.title.clone(),
            referrer: hit.referrer.clone(),
            entry_url: hit.url.clone(),
            browser: hit.browser.clone(),
            browser_version: hit.browser_version.clone(),
            os: hit.os.clone(),
            os_version: hit.os_version.clone(),
            device: hit.device.clone(),
            country: hit.country.clone(),
            region: hit.region.clone(),
            city: hit.city.clone(),
            language: hit.language.clone(),
            screen: hit.screen.clone(),
            data: hit.data.clone(),
            // The first pageview of a session counts as one view.
            views: if hit.kind == HitKind::Pageview { 1 } else { 0 },
        }
    }

    fn merge(&mut self, hit: &Hit) {
        self.updated_at = hit.created_at;
        self.views += 1;
        if hit.url.is_some() {
            self.url = hit.url.clone();
        }
        if hit.title.is_some() {
            self.title = hit.title.clone();
        }
        if hit.referrer.is_some() {
            self.referrer = hit.referrer.clone();
        }
        if hit.screen.is_some() {
            self.screen = hit.screen.clone();
        }
        if hit.data.is_some() {
            self.data = hit.data.clone();
        }
    }
}

/// Insert or update a session row.
fn upsert_session(conn: &Connection, s: &SessionAggregate) -> Result<()> {
    conn.execute(
        r#"
INSERT INTO session (
    id, website_id, visitor_id, created_at, updated_at, hostname, url, title, referrer, entry_url,
    browser, browser_version, os, os_version, device, country, region, city, language, screen,
    data, views, duration
) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 0)
ON CONFLICT (id) DO UPDATE SET
    updated_at      = excluded.updated_at,
    url             = COALESCE(excluded.url, session.url),
    title           = COALESCE(excluded.title, session.title),
    referrer        = COALESCE(excluded.referrer, session.referrer),
    screen          = COALESCE(excluded.screen, session.screen),
    data            = COALESCE(excluded.data, session.data),
    views           = session.views + excluded.views,
    duration        = CAST(epoch(excluded.updated_at - session.created_at) AS INTEGER)
"#,
        duckdb::params![
            s.id,
            s.website_id,
            s.visitor_id,
            s.created_at,
            s.updated_at,
            s.hostname,
            s.url,
            s.title,
            s.referrer,
            s.entry_url,
            s.browser,
            s.browser_version,
            s.os,
            s.os_version,
            s.device,
            s.country,
            s.region,
            s.city,
            s.language,
            s.screen,
            s.data,
            s.views,
        ],
    )?;
    Ok(())
}

/// Insert a custom event row.
fn insert_event(conn: &Connection, hit: &Hit) -> Result<()> {
    conn.execute(
        r#"
INSERT INTO event (id, website_id, session_id, created_at, name, url, hostname, data)
VALUES (?, ?, ?, ?, ?, ?, ?, ?)
"#,
        duckdb::params![
            uuid::Uuid::new_v4().to_string(),
            hit.website_id,
            hit.session_id,
            hit.created_at,
            hit.name,
            hit.url,
            hit.hostname,
            hit.data,
        ],
    )?;
    Ok(())
}
