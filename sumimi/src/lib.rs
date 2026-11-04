//! # Sumimi — the open-source build, as a library
//!
//! This crate is the **whole** open-source product behind one name:
//!
//! * [`service`] — the analytics core: the collection and stats API, the DuckDB
//!   and SQLite data engines, the shared report layer, the tracker and the
//!   dashboard shell.
//! * [`cli`] — the client: the same reports, run offline against a database
//!   file or a Parquet archive.
//!
//! ## Why a crate
//!
//! The private [`sumimi-pro`](https://github.com/sumimi-dev/sumimi-pro) edition is
//! the open-source build **plus** the PostgreSQL backend, the S3 cold tier and the
//! archival job. It used to carry a copy of the open-source tree, which meant every
//! change to the open-source code had to be merged by hand — and, more than once,
//! was not.
//!
//! Now the private edition depends on **this** crate. There is one implementation
//! of the schema, the metrics and the reports, and the private edition adds to it
//! instead of copying it.
//!
//! ```no_run
//! use std::sync::Arc;
//! use sumimi::service::config::Config;
//! use sumimi::service::instance::Instance;
//! use sumimi::service::storage::Storage;
//!
//! # async fn example(
//! #     config: Config,
//! #     storage: Arc<dyn Storage>,
//! #     instance: Arc<dyn Instance>,
//! # ) -> anyhow::Result<()> {
//! sumimi::service::server::serve_with(Arc::new(config), storage, instance).await
//! # }
//! ```

/// The analytics core: the API, the data engines and the shared report layer.
pub use sumimi_service as service;

/// The client: the same reports, offline.
pub use sumimi_cli as cli;

pub use sumimi_service::config::Config;
pub use sumimi_service::models::{
    BreakdownRow, EventRow, Hit, MetricsSummary, Revenue, SessionRow, Website,
};
pub use sumimi_service::pool::Db;
pub use sumimi_service::sqlite::SqliteBackend;
pub use sumimi_service::storage::{
    Dimension, ReportQuery, Storage, StorageStats, SummaryMetrics, TimeRange, WebsitePatch,
};

/// The version of the open-source build.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
