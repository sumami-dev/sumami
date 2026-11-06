//! The admission gate: what may be in flight, and how fast it may come in.
//!
//! The collection path is the only place in the process where the amount of work is
//! decided by somebody else — the traffic. Everything else has a fixed size. So the
//! collection path needs its own limits, and they have to sit **before the body is
//! read**, because the body is what costs the memory:
//!
//! ```text
//! POST /api/send
//!   └─ the headers only              ← nothing is allocated yet
//!      └─ ADMIT
//!         ├─ is the body within the per-request limit?   → 413
//!         ├─ is there a rate token?                      → 503
//!         ├─ is there room in the in-flight requests?    → 503
//!         └─ is there room in the in-flight bytes?       → 503
//!      └─ the body is read             ← now the memory is spent
//!         └─ parsed, and queued
//! ```
//!
//! **A refusal must happen before the allocation it refuses.** The queue's own
//! `503` (the writer is full) is raised *after* the body has been read and parsed —
//! it protects the queue, not the box.
//!
//! Every limit is a knob ([`Limits`]) and **zero means "no limit"**, so the gate
//! changes nothing until somebody turns it on. The numbers are measured, not
//! guessed: see `MEMORY.md` and [issue #14](https://github.com/sumimi-dev/sumimi/issues/14).

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use axum::extract::{Request, State};
use axum::http::header::{CONTENT_LENGTH, RETRY_AFTER};
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use crate::api::AppState;

/// The default body limit — axum's own default (2 MB).
pub const DEFAULT_MAX_BODY_BYTES: usize = 2 * 1024 * 1024;

/// The limits of the collection path.
///
/// **Zero means "no limit"** for everything but the body size: the gate is off
/// until somebody turns it on, so adding it does not change the server's behaviour
/// by itself. The defaults are the ones the measurements will pick; until then they
/// are the numbers the server has always used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// The largest request body the collector will read, in bytes.
    pub max_body_bytes: usize,
    /// How many collection requests may be in flight (being read or parsed) at once.
    pub max_inflight_requests: u64,
    /// How many bytes may be in flight at once.
    pub max_inflight_bytes: u64,
    /// How many collection requests per second are admitted.
    pub max_admit_rps: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_body_bytes: DEFAULT_MAX_BODY_BYTES,
            max_inflight_requests: 0,
            max_inflight_bytes: 0,
            max_admit_rps: 0,
        }
    }
}

/// The gate's live counters — what the heartbeat reports.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GateStats {
    /// Requests being read or parsed right now.
    pub inflight_requests: u64,
    /// Bytes being read or parsed right now.
    pub inflight_bytes: u64,
    /// Requests refused by the rate limit.
    pub refused_rate: u64,
    /// Requests refused because too many were in flight.
    pub refused_inflight: u64,
    /// Requests refused because the body was too large.
    pub refused_body: u64,
}

/// The gate.
pub struct Gate {
    limits: Limits,
    inflight_requests: AtomicU64,
    inflight_bytes: AtomicU64,
    refused_rate: AtomicU64,
    refused_inflight: AtomicU64,
    refused_body: AtomicU64,
    /// The token bucket, in thousandths of a request.
    tokens: AtomicU64,
    /// When the bucket was last filled, in nanoseconds since the epoch.
    stamp_ns: AtomicU64,
    /// When the gate was made — the bucket's clock starts here.
    epoch: Instant,
}

impl Gate {
    /// Build a gate.
    pub fn new(limits: Limits) -> Self {
        Self {
            limits,
            inflight_requests: AtomicU64::new(0),
            inflight_bytes: AtomicU64::new(0),
            refused_rate: AtomicU64::new(0),
            refused_inflight: AtomicU64::new(0),
            refused_body: AtomicU64::new(0),
            // The bucket starts **full**: the server has not done anything wrong
            // yet, and a rate limit must not refuse the first request of a burst
            // just because the bucket has not been filled yet.
            tokens: AtomicU64::new(limits.max_admit_rps.saturating_mul(1000)),
            stamp_ns: AtomicU64::new(0),
            epoch: Instant::now(),
        }
    }

