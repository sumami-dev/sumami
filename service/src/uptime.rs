//! Uptime: is the website actually answering?
//!
//! Analytics tells you how much traffic arrived. It cannot tell you whether
//! the site was **down** — a site that is down gets no traffic, which looks
//! exactly like a site nobody wants.
//!
//! So the service checks the websites it knows about and keeps the answers.
//!
//! # The one thing that makes it useful: ruling out **our** network
//!
//! A failed check means one of two things: the site is down, or **we** cannot
//! reach the internet. Those need different reactions, so the checker does not
//! guess:
//!
//! when the check fails, it also checks two references —
//!
//! | reference | what it rules out |
//! | --- | --- |
//! | `https://www.bing.com/` | the DNS and the HTTP path |
//! | `8.8.8.8:53` (a TCP connect) | the network itself, with no name lookup involved |
//!
//! * the references answer → **the site is down**;
//! * they do not → **we are offline** — and the check says so instead of
//!   blaming the site.
//!
//! A check that succeeded needs no references: the site answered, so the
//! network was fine.

use std::net::{SocketAddr, TcpStream};
use std::time::{Duration, Instant};

use chrono::NaiveDateTime;
use serde::{Deserialize, Serialize};

/// One check of one website.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UptimeCheck {
    /// The website that was checked.
    pub website_id: String,
    /// When the check happened.
    pub at: NaiveDateTime,
    /// Did the website answer?
    pub ok: bool,
    /// The HTTP status, if the server answered at all.
    pub status: Option<u16>,
    /// How long it took, in milliseconds.
    pub latency_ms: Option<u64>,
    /// What went wrong, if something did.
    pub error: Option<String>,
    /// Was the network itself fine?
    ///
    /// `None` when the check succeeded — the question did not arise.
    /// `Some(false)` when the references failed too: **we** are offline, and
    /// the site is not to blame.
    pub network_ok: Option<bool>,
}

/// What a check found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// The website answered.
    Up {
        /// The HTTP status.
        status: u16,
        /// How long it took, in milliseconds.
        latency_ms: u64,
    },
    /// The website did not answer, and the network is fine — **the site is
    /// down**.
    Down {
        /// What went wrong.
        error: String,
    },
    /// The website did not answer, and neither did the references — **we are
    /// offline**, so nothing can be said about the site.
    NetworkDown {
        /// What went wrong with the website.
        error: String,
        /// What went wrong with the references.
        references: Vec<String>,
    },
}

impl Verdict {
    /// Is the website up?
    pub fn is_up(&self) -> bool {
        matches!(self, Verdict::Up { .. })
    }

    /// The one-line summary.
    pub fn summary(&self) -> String {
        match self {
            Verdict::Up {
                status,
                latency_ms,
            } => format!("up — {status} in {latency_ms} ms"),
            Verdict::Down { error } => format!("down — {error}"),
            Verdict::NetworkDown {
                error,
                references,
            } => format!(
                "cannot tell — {error} (and our own network failed: {})",
                references.join("; ")
            ),
        }
    }
}

/// The checker: it asks a website whether it is there.
#[derive(Debug, Clone)]
pub struct Checker {
    /// How long to wait for an answer.
    pub timeout: Duration,
    /// The references that rule out our own network.
    pub references: Vec<String>,
    /// The address that is connected to directly (no name lookup).
    pub reference_address: String,
}

impl Default for Checker {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(10),
            // The two the issue asks for.
            references: vec!["https://www.bing.com/".to_string()],
            reference_address: "8.8.8.8:53".to_string(),
        }
    }
}

impl Checker {
    /// A checker with a given timeout.
    pub fn with_timeout(timeout: Duration) -> Self {
        Self {
            timeout,
            ..Self::default()
        }
    }

    /// Check a URL.
    ///
    /// This is a **blocking** call: it is made from a thread that is allowed to
    /// block (`spawn_blocking`), never from the request path.
    pub fn check(&self, url: &str) -> Verdict {
        let started = Instant::now();
        match self.request(url) {
            Ok((status, _)) => {
                let latency = started.elapsed().as_millis() as u64;
                // The server answered. A 404 or a 500 still means it is there.
                if status < 500 {
                    Verdict::Up {
                        status,
                        latency_ms: latency,
                    }
                } else {
                    Verdict::Down {
                        error: format!("the server answered with {status}"),
                    }
                }
            }
            Err(error) => {
                // It did not answer. Is that the site, or us?
                let references = self.check_references();
                if references.is_empty() {
                    Verdict::Down { error }
                } else {
                    Verdict::NetworkDown {
                        error,
                        references,
                    }
                }
            }
        }
    }

