//! The Sumami client — read and analyse a Sumami database.
//!
//! The same commands work against a local DuckDB file (`--db`) or against a running
//! server over HTTP (`--url`, with the admin account).

#![forbid(unsafe_code)]

pub mod backup;
mod output;
mod seed;
mod source;

use std::path::PathBuf;

use anyhow::{anyhow, Result};
use clap::{Parser, Subcommand};
use serde_json::Value;

use source::{parse_time, Range, Report, Source};

/// The Sumami command line interface.
#[derive(Debug, Parser)]
#[command(name = "sumami", version, about = "Sumami CLI — read and analyse Sumami data")]
struct Cli {
    /// Path of the DuckDB database file (local mode).
    #[arg(long, global = true, env = "SUMAMI_DB")]
    db: Option<PathBuf>,
    /// Base URL of a running Sumami server (remote mode).
    #[arg(long, global = true, env = "SUMAMI_URL")]
    url: Option<String>,
    /// Bearer token for remote mode.
    #[arg(long, global = true, env = "SUMAMI_TOKEN")]
    token: Option<String>,
    /// Admin username (remote mode).
    #[arg(long, global = true, env = "SUMAMI_ADMIN_USER")]
    username: Option<String>,
    /// Admin password (remote mode).
    #[arg(long, global = true, env = "SUMAMI_ADMIN_PASSWORD")]
    password: Option<String>,
    /// Print raw JSON instead of a table.
    #[arg(long, global = true)]
    json: bool,
    /// Analyse a Parquet archive instead of the database (e.g. './archive/*').
    #[arg(long, global = true, env = "SUMAMI_PARQUET")]
    parquet: Option<String>,
    #[command(subcommand)]
    command: Command,
}

/// The commands.
#[derive(Debug, Clone, Subcommand)]
enum Command {
    /// List the websites.
    Websites,
    /// Show the summary metrics.
    Summary {
        /// The website id.
        website: String,
        /// Start of the window (7d, 24h, 2026-01-01, ...).
        #[arg(long)]
        from: Option<String>,
        /// End of the window.
        #[arg(long)]
        to: Option<String>,
    },
    /// Show the top pages.
    Pages(WindowArgs),
    /// Show the traffic sources.
    Referrers(WindowArgs),
    /// Show the browser breakdown.
    Browsers(WindowArgs),
    /// Show the operating system breakdown.
    Os(WindowArgs),
    /// Show the device breakdown.
    Devices(WindowArgs),
    /// Show the country breakdown.
    Countries(WindowArgs),
    /// Show the language breakdown.
    Languages(WindowArgs),
    /// Show the screen size breakdown.
    Screens(WindowArgs),
    /// Show the custom events.
    Events(WindowArgs),
    /// Show the sessions.
    Sessions(WindowArgs),
    /// Show the time series.
    Timeseries(WindowArgs),
    /// Export a report as JSON or CSV.
    Export {
        /// The website id.
        website: String,
        /// The report to export.
        #[arg(value_enum)]
        report: ExportReport,
        /// The output format.
        #[arg(long, value_enum, default_value_t = ExportFormat::Json)]
        format: ExportFormat,
        /// Start of the window.
        #[arg(long)]
        from: Option<String>,
        /// End of the window.
        #[arg(long)]
        to: Option<String>,
    },
    /// Write the database to a Parquet archive (one file per day).
    Backup {
        /// The directory to write the archive to.
        #[arg(long, default_value = "archive")]
        out: PathBuf,
        /// Only archive this website.
        #[arg(long)]
        website: Option<String>,
        /// Start of the range (YYYY-MM-DD).
        #[arg(long)]
        from: Option<String>,
        /// End of the range (YYYY-MM-DD).
        #[arg(long)]
        to: Option<String>,
        /// Keep files that already exist.
        #[arg(long)]
        skip_existing: bool,
    },
    /// Delete data that is older than the retention window.
    Prune {
        /// The website id (default: every website).
        website: Option<String>,
    },
    /// Delete a website and all of its data.
    Erase {
        /// The website id.
        website: String,
        /// Do not ask for confirmation.
        #[arg(long)]
        yes: bool,
    },
    /// Fill the database with synthetic demo data.
    Seed {
        /// The name of the website to seed (created when missing).
        #[arg(long, default_value = "Example Blog")]
        website: String,
        /// The domain of the website.
        #[arg(long, default_value = "example.com")]
        domain: String,
        /// How many days of history to generate.
        #[arg(long, default_value_t = 30)]
        days: i64,
        /// Roughly how many visitors per day.
        #[arg(long, default_value_t = 120)]
        visitors: i64,
    },
    /// Store the credentials for a server.
    Login {
        /// The base URL of the server.
        url: String,
        /// The admin username.
        #[arg(long, default_value = "admin")]
        username: String,
        /// The admin password (asked for interactively when omitted).
        #[arg(long)]
        password: Option<String>,
    },
}