    /// The limits this gate enforces.
    pub fn limits(&self) -> &Limits {
        &self.limits
    }

    /// The live counters.
    pub fn stats(&self) -> GateStats {
        GateStats {
            inflight_requests: self.inflight_requests.load(Ordering::Relaxed),
            inflight_bytes: self.inflight_bytes.load(Ordering::Relaxed),
            refused_rate: self.refused_rate.load(Ordering::Relaxed),
            refused_inflight: self.refused_inflight.load(Ordering::Relaxed),
            refused_body: self.refused_body.load(Ordering::Relaxed),
        }
    }

    /// Admit a request of `bytes` bytes, or refuse it.
    ///
    /// The reservation is held until the returned permit is dropped — which is
    /// when the request is finished, body read and hit queued.
    pub fn try_admit(&self, bytes: u64) -> Option<Permit<'_>> {
        if !self.take_token() {
            self.refused_rate.fetch_add(1, Ordering::Relaxed);
            return None;
        }

        // The counters are always tracked (the heartbeat reports them), but only
        // the ones with a limit can refuse.
        let previous = self.inflight_requests.fetch_add(1, Ordering::AcqRel);
        if self.limits.max_inflight_requests > 0 && previous >= self.limits.max_inflight_requests {
            self.inflight_requests.fetch_sub(1, Ordering::AcqRel);
            self.refused_inflight.fetch_add(1, Ordering::Relaxed);
            return None;
        }

        let previous = self.inflight_bytes.fetch_add(bytes, Ordering::AcqRel);
        if self.limits.max_inflight_bytes > 0
            && previous.saturating_add(bytes) > self.limits.max_inflight_bytes
        {
            self.inflight_bytes.fetch_sub(bytes, Ordering::AcqRel);
            self.inflight_requests.fetch_sub(1, Ordering::AcqRel);
            self.refused_inflight.fetch_add(1, Ordering::Relaxed);
            return None;
        }

        Some(Permit {
            gate: self,
            bytes,
        })
    }

    /// Take one token, if the bucket has one.
    fn take_token(&self) -> bool {
        let rps = self.limits.max_admit_rps;
        if rps == 0 {
            return true;
        }
        self.refill(rps);
        self.tokens
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |tokens| {
                // `then`, not `then_some`: the subtraction must not happen when
                // the bucket is empty.
                (tokens >= 1000).then(|| tokens - 1000)
            })
            .is_ok()
    }

    /// Fill the bucket with the time that has passed since the last fill.
    fn refill(&self, rps: u64) {
        let now = self.now_ns();
        let last = self.stamp_ns.load(Ordering::Relaxed);
        if now <= last {
            return;
        }
        // Move the stamp first: if another thread got there first, it has already
        // filled the bucket for this window.
        if self
            .stamp_ns
            .compare_exchange(last, now, Ordering::Relaxed, Ordering::Relaxed)
            .is_err()
        {
            return;
        }
        // The tokens are in thousandths of a request.
        let added = (now - last) as u128 * rps as u128 * 1000 / 1_000_000_000;
        if added == 0 {
            return;
        }
        // The bucket holds at most one second's worth of requests.
        let cap = rps.saturating_mul(1000);
        let _ = self.tokens.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |tokens| {
            Some(tokens.saturating_add(added as u64).min(cap))
        });
    }

    /// The nanoseconds since the gate was made.
    fn now_ns(&self) -> u64 {
        self.epoch.elapsed().as_nanos() as u64
    }
}

/// The right to be in flight — it gives the room back when it is dropped.
pub struct Permit<'a> {
    gate: &'a Gate,
    bytes: u64,
}

impl Drop for Permit<'_> {
    fn drop(&mut self) {
        self.gate.inflight_requests.fetch_sub(1, Ordering::AcqRel);
        self.gate.inflight_bytes.fetch_sub(self.bytes, Ordering::AcqRel);
    }
}

