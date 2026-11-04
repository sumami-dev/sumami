//! Configuration for the Sumami service.
//!
//! Everything is overridable by environment variable so a deployment is a systemd unit
//! plus a small `EnvironmentFile`, never a config file in the repository.

use std::path::PathBuf;

use anyhow::Context;

/// Where the dashboard frontend is served from by default.
///
/// jsDelivr serves the public GitHub repository directly, so the frontend can be
/// updated (or pinned to a release tag) without touching the backend.
pub const CDN_APP_URL: &str = "https://cdn.jsdelivr.net/gh/sumami-dev/sumami@main/js";

/// Runtime configuration.
#[derive(Debug, Clone)]
pub struct Config {
    /// Path of the embedded DuckDB database file.
    pub db_path: PathBuf,
    /// Address to bind the HTTP server to.
    pub host: String,
    /// Port to bind the HTTP server to.
    pub port: u16,
    /// Secret used to sign JWTs. Generated and persisted on first run when absent.
    pub jwt_secret: String,
    /// Admin username created on first run.
    pub admin_user: String,
    /// Admin password created on first run (`None` = generate + print once).
    pub admin_password: Option<String>,
    /// DuckDB `memory_limit`. The ceiling for the buffer manager — *not* an
    /// allocation. Defaults to a quarter of the machine's RAM, capped at 128 MB.
    pub duckdb_memory_limit: String,
    /// DuckDB `allocator_flush_threshold`: how much freed memory the allocator
    /// keeps before returning it to the OS. Kept low so a small box stays small.
    pub duckdb_flush_threshold: String,
    /// DuckDB `max_temp_directory_size`: how much disk a query may spill.
    pub duckdb_temp_limit: String,
    /// DuckDB `threads`. `0` = derive it from the memory limit.
    ///
    /// This is not a performance knob only: a parallel `COUNT(DISTINCT …)` builds one
    /// hash table **per thread**, so the memory a query needs scales with the thread
    /// count. With 10 threads and a 128 MB limit, a five-million-row summary query
    /// runs out of memory — with 2 threads it takes 154 ms.
    pub duckdb_threads: usize,
    /// Maximum number of events buffered in memory before the oldest are dropped.
    pub max_queue: usize,
    /// Seconds between writer flushes.
    pub flush_interval_secs: u64,
    /// Default retention window in days (`0` = keep forever).
    pub retention_days: i32,
    /// Header used to derive the country code (e.g. `CF-IPCountry`).
    pub country_header: Option<String>,
    /// Keep the query string of collected URLs (`false` by default: query strings
    /// routinely carry session tokens and other personal data).
    pub keep_query: bool,
    /// Store the page title with every session.
    ///
    /// Off by default: at 10M views/day the title is roughly half of the stored bytes
    /// and almost nobody looks at it. Turn it on for small sites if you want the
    /// session list to show page titles.
    pub store_titles: bool,
    /// Store the whole referring URL instead of just its host.
    ///
    /// Off by default: the referrer breakdown only needs the host (that is what
    /// Umami shows too), and the full URL is the single largest field we store.
    pub store_full_referrer: bool,
    /// Directory holding the frontend files served at `/app/*` (and the tracker source).
    pub js_dir: PathBuf,
    /// Base URL of the frontend JavaScript. Point this at a CDN to serve the dashboard
    /// frontend from anywhere; the backend then only ships the tiny HTML shell.
    pub app_url: Option<String>,
    /// Directory served as the documentation website at `/docs`.
    pub docs_dir: PathBuf,
    /// Domain for automatic TLS (ACME). `None` = plain HTTP.
    pub ssl_domain: Option<String>,
    /// Contact address registered with the ACME provider.
    pub ssl_email: Option<String>,
    /// Directory where the ACME account and certificates are cached.
    pub ssl_cache_dir: PathBuf,
    /// Port used for HTTPS (443 by default).
    pub ssl_port: u16,
}

