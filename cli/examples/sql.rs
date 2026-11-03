//! Run a SQL file against a DuckDB database — a small development helper.
//!
//! ```bash
//! cargo run --release -p sumami-cli --example sql -- <db> <file.sql>
//! ```

use duckdb::Connection;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let db = args.get(1).cloned().unwrap_or_else(|| ":memory:".to_string());
    let sql = args
        .get(2)
        .map(std::fs::read_to_string)
        .transpose()?
        .unwrap_or_else(|| "SELECT 1".to_string());

    let conn = Connection::open(&db)?;
    let mut stmt = conn.prepare(&sql)?;
    let column_count = stmt.column_count();
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let mut values = Vec::new();
        for index in 0..column_count {
            values.push(format!("{:?}", row.get::<_, duckdb::types::Value>(index)));
        }
        println!("{}", values.join(" | "));
    }
    Ok(())
}
