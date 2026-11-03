//! The `sumami` executable — the server and the client in one binary.
//!
//! ```text
//! sumami serve --db ./sumami.duckdb --port 3000   # the server
//! sumami summary <website-id> --from 7d           # the client
//! ```
//!
//! The program decides what to do from its arguments: `serve`, `server`, `admin`
//! and `config` start the server, everything else is a client report.
//!
//! Installing the binary under another name (`sumami-server`, `sumami-client`)
//! also works: the name decides the default mode.

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let program = args
        .first()
        .map(String::as_str)
        .unwrap_or("sumami")
        .rsplit('/')
        .next()
        .unwrap_or("sumami");

    // The name decides the default mode.
    let by_name = match program {
        "sumami-server" => Some(true),
        "sumami-client" => Some(false),
        _ => None,
    };

    let first = args.get(1).map(String::as_str).unwrap_or("");
    let wants_server = match first {
        "serve" | "server" | "admin" | "config" => true,
        _ => by_name.unwrap_or(false),
    };

    if wants_server && !matches!(first, "summary" | "websites" | "pages" | "referrers" | "browsers" | "os" | "devices" | "countries" | "languages" | "screens" | "events" | "sessions" | "timeseries" | "export" | "prune" | "erase" | "login" | "seed" | "help" | "--help" | "-h") {
        tokio::runtime::Runtime::new()?.block_on(sumami_service::server::run())
    } else {
        sumami_cli::run()
    }
}
