//! The `sumami-client` executable — the standalone analysis CLI.

fn main() -> anyhow::Result<()> {
    sumami_cli::run()
}
