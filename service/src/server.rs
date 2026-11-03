//! The server entry point: argument parsing and startup.
//!
//! Lives in the library so that the combined `sumami` binary can offer the same
//! `serve` / `admin` / `config` commands as the standalone `sumami-server`.

use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

use crate::api::{self, AppState};
use crate::auth;
use crate::config;
use crate::db;
use crate::config::Config;
use crate::pool::Db;
use crate::writer::Writer;

/// The Sumami analytics service.
#[derive(Debug, Parser)]
#[command(name = "sumami-server", version, about = "Sumami analytics service")]
pub struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Run the HTTP server.
    Serve(ServeArgs),
    /// Create or update the admin user, then exit.
    Admin(AdminArgs),
    /// Print the resolved configuration, then exit.
    Config,
}

#[derive(Debug, clap::Args)]
pub struct ServeArgs {
    /// Path of the DuckDB database file.
    #[arg(long, env = "SUMAMI_DB")]
    db: Option<std::path::PathBuf>,
    /// Address to bind to.
    #[arg(long, env = "SUMAMI_HOST")]
    host: Option<String>,
    /// Port to bind to.
    #[arg(long, env = "SUMAMI_PORT")]
    port: Option<u16>,
    /// Secret used to sign tokens (generated and stored on first run when omitted).
    #[arg(long, env = "SUMAMI_JWT_SECRET")]
    jwt_secret: Option<String>,
    /// Default retention window in days (0 = keep forever).
    #[arg(long, env = "SUMAMI_RETENTION_DAYS")]
    retention_days: Option<i32>,
    /// Header carrying the country code (e.g. `CF-IPCountry`).
    #[arg(long, env = "SUMAMI_COUNTRY_HEADER")]
    country_header: Option<String>,
    /// Directory holding the frontend files.
    #[arg(long, env = "SUMAMI_JS_DIR")]
    js_dir: Option<std::path::PathBuf>,
    /// Directory holding the documentation site.
    #[arg(long, env = "SUMAMI_DOCS_DIR")]
    docs_dir: Option<std::path::PathBuf>,
    /// Serve HTTPS with an automatically issued (and renewed) certificate.
    #[arg(long = "ssl", alias = "ssl-domain", env = "SUMAMI_SSL_DOMAIN")]
    ssl_domain: Option<String>,
    /// Contact address registered with the ACME provider (Let's Encrypt).
    #[arg(long, env = "SUMAMI_SSL_EMAIL")]
    ssl_email: Option<String>,
    /// Directory where the ACME account and certificates are cached.
    #[arg(long, env = "SUMAMI_SSL_CACHE_DIR")]
    ssl_cache_dir: Option<std::path::PathBuf>,
    /// Port used for HTTPS.
    #[arg(long, env = "SUMAMI_SSL_PORT", default_value_t = 443)]
    ssl_port: u16,
}

#[derive(Debug, clap::Args)]
pub struct AdminArgs {
    /// Admin username.
    #[arg(long, env = "SUMAMI_ADMIN_USER", default_value = "admin")]
    username: String,
    /// Admin password (generated and printed when omitted).
    #[arg(long, env = "SUMAMI_ADMIN_PASSWORD")]
    password: Option<String>,
    /// Path of the DuckDB database file.
    #[arg(long, env = "SUMAMI_DB")]
    db: Option<std::path::PathBuf>,
}

/// Parse the command line and run the server.
pub async fn run() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info,sumami=debug")),
        )
        .with_target(false)
        .init();

    let cli = Cli::parse();

    match cli.command {
        Command::Serve(args) => serve(args).await,
        Command::Admin(args) => admin(args),
        Command::Config => {
            let config = Config::from_env_and_flags(None, None, None, None, None, None, None)?;
            println!("{config:#?}");
            Ok(())
        }
    }
}

/// Run the server on the current Tokio runtime.
pub async fn serve_on_runtime(args: ServeArgs) -> Result<()> {
    serve(args).await
}

/// Build the configuration from the serve arguments.
fn config_from(args: &ServeArgs) -> Result<Config> {
    Config::from_env_and_flags(
        args.db.clone(),
        args.host.clone(),
        args.port,
        args.jwt_secret.clone(),
        args.retention_days,
        args.country_header.clone(),
        args.docs_dir.clone(),
    )
}

/// Create (or update) the admin user.
fn admin(args: AdminArgs) -> Result<()> {
    let config = Config::from_env_and_flags(
        args.db.clone(),
        None,
        None,
        None,
        None,
        None,
        None,
    )?;

    let db = Db::open_with(
        &config.db_path,
        &config.duckdb_memory_limit,
        &config.duckdb_flush_threshold,
        &config.duckdb_temp_limit,
    )?;
    let password = args
        .password
        .clone()
        .or_else(|| config.admin_password.clone())
        .unwrap_or_else(config::random_secret);
    let generated = args.password.is_none() && config.admin_password.is_none();

    db.with_write(|conn| upsert_admin(conn, &args.username, &password))?;

    println!("admin user '{}' ready", args.username);
    if generated {
        println!("password: {password}");
        println!("(store it now — it is not shown again)");
    }

    Ok(())
}