/// Arguments shared by the windowed reports.
#[derive(Debug, Clone, clap::Args)]
struct WindowArgs {
    /// The website id.
    website: String,
    /// Start of the window (7d, 24h, 2026-01-01, ...).
    #[arg(long)]
    from: Option<String>,
    /// End of the window.
    #[arg(long)]
    to: Option<String>,
    /// How many rows to show.
    #[arg(long, default_value_t = 50)]
    limit: i64,
}

/// The reports that can be exported.
#[derive(Debug, Clone, Copy, clap::ValueEnum)]
enum ExportReport {
    /// The summary metrics.
    Summary,
    /// The top pages.
    Pages,
    /// The traffic sources.
    Referrers,
    /// The browser breakdown.
    Browsers,
    /// The custom events.
    Events,
    /// The sessions.
    Sessions,
    /// The time series.
    Timeseries,
}

impl From<ExportReport> for Report {
    fn from(report: ExportReport) -> Self {
        match report {
            ExportReport::Summary => Report::Summary,
            ExportReport::Pages => Report::Pages,
            ExportReport::Referrers => Report::Referrers,
            ExportReport::Browsers => Report::Browsers,
            ExportReport::Events => Report::Events,
            ExportReport::Sessions => Report::Sessions,
            ExportReport::Timeseries => Report::Timeseries,
        }
    }
}

/// The export formats.
#[derive(Debug, Clone, Copy, clap::ValueEnum)]
enum ExportFormat {
    /// JSON.
    Json,
    /// CSV.
    Csv,
}