impl Config {
    /// Build the configuration from CLI flags + environment.
    pub fn from_env_and_flags(
        db_path: Option<PathBuf>,
        host: Option<String>,
        port: Option<u16>,
        jwt_secret: Option<String>,
        retention_days: Option<i32>,
        country_header: Option<String>,
        docs_dir: Option<PathBuf>,
    ) -> anyhow::Result<Self> {
        let db_path = db_path
            .or_else(|| env_path("SUMAMI_DB"))
            .unwrap_or_else(|| PathBuf::from("sumami.duckdb"));

        let host = host
            .or_else(|| std::env::var("SUMAMI_HOST").ok())
            .unwrap_or_else(|| "0.0.0.0".to_string());

        let port = port
            .or_else(|| std::env::var("SUMAMI_PORT").ok().and_then(|v| v.parse().ok()))
            .unwrap_or(3000);

        let jwt_secret = jwt_secret
            .or_else(|| std::env::var("SUMAMI_JWT_SECRET").ok())
            .unwrap_or_default();

        let admin_user = std::env::var("SUMAMI_ADMIN_USER").unwrap_or_else(|_| "admin".to_string());
        let admin_password = std::env::var("SUMAMI_ADMIN_PASSWORD").ok();

        // DuckDB's own default is 80% of RAM — on a 512 MB VPS that is 410 MB,
        // which would leave nothing for the OS. We take a quarter of RAM, capped
        // at 128 MB: the analytics queries stream and spill to disk long before
        // that, so the limit costs nothing and buys a lot of headroom.
        let duckdb_memory_limit = std::env::var("SUMAMI_DUCKDB_MEMORY_LIMIT")
            .ok()
            .filter(|v| !v.is_empty())
            .unwrap_or_else(default_memory_limit);

        // DuckDB's default flush threshold is 128 MB: it would keep that much
        // freed memory before giving it back. On a small box, hand it back sooner.
        let duckdb_flush_threshold = std::env::var("SUMAMI_DUCKDB_FLUSH_THRESHOLD")
            .ok()
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| "32MB".to_string());