/// Insert or update the admin user.
fn upsert_admin(conn: &duckdb::Connection, username: &str, password: &str) -> Result<()> {
    let hash = auth::hash_password(password)?;
    let id = uuid::Uuid::new_v4().to_string();
    let now = db::now();

    let existing: i64 = {
        let mut stmt = conn.prepare("SELECT count(*) FROM \"user\" WHERE username = ?")?;
        let mut rows = stmt.query([username])?;
        match rows.next()? {
            Some(row) => row.get(0)?,
            None => 0,
        }
    };

    if existing > 0 {
        conn.execute(
            "UPDATE \"user\" SET password_hash = ? WHERE username = ?",
            duckdb::params![hash, username],
        )?;
    } else {
        conn.execute(
            "INSERT INTO \"user\" (id, username, password_hash, created_at, role)
             VALUES (?, ?, ?, ?, 'admin')",
            duckdb::params![id, username, hash, now],
        )?;
    }

    Ok(())
}

/// Run the HTTP server.
async fn serve(args: ServeArgs) -> Result<()> {
    let mut config = config_from(&args)?;
    if let Some(js_dir) = args.js_dir.clone() {
        config.js_dir = js_dir;
    }

    if let Some(domain) = args.ssl_domain.clone() {
        config.ssl_domain = Some(domain);
    }
    if let Some(email) = args.ssl_email.clone() {
        config.ssl_email = Some(email);
    }
    if let Some(dir) = args.ssl_cache_dir.clone() {
        config.ssl_cache_dir = dir;
    }
    config.ssl_port = args.ssl_port;

    let jwt_secret = config::resolve_jwt_secret(&config.jwt_secret, &config.db_path)?;
    let jwt_secret = Arc::new(jwt_secret);

    let db = Arc::new(Db::open_with(
        &config.db_path,
        &config.duckdb_memory_limit,
        &config.duckdb_flush_threshold,
        &config.duckdb_temp_limit,
    )?);
    config::restrict_permissions(&config.db_path);

    // Make sure there is always an admin account to log in with.
    let has_admin = db.with_write(admin_exists)?;
    if !has_admin {
        let password = config
            .admin_password
            .clone()
            .unwrap_or_else(config::random_secret);
        let generated = config.admin_password.is_none();
        db.with_write(|conn| upsert_admin(conn, &config.admin_user, &password))?;
        println!("[sumami] created the admin user '{}'", config.admin_user);
        if generated {
            println!("[sumami] password: {password}");
            println!("[sumami] store it now — it is not shown again");
        }
    }

    let (writer, _writer_handle) = Writer::spawn(
        db.clone(),
        std::time::Duration::from_secs(config.flush_interval_secs),
        config.max_queue,
    );

    let state = AppState::new(db, writer.clone(), Arc::new(config.clone()), jwt_secret);
    let app = api::router(state.clone());

    let addr: SocketAddr = format!("{}:{}", config.host, config.port)
        .parse()
        .context("invalid bind address")?;

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("binding {addr}"))?;

    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        db = %config.db_path.display(),
        "sumami is listening on http://{addr}"
    );
    println!("[sumami] listening on http://{addr} (db: {})", config.db_path.display());

    // Make sure the writer thread is told to stop when the server does.
    let writer_for_shutdown = state.writer.clone();

    // HTTPS with an automatically managed certificate, or plain HTTP.
    #[cfg(feature = "tls")]
    if let Some(domain) = config.ssl_domain.clone() {
        drop(listener);
        let ssl_addr: SocketAddr = format!("{}:{}", config.host, config.ssl_port)
            .parse()
            .context("invalid ssl bind address")?;
        let app = app;
        tokio::select! {
            result = crate::tls::serve(
                app,
                domain,
                config.ssl_email.clone(),
                config.ssl_cache_dir.clone(),
                ssl_addr,
            ) => result?,
            _ = shutdown_signal() => {}
        }
        writer_for_shutdown.shutdown();
        return Ok(());
    }

    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await?;

    writer_for_shutdown.shutdown();
    Ok(())
}

/// Is there at least one user?
fn admin_exists(conn: &duckdb::Connection) -> Result<bool> {
    let mut stmt = conn.prepare("SELECT count(*) FROM \"user\"")?;
    let mut rows = stmt.query([])?;
    match rows.next()? {
        Some(row) => Ok(row.get::<_, i64>(0)? > 0),
        None => Ok(false),
    }
}

/// Wait for Ctrl-C (or SIGTERM) and shut down gracefully.
async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };

    #[cfg(unix)]
    let terminate = async {
        if let Ok(mut signal) =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            signal.recv().await;
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }

    println!("[sumami] shutting down");
}
