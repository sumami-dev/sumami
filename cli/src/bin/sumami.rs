//! The `sumami` executable — the two-in-one.
//!
//! `sumami serve` runs the server, everything else is the analysis client:
//!
//! ```text
//! sumami serve --db ./sumami.duckdb --port 3000
//! sumami summary <website-id> --from 7d
//! ```

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("serve") | Some("server") | Some("admin") | Some("config") => {
            tokio::runtime::Runtime::new()?.block_on(sumami_service::server::run())
        }
        _ => sumami_cli::run(),
    }
}