    /// The check as a record, ready to be stored.
    pub fn record(&self, website_id: &str, url: &str) -> UptimeCheck {
        let at = crate::db::now();
        let started = Instant::now();
        let (ok, status, latency_ms, error, network_ok) = match self.request(url) {
            Ok((status, _)) => {
                let latency = started.elapsed().as_millis() as u64;
                if status < 500 {
                    (true, Some(status), Some(latency), None, None)
                } else {
                    (
                        false,
                        Some(status),
                        Some(latency),
                        Some(format!("the server answered with {status}")),
                        None,
                    )
                }
            }
            Err(error) => {
                let references = self.check_references();
                let network_ok = Some(references.is_empty());
                let error = if references.is_empty() {
                    error
                } else {
                    format!("{error} (our own network failed too: {})", references.join("; "))
                };
                (false, None, None, Some(error), network_ok)
            }
        };

        UptimeCheck {
            website_id: website_id.to_string(),
            at,
            ok,
            status,
            latency_ms,
            error,
            network_ok,
        }
    }

    /// Ask a URL for its page.
    fn request(&self, url: &str) -> Result<(u16, String), String> {
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(self.timeout))
            .build()
            .new_agent();

        match agent.head(url).call() {
            Ok(response) => Ok((response.status().as_u16(), String::new())),
            // A status code is still an answer: the server is there.
            Err(ureq::Error::StatusCode(code)) => Ok((code, String::new())),
            Err(err) => Err(err.to_string()),
        }
    }

    /// Check the references. An empty list means "the network is fine".
    fn check_references(&self) -> Vec<String> {
        let mut failures = Vec::new();

        // A plain TCP connection: no name lookup, so it isolates the network.
        if let Some(address) = self
            .reference_address
            .parse::<SocketAddr>()
            .ok()
            .or_else(|| {
                use std::net::ToSocketAddrs;
                self.reference_address.to_socket_addrs().ok().and_then(|mut a| a.next())
            }) {
            if TcpStream::connect_timeout(&address, self.timeout).is_err() {
                failures.push(format!("{} is unreachable", self.reference_address));
            }
        } else {
            failures.push(format!("{} could not be looked up", self.reference_address));
        }

        // And one that needs the whole path: DNS and HTTP.
        for reference in &self.references {
            let agent = ureq::Agent::config_builder()
                .timeout_global(Some(self.timeout))
                .build()
                .new_agent();
            match agent.head(reference).call() {
                Ok(_) => {}
                Err(ureq::Error::StatusCode(_)) => {}
                Err(err) => failures.push(format!("{reference}: {err}")),
            }
        }

        failures
    }
}

/// The availability of a website over a set of checks.
///
/// The percentage is over the checks that could say something: a check that
/// failed because **we** were offline says nothing about the site, so it is
/// left out of the count (and counted separately).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Availability {
    /// How many checks there were.
    pub checks: usize,
    /// How many of them the website answered.
    pub up: usize,
    /// The percentage, from 0 to 100.
    pub percent: f64,
    /// How many checks could not say anything (we were offline).
    pub unknown: usize,
    /// The average latency of the checks that answered, in milliseconds.
    pub average_latency_ms: Option<u64>,
}

impl Availability {
    /// Work it out from the checks.
    pub fn of(checks: &[UptimeCheck]) -> Availability {
        let total = checks.len();
        let unknown = checks.iter().filter(|check| check.network_ok == Some(false)).count();
        let answered: Vec<&UptimeCheck> = checks.iter().filter(|check| check.ok).collect();
        let up = answered.len();
        let counted = total - unknown;
        let percent = if counted == 0 {
            0.0
        } else {
            (up as f64 / counted as f64) * 100.0
        };
        let average_latency_ms = if answered.is_empty() {
            None
        } else {
            let sum: u64 = answered.iter().filter_map(|check| check.latency_ms).sum();
            Some(sum / answered.len() as u64)
        };

        Availability {
            checks: total,
            up,
            percent: (percent * 10.0).round() / 10.0,
            unknown,
            average_latency_ms,
        }
    }
}