/// Parse the command line and run the client.
pub fn run() -> Result<()> {
    let cli = Cli::parse();

    let command = cli.command.clone();
    match command {
        Command::Login {
            url,
            username,
            password,
        } => {
            let password = match password {
                Some(password) => password,
                None => {
                    print!("password for {username} at {url}: ");
                    use std::io::Write;
                    std::io::stdout().flush()?;
                    let mut line = String::new();
                    std::io::stdin().read_line(&mut line)?;
                    line.trim().to_string()
                }
            };

            // Verify the credentials before storing them.
            let source = Source::remote(&url, None, Some(&username), Some(&password))?;
            if let Source::Remote { base, .. } = &source {
                let path = source::store_credentials(&username, &password)?;
                println!("credentials saved to {}", path.display());
                println!("logged in to {base}");
            }
            Ok(())
        }
        Command::Seed {
            website,
            domain,
            days,
            visitors,
        } => {
            let path = cli
                .db
                .clone()
                .unwrap_or_else(|| PathBuf::from("sumami.duckdb"));

            let options = seed::SeedOptions {
                website,
                domain,
                days,
                visitors_per_day: visitors,
            };

            let db = sumami_service::pool::Db::open(&path, "256MB")
                .map_err(|err| anyhow!("opening {}: {err}", path.display()))?;

            let (website_id, sessions, events) = db
                .with_write(|conn| seed::seed(conn, &options))
                .map_err(|err| anyhow!("{err}"))?;

            println!("seeded {sessions} sessions and {events} events");
            println!("website: {website_id}");
            println!("start the server and open the dashboard to see it");
            Ok(())
        }
        Command::Backup {
            out,
            website,
            from,
            to,
            skip_existing,
        } => {
            let path = cli
                .db
                .clone()
                .unwrap_or_else(|| PathBuf::from("sumami.duckdb"));

            // The backup reads the database, so the server must be stopped (DuckDB
            // allows a single writer per file). `--url` is not supported here for the
            // same reason.
            let db = sumami_service::pool::Db::open(&path, "256MB")
                .map_err(|err| anyhow!("opening {}: {err}", path.display()))?;

            let from = from.as_deref().map(parse_date).transpose()?;
            let to = to.as_deref().map(parse_date).transpose()?;

            let stats = db
                .with_write(|conn| {
                    backup::export(
                        conn,
                        &out,
                        website.as_deref(),
                        from,
                        to,
                        skip_existing,
                    )
                })
                .map_err(|err| anyhow!("{err}"))?;

            println!(
                "archived {} rows into {} file(s) under {}",
                stats.rows,
                stats.files,
                stats.directory.display()
            );
            println!("read it back with:  sumami --parquet '{}/' summary <website-id>", stats.directory.display());
            Ok(())
        }
        Command::Prune { website } => {
            let source = open_source(&cli, true)?;
            let (sessions, events) = source.prune(website.as_deref())?;
            println!("removed {sessions} sessions and {events} events");
            Ok(())
        }
        Command::Erase { website, yes } => {
            if !yes {
                println!("this deletes the website {website} and all of its data.");
                print!("type the website id to confirm: ");
                use std::io::Write;
                std::io::stdout().flush()?;
                let mut answer = String::new();
                std::io::stdin().read_line(&mut answer)?;
                if answer.trim() != website {
                    return Err(anyhow!("aborted"));
                }
            }
            let source = open_source(&cli, true)?;
            let removed = source.erase(&website)?;
            println!("removed {removed} rows");
            Ok(())
        }
        _ => {
            let source = open_source(&cli, false)?;
            match &source {
                Source::Local { path, .. } => {
                    eprintln!("[sumami] source: {}", path.display());
                }
                Source::Remote { base, .. } => {
                    eprintln!("[sumami] source: {base}");
                }
            }
            let (report, website, range, limit) = match &cli.command {
                Command::Websites => (Report::Websites, None, Range::default_window(), 50),
                Command::Summary {
                    website,
                    from,
                    to,
                } => (
                    Report::Summary,
                    Some(website.clone()),
                    window(from.as_deref(), to.as_deref())?,
                    50,
                ),
                Command::Pages(args) => (
                    Report::Pages,
                    Some(args.website.clone()),
                    window(args.from.as_deref(), args.to.as_deref())?,
                    args.limit,
                ),
                Command::Referrers(args) => (
                    Report::Referrers,
                    Some(args.website.clone()),
                    window(args.from.as_deref(), args.to.as_deref())?,
                    args.limit,
                ),
                Command::Browsers(args) => (
                    Report::Browsers,
                    Some(args.website.clone()),
                    window(args.from.as_deref(), args.to.as_deref())?,
                    args.limit,
                ),
                Command::Os(args) => (
                    Report::Os,
                    Some(args.website.clone()),
                    window(args.from.as_deref(), args.to.as_deref())?,
                    args.limit,
                ),
                Command::Devices(args) => (
                    Report::Devices,
                    Some(args.website.clone()),
                    window(args.from.as_deref(), args.to.as_deref())?,
                    args.limit,
                ),
                Command::Countries(args) => (
                    Report::Countries,
                    Some(args.website.clone()),
                    window(args.from.as_deref(), args.to.as_deref())?,
                    args.limit,
                ),
                Command::Languages(args) => (
                    Report::Languages,
                    Some(args.website.clone()),
                    window(args.from.as_deref(), args.to.as_deref())?,
                    args.limit,
                ),
                Command::Screens(args) => (
                    Report::Screens,
                    Some(args.website.clone()),
                    window(args.from.as_deref(), args.to.as_deref())?,
                    args.limit,
                ),
                Command::Events(args) => (
                    Report::Events,
                    Some(args.website.clone()),
                    window(args.from.as_deref(), args.to.as_deref())?,
                    args.limit,
                ),
                Command::Sessions(args) => (
                    Report::Sessions,
                    Some(args.website.clone()),
                    window(args.from.as_deref(), args.to.as_deref())?,
                    args.limit,
                ),
                Command::Timeseries(args) => (
                    Report::Timeseries,
                    Some(args.website.clone()),
                    window(args.from.as_deref(), args.to.as_deref())?,
                    args.limit,
                ),
                Command::Export {
                    website,
                    report,
                    format,
                    from,
                    to,
                } => {
                    let report: Report = (*report).into();
                    let value = source.report(
                        report,
                        Some(website),
                        window(from.as_deref(), to.as_deref())?,
                        10_000,
                    )?;
                    match format {
                        ExportFormat::Json => println!("{}", serde_json::to_string_pretty(&value)?),
                        ExportFormat::Csv => print_csv(&value),
                    }
                    return Ok(());
                }
                _ => unreachable!(),
            };

            let value = source.report(report, website.as_deref(), range, limit)?;

            if cli.json {
                println!("{}", serde_json::to_string_pretty(&value)?);
            } else {
                print_report(report, &value);
            }
            Ok(())
        }
    }
}

/// Open the configured source.
fn open_source(cli: &Cli, writable: bool) -> Result<Source> {
    // The cold tier: a Parquet archive, queried in place.
    if let Some(pattern) = &cli.parquet {
        return Source::parquet(pattern);
    }

    if let Some(url) = &cli.url {
        return Source::remote(
            url,
            cli.token.as_deref(),
            cli.username.as_deref(),
            cli.password.as_deref(),
        );
    }

    let path = cli
        .db
        .clone()
        .unwrap_or_else(|| PathBuf::from("sumami.duckdb"));

    if writable {
        Source::local_writable(&path)
    } else {
        Source::local(&path)
    }
}

/// Parse a `YYYY-MM-DD` date.
fn parse_date(value: &str) -> Result<chrono::NaiveDate> {
    chrono::NaiveDate::parse_from_str(value.trim(), "%Y-%m-%d")
        .map_err(|err| anyhow!("invalid date '{value}': {err}"))
}

