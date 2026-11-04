//! Sumami service — the analytics core.
//!
//! The crate is split into a library (this file) and a thin binary so the HTTP API
//! can be integration-tested without spawning a process.

#![forbid(unsafe_code)]

pub mod analyst;
pub mod api;
pub mod auth;
pub mod config;
pub mod db;
pub mod error;
pub mod instance;
pub mod models;
pub mod pool;
pub mod reports;
pub mod server;
pub mod sqlite;
pub mod storage;
pub mod static_files;
#[cfg(feature = "tls")]
pub mod tls;
pub mod ua;
pub mod writer;
