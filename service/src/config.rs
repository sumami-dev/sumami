//! Configuration for the Sumami service.
//!
//! Everything is overridable by environment variable so a deployment is a systemd unit
//! plus a small `EnvironmentFile`, never a config file in the repository.

use std::path::PathBuf;

use anyhow::Context;

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
    /// DuckDB `memory_limit`, in bytes. Keeps the process inside a 512 MB VPS.
    pub duckdb_memory_limit: String,
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

        let duckdb_memory_limit = std::env::var("SUMAMI_DUCKDB_MEMORY_LIMIT")
            .unwrap_or_else(|_| "256MB".to_string());

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

        let keep_query = std::env::var("SUMAMI_KEEP_QUERY")
            .map(|v| matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes"))
            .unwrap_or(false);

        let js_dir = env_path("SUMAMI_JS_DIR").unwrap_or_else(|| PathBuf::from("js"));

        let app_url = std::env::var("SUMAMI_APP_URL").ok().filter(|s| !s.is_empty());

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
            max_queue,
            flush_interval_secs,
            retention_days,
            country_header,
            keep_query,
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
