//! Print the DuckDB settings that matter for a small VPS.
use duckdb::Connection;

fn main() -> anyhow::Result<()> {
    let path = std::env::args().nth(1).unwrap_or_else(|| "sumami.duckdb".to_string());
    let conn = Connection::open(&path)?;
    for setting in [
        "memory_limit",
        "threads",
        "temp_directory",
        "checkpoint_threshold",
        "allocator_flush_threshold",
        "allocator_background_threads",
    ] {
        let value: String = conn
            .query_row(&format!("SELECT CAST(current_setting('{setting}') AS VARCHAR)"), [], |r| r.get(0))
            .unwrap_or_else(|e| format!("<{e}>"));
        println!("{setting:22} {value}");
    }
    Ok(())
}