/// The gate, as an axum middleware.
///
/// It runs **before the body is read**: the headers are all it needs to decide, and
/// by the time the handler runs, the request has already earned its place.
pub async fn admit(State(state): State<AppState>, request: Request, next: Next) -> Response {
    let gate = state.gate.clone();
    let limits = *gate.limits();

    // How big is the body? The header says — and the body has not been touched yet.
    let declared = request
        .headers()
        .get(CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok());

    if let Some(length) = declared {
        if length > limits.max_body_bytes as u64 {
            gate.refused_body.fetch_add(1, Ordering::Relaxed);
            return too_large();
        }
    }

    // A request without a length (chunked) may still be the largest one we allow:
    // reserve that much, so the budget cannot be spent by a body we have not seen.
    let reservation = declared.unwrap_or(limits.max_body_bytes as u64);

    // The permit lives until the response is produced — the body has been read and
    // the hit is in the queue by then.
    let Some(_permit) = gate.try_admit(reservation) else {
        return overloaded();
    };

    next.run(request).await
}

/// The body is too large — refused before it is read.
fn too_large() -> Response {
    StatusCode::PAYLOAD_TOO_LARGE.into_response()
}

/// The server is over capacity — the same answer the writer gives when the queue is
/// full: `503` with a `Retry-After`. What we acknowledged, we have.
fn overloaded() -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        [(RETRY_AFTER, "1")],
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A gate with no limits at all lets everything through.
    #[test]
    fn no_limits_lets_everything_through() {
        let gate = Gate::new(Limits::default());
        let permit = gate.try_admit(1024).expect("unlimited gate");
        drop(permit);
        assert_eq!(gate.stats().inflight_requests, 0);
        assert_eq!(gate.stats().refused_inflight, 0);
    }

    /// The in-flight request limit refuses the request that would exceed it.
    #[test]
    fn the_inflight_request_limit_refuses() {
        let gate = Gate::new(Limits {
            max_inflight_requests: 2,
            ..Limits::default()
        });
        let first = gate.try_admit(10).expect("the first is admitted");
        let second = gate.try_admit(10).expect("the second is admitted");
        assert!(gate.try_admit(10).is_none(), "the third is refused");
        assert_eq!(gate.stats().refused_inflight, 1);
        drop(first);
        // The room that was given back is room again.
        assert!(gate.try_admit(10).is_some());
        drop(second);
    }

    /// The in-flight byte limit refuses the request that would exceed it.
    #[test]
    fn the_inflight_byte_limit_refuses() {
        let gate = Gate::new(Limits {
            max_inflight_bytes: 1000,
            ..Limits::default()
        });
        let first = gate.try_admit(900).expect("900 bytes fit");
        assert!(gate.try_admit(200).is_none(), "1100 bytes do not fit");
        assert_eq!(gate.stats().inflight_bytes, 900);
        drop(first);
        assert_eq!(gate.stats().inflight_bytes, 0);
        assert!(gate.try_admit(1000).is_some());
    }

    /// The rate limit hands out one request per window and refuses the rest.
    #[test]
    fn the_rate_limit_refuses_the_extra() {
        let gate = Gate::new(Limits {
            max_admit_rps: 1,
            ..Limits::default()
        });
        // The bucket starts full: one second's worth of one request per second.
        assert!(gate.try_admit(10).is_some(), "the first is admitted");
        assert!(gate.try_admit(10).is_none(), "the second is refused");
        assert_eq!(gate.stats().refused_rate, 1);
    }

    /// A refused request leaves nothing behind.
    #[test]
    fn a_refused_request_leaves_nothing_behind() {
        let gate = Gate::new(Limits {
            max_inflight_bytes: 100,
            ..Limits::default()
        });
        assert!(gate.try_admit(500).is_none());
        assert_eq!(gate.stats().inflight_requests, 0);
        assert_eq!(gate.stats().inflight_bytes, 0);
    }
}
