//! How much of the stored data is the UUID?
//!
//! The same rows, written four times: with everything, without the visitor id,
//! without both ids, and without the ids and the page. The difference between
//! the file sizes is what those columns cost on disk.

use duckdb::Connection;

fn main() {
    let dir = std::env::temp_dir().join("sumimi-uuid-share");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    let variants = [
        ("everything", "*"),
        ("no visitor_id", "* EXCLUDE (visitor_id)"),
        ("no ids", "* EXCLUDE (visitor_id, session_id)"),
        ("no ids, no page", "* EXCLUDE (visitor_id, session_id, url, url_query, title)"),
    ];

    let mut sizes = Vec::new();
    for (name, columns) in variants {
        let path = dir.join(format!("{name}.duckdb"));
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE MACRO uuid_of(s) AS CAST(
                substr(sha256(s), 1, 8) || '-' || substr(sha256(s), 9, 4) || '-' ||
                substr(sha256(s), 13, 4) || '-' || substr(sha256(s), 17, 4) || '-' ||
                substr(sha256(s), 21, 12) AS UUID);",
        )
        .unwrap();
        conn.execute_batch(
            "CREATE TABLE event (
                id BIGINT, website_id VARCHAR, session_id UUID, visitor_id UUID,
                created_at TIMESTAMP, url VARCHAR, url_query VARCHAR, title VARCHAR,
                referrer VARCHAR, referrer_path VARCHAR, referrer_query VARCHAR,
                hostname VARCHAR, channel VARCHAR,
                browser VARCHAR, browser_version VARCHAR, os VARCHAR, os_version VARCHAR,
                device VARCHAR, screen VARCHAR, language VARCHAR,
                country VARCHAR, region VARCHAR, city VARCHAR,
                utm_source VARCHAR, utm_medium VARCHAR, utm_campaign VARCHAR,
                utm_content VARCHAR, utm_term VARCHAR,
                event_type VARCHAR, event_name VARCHAR, tag VARCHAR, data VARCHAR);
            INSERT INTO event SELECT
                i, 'w1', uuid_of('session-' || (i / 3)::VARCHAR),
                uuid_of('visitor-' || (i % 50000)::VARCHAR),
                TIMESTAMP '2024-01-01' + to_seconds((i * 7919) % 31536000),
                '/blog/post-' || (i % 5000)::VARCHAR,
                'utm_source=twitter&utm_medium=cpc&utm_campaign=spring&utm_content=cta&utm_term=term',
                'A title about topic ' || (i % 500)::VARCHAR,
                'news.ycombinator.com', '/item', '?q=search', 'example.com', 'social',
                'Chrome', '120', 'macOS', '10', 'desktop', '1920x1080', 'en-US',
                'US', 'BE', 'Berlin', 'twitter', 'social', 'spring', 'cta', 'term',
                'pageview', NULL, NULL, NULL
            FROM generate_series(1, 200000) t(i);",
        )
        .unwrap();
        let sql = format!("CREATE TABLE kept AS SELECT {columns} FROM event; CHECKPOINT;");
        conn.execute_batch(&sql).unwrap();
        let size = std::fs::metadata(&path).unwrap().len();
        sizes.push((name.to_string(), size));
    }

    let rows = 200_000.0_f64;
    let base = sizes[0].1 as f64;
    println!("| columns | size | per view | share |");
    println!("| --- | ---: | ---: | ---: |");
    for (name, size) in &sizes {
        println!(
            "| {} | {:.1} MB | {:.1} B | {:.1}% |",
            name,
            *size as f64 / 1048576.0,
            *size as f64 / rows,
            *size as f64 / base * 100.0
        );
    }
    let visitor = (base - sizes[1].1 as f64) / rows;
    let session = (sizes[1].1 as f64 - sizes[2].1 as f64) / rows;
    let page = (sizes[2].1 as f64 - sizes[3].1 as f64) / rows;
    println!("\nvisitor_id: {visitor:.1} B per view");
    println!("session_id: {session:.1} B per view");
    println!("the page (url+query+title): {page:.1} B per view");
}