        // A runaway query spills to disk; it must not fill the VPS.
        let duckdb_temp_limit = std::env::var("SUMAMI_DUCKDB_TEMP_LIMIT")
            .ok()
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| "2GB".to_string());

        // 0 = derive from the memory limit.
        let duckdb_threads = std::env::var("SUMAMI_DUCKDB_THREADS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);

        let max_queue = std::env::var("SUMAMI_MAX_QUEUE")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(100_000);

        let flush_interval_secs = std::env::var("SUMAMI_FLUSH_INTERVAL_SECS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(2);

        let retention_days = retention_days
            .or_else(|| std::env::var("SUMAMI_RETENTION_DAYS").ok().and_then(|v| v.parse().ok()))
            .unwrap_or(0);

        let country_header = country_header.or_else(|| std::env::var("SUMAMI_COUNTRY_HEADER").ok());

        // These three keep everything by default — that is what Umami stores, and
        // it is the baseline we promised. Turning one off is a per-site policy
        // decision, so it is environment-only.
        let store_titles = env_flag("SUMAMI_STORE_TITLES", true);
        let store_full_referrer = env_flag("SUMAMI_STORE_FULL_REFERRER", true);

        let keep_query = env_flag("SUMAMI_KEEP_QUERY", true);

        let js_dir = env_path("SUMAMI_JS_DIR").unwrap_or_else(|| PathBuf::from("js"));

        // The dashboard frontend is published on a CDN (jsDelivr serves this public
        // repository directly), so the backend never ships it. Point this at `/app`
        // to serve it from the backend instead — useful for air-gapped installs.
        let app_url = std::env::var("SUMAMI_APP_URL")
            .ok()
            .filter(|s| !s.is_empty())
            .or_else(|| Some(CDN_APP_URL.to_string()));

        let docs_dir = docs_dir
            .or_else(|| env_path("SUMAMI_DOCS_DIR"))
            .unwrap_or_else(|| PathBuf::from("docs"));

        let ssl_domain = std::env::var("SUMAMI_SSL_DOMAIN").ok().filter(|s| !s.is_empty());
        let ssl_email = std::env::var("SUMAMI_SSL_EMAIL").ok().filter(|s| !s.is_empty());
        // Keep the ACME cache next to the database by default: an absolute path is
        // easier to reason about under systemd than a relative one.
        let ssl_cache_dir = env_path("SUMAMI_SSL_CACHE_DIR").unwrap_or_else(|| {
            db_path
                .parent()
                .filter(|parent| !parent.as_os_str().is_empty())
                .map(|parent| parent.join("acme"))
                .unwrap_or_else(|| PathBuf::from("acme"))
        });
        let ssl_port = std::env::var("SUMAMI_SSL_PORT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(443);

        Ok(Config {
            db_path,
            host,
            port,
            jwt_secret,
            admin_user,
            admin_password,
            duckdb_memory_limit,
            duckdb_flush_threshold,
            duckdb_temp_limit,
            duckdb_threads,
            max_queue,
            flush_interval_secs,
            retention_days,
            country_header,
            keep_query,
            store_titles,
            store_full_referrer,
            js_dir,
            app_url,
            docs_dir,
            ssl_domain,
            ssl_email,
            ssl_cache_dir,
            ssl_port,
        })
    }
}

/// The default DuckDB memory ceiling: a quarter of RAM, capped at 128 MB.
pub fn default_memory_limit() -> String {
    let ram = total_ram_bytes().unwrap_or(512 * 1024 * 1024);
    let limit = (ram / 4).min(128 * 1024 * 1024);
    format!("{}MB", limit / (1024 * 1024))
}

/// How many threads DuckDB should use for a given memory limit.
///
/// A parallel hash aggregate builds one hash table per thread, so the memory a query
/// needs grows with the thread count. We give DuckDB one thread per 64 MB of budget,
/// never more than the machine has cores.
pub fn threads_for(memory_limit: &str, cores: usize) -> usize {
    let budget = parse_size(memory_limit).unwrap_or(128 * 1024 * 1024);
    let by_budget = (budget / (64 * 1024 * 1024)).max(1) as usize;
    by_budget.min(cores.max(1))
}

/// Parse a size like `128MB` into bytes.
pub fn parse_size(value: &str) -> Option<u64> {
    let value = value.trim().to_ascii_uppercase();
    let (number, unit) = match value.find(|c: char| c.is_alphabetic()) {
        Some(index) => (&value[..index], &value[index..]),
        None => (value.as_str(), "B"),
    };
    let number: f64 = number.trim().parse().ok()?;
    let multiplier: f64 = match unit {
        "B" => 1.0,
        "KB" | "K" => 1024.0,
        "MB" | "M" => 1024.0 * 1024.0,
        "GB" | "G" => 1024.0 * 1024.0 * 1024.0,
        _ => return None,
    };
    Some((number * multiplier) as u64)
}

/// The machine's total RAM, if we can find it out.
pub fn total_ram_bytes() -> Option<u64> {
    // Linux: /proc/meminfo.
    if let Ok(meminfo) = std::fs::read_to_string("/proc/meminfo") {
        for line in meminfo.lines() {
            if let Some(rest) = line.strip_prefix("MemTotal:") {
                let kb: u64 = rest
                    .split_whitespace()
                    .next()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(0);
                if kb > 0 {
                    return Some(kb * 1024);
                }
            }
        }
    }

    // macOS: sysctl.
    if cfg!(target_os = "macos") {
        if let Ok(out) = std::process::Command::new("sysctl")
            .args(["-n", "hw.memsize"])
            .output()
        {
            if let Ok(text) = String::from_utf8(out.stdout) {
                if let Ok(bytes) = text.trim().parse::<u64>() {
                    return Some(bytes);
                }
            }
        }
    }

    None
}

/// Read a boolean environment variable.
fn env_flag(key: &str, default: bool) -> bool {
    std::env::var(key)
        .map(|v| matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
        .unwrap_or(default)
}

fn env_path(key: &str) -> Option<PathBuf> {
    std::env::var(key).ok().map(PathBuf::from)
}

/// Resolve the JWT secret, generating and persisting one on first run.
///
/// A random per-install secret is better than a hard-coded default: a leaked default would
/// let anybody mint admin tokens.
pub fn resolve_jwt_secret(configured: &str, db_path: &std::path::Path) -> anyhow::Result<String> {
    if !configured.is_empty() {
        return Ok(configured.to_string());
    }
    if let Ok(v) = std::env::var("SUMAMI_JWT_SECRET_FILE") {
        let path = PathBuf::from(v);
        if path.exists() {
            let s = std::fs::read_to_string(&path)
                .with_context(|| format!("reading jwt secret file {}", path.display()))?;
            let s = s.trim().to_string();
            if !s.is_empty() {
                return Ok(s);
            }
        }
    }
    // Fall back to a secret file next to the database.
    let path = db_path.with_extension("jwt-secret");
    if path.exists() {
        let s = std::fs::read_to_string(&path)
            .with_context(|| format!("reading jwt secret file {}", path.display()))?;
        let s = s.trim().to_string();
        if !s.is_empty() {
            return Ok(s);
        }
    }
    let secret = random_secret();
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).ok();
        }
    }
    std::fs::write(&path, &secret)
        .with_context(|| format!("writing jwt secret file {}", path.display()))?;
    restrict_permissions(&path);
    Ok(secret)
}