/// Build the time window.
fn window(from: Option<&str>, to: Option<&str>) -> Result<Range> {
    let default = Range::default_window();
    let start = match from {
        Some(value) => parse_time(value)?,
        None => default.start,
    };
    let end = match to {
        Some(value) => parse_time(value)?,
        None => default.end,
    };
    Ok(Range { start, end })
}

/// Print a report as a table.
fn print_report(report: Report, value: &Value) {
    match report {
        Report::Websites => {
            let rows: Vec<Vec<String>> = value
                .as_array()
                .map(|items| {
                    items
                        .iter()
                        .map(|item| {
                            vec![
                                output::cell(item.get("id").unwrap_or(&Value::Null)),
                                output::cell(item.get("name").unwrap_or(&Value::Null)),
                                output::cell(item.get("domain").unwrap_or(&Value::Null)),
                                output::cell(item.get("createdAt").unwrap_or(&Value::Null)),
                            ]
                        })
                        .collect()
                })
                .unwrap_or_default();
            output::table(&["ID", "NAME", "DOMAIN", "CREATED"], &rows);
        }
        Report::Summary => {
            let mut rows = Vec::new();
            for (key, label) in [
                ("pageviews", "Pageviews"),
                ("visitors", "Visitors"),
                ("views", "Views"),
                ("bounces", "Bounces"),
                ("visitDuration", "Visit duration (s)"),
            ] {
                let metric = value.get(key);
                let count = metric
                    .and_then(|m| m.get("value"))
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0);
                rows.push(vec![label.to_string(), output::number(count)]);
            }
            output::table(&["METRIC", "VALUE"], &rows);
        }
        Report::Timeseries => {
            let rows: Vec<Vec<String>> = value
                .as_array()
                .map(|items| {
                    items
                        .iter()
                        .map(|item| {
                            vec![
                                output::cell(item.get("date").or_else(|| item.get("name")).unwrap_or(&Value::Null)),
                                output::number(
                                    item.get("value")
                                        .and_then(|v| v.as_i64())
                                        .unwrap_or(0),
                                ),
                            ]
                        })
                        .collect()
                })
                .unwrap_or_default();
            output::table(&["DATE", "VIEWS"], &rows);
        }
        Report::Sessions => {
            let rows: Vec<Vec<String>> = value
                .as_array()
                .map(|items| {
                    items
                        .iter()
                        .map(|item| {
                            vec![
                                output::cell(item.get("createdAt").unwrap_or(&Value::Null)),
                                output::cell(item.get("url").unwrap_or(&Value::Null)),
                                output::cell(item.get("referrer").unwrap_or(&Value::Null)),
                                output::cell(item.get("browser").unwrap_or(&Value::Null)),
                                output::cell(item.get("country").unwrap_or(&Value::Null)),
                                output::number(
                                    item.get("views").and_then(|v| v.as_i64()).unwrap_or(0),
                                ),
                            ]
                        })
                        .collect()
                })
                .unwrap_or_default();
            output::table(&["WHEN", "URL", "REFERRER", "BROWSER", "COUNTRY", "VIEWS"], &rows);
        }
        _ => {
            let rows: Vec<Vec<String>> = value
                .as_array()
                .map(|items| {
                    items
                        .iter()
                        .map(|item| {
                            vec![
                                output::cell(item.get("name").unwrap_or(&Value::Null)),
                                output::number(
                                    item.get("value").and_then(|v| v.as_i64()).unwrap_or(0),
                                ),
                            ]
                        })
                        .collect()
                })
                .unwrap_or_default();
            output::table(&["NAME", "VALUE"], &rows);
        }
    }
}

/// Print a report as CSV.
fn print_csv(value: &Value) {
    let items = match value.as_array() {
        Some(items) => items,
        None => {
            println!("{}", serde_json::to_string(value).unwrap_or_default());
            return;
        }
    };

    if let Some(first) = items.first() {
        if let Some(object) = first.as_object() {
            let headers: Vec<String> = object.keys().cloned().collect();
            println!("{}", headers.join(","));
            for item in items {
                let row: Vec<String> = headers
                    .iter()
                    .map(|header| csv_cell(item.get(header).unwrap_or(&Value::Null)))
                    .collect();
                println!("{}", row.join(","));
            }
            return;
        }
    }

    for item in items {
        println!("{}", csv_cell(item));
    }
}

/// Quote a CSV cell when needed.
fn csv_cell(value: &Value) -> String {
    let text = match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    };
    if text.contains(',') || text.contains('"') || text.contains('\n') {
        format!("\"{}\"", text.replace('"', "\"\""))
    } else {
        text
    }
}
