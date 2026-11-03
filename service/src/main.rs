//! The `sumami-server` executable.

#![forbid(unsafe_code)]

fn main() -> anyhow::Result<()> {
    tokio::runtime::Runtime::new()?.block_on(sumami_service::server::run())
}