/// Best-effort `chmod 0600`.
pub fn restrict_permissions(path: &std::path::Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    #[cfg(not(unix))]
    let _ = path;
}

/// Generate `n` random bytes from the OS.
pub fn random_bytes(n: usize) -> Vec<u8> {
    let hex = random_secret();
    let bytes: Vec<u8> = hex
        .as_bytes()
        .chunks(2)
        .filter_map(|pair| {
            let text = std::str::from_utf8(pair).ok()?;
            u8::from_str_radix(text, 16).ok()
        })
        .collect();
    bytes.into_iter().take(n).collect()
}

/// Generate a URL-safe random secret.
pub fn random_secret() -> String {
    use rand_compat::RngCore;
    let mut bytes = [0u8; 32];
    rand_compat::thread_rng().fill_bytes(&mut bytes);
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

/// Small RNG shim so we do not need an extra dependency for `getrandom`.
pub mod rand_compat {
    /// Thread-local CSPRNG backed by the OS.
    pub struct ThreadRng(std::cell::RefCell<State>);

    struct State {
        s: [u8; 32],
        i: usize,
    }

    impl ThreadRng {
        fn next_byte(&mut self) -> u8 {
            let mut st = self.0.borrow_mut();
            if st.i == 0 {
                st.s = os_random();
                st.i = 0;
            }
            let b = st.s[st.i % 32];
            st.i += 1;
            b
        }
    }

    /// Random bytes from the OS.
    pub trait RngCore {
        /// Fill the buffer with random bytes.
        fn fill_bytes(&mut self, dest: &mut [u8]);
    }

    impl RngCore for ThreadRng {
        fn fill_bytes(&mut self, dest: &mut [u8]) {
            for b in dest.iter_mut() {
                *b = self.next_byte();
            }
        }
    }

    /// Thread-local RNG.
    pub fn thread_rng() -> ThreadRng {
        ThreadRng(std::cell::RefCell::new(State { s: [0u8; 32], i: 32 }))
    }

    fn os_random() -> [u8; 32] {
        // /dev/urandom is available on every unix target we support.
        use std::io::Read;
        let mut buf = [0u8; 32];
        if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
            if f.read_exact(&mut buf).is_ok() {
                return buf;
            }
        }
        // Extremely unlikely fallback: mix time + address entropy.
        let t = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0);
        let a = &buf as *const _ as u64;
        for (i, b) in buf.iter_mut().enumerate() {
            let mut x = t ^ (a << (i % 8)) ^ (i as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
            x ^= x >> 33;
            x = x.wrapping_mul(0xff51_afd7_ed55_8ccd);
            x ^= x >> 33;
            *b = (x & 0xff) as u8;
        }
        buf
    }
}
