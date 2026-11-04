//! The schema itself: the migration between versions, and the promise that the
//! append-only fact tables carry no index.

use sumami_service::db::SCHEMA_VERSION;
use sumami_service::pool::Db;
use sumami_service::storage::Storage;

/// A unique temporary database path.
fn temp_db(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("sumami-schema-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir.join("sumami.duckdb")
}

/// How many primary keys does this table have?
fn primary_keys(conn: &duckdb::Connection, table: &str) -> i64 {
    conn.query_row(
        "SELECT count(*) FROM duckdb_constraints()
         WHERE table_name = ? AND constraint_type = 'PRIMARY KEY'",
        [table],
        |row| row.get(0),
    )
    .unwrap()
}

/// A fresh database has no index on the fact tables.
///
/// A `PRIMARY KEY` on an append-only table buys nothing — every query is a filtered
/// scan, and DuckDB prunes those with zone maps — but it costs 8 bytes per row on
/// disk (the persisted ART index) and a uniqueness check on every append.
#[test]
fn the_fact_tables_have_no_primary_key() {
    let path = temp_db("no-pk");
    let db = Db::open(&path, "256MB").unwrap();

    db.with_read(|conn| {
        assert_eq!(
            primary_keys(conn, "event"),
            0,
            "the event table must not carry a primary key"
        );
        assert_eq!(
            primary_keys(conn, "revenue"),
            0,
            "the revenue table must not carry a primary key"
        );
        // The small dimension tables keep their keys.
        assert_eq!(primary_keys(conn, "website"), 1);
        assert_eq!(primary_keys(conn, "meta"), 1);
        // The accounts are **not** in the analytics file: they live in the instance
        // store, in their own small file.
        assert_eq!(
            primary_keys(conn, "user"),
            0,
            "the accounts must not be in the analytics file"
        );
        Ok::<_, anyhow::Error>(())
    })
    .unwrap();

    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

/// The ids stay unique without a constraint: the writer hands them out.
#[test]
fn the_ids_are_unique_without_a_constraint() {
    let path = temp_db("ids");
    let db = Db::open(&path, "256MB").unwrap();

    for i in 0..500 {
        db.with_write(|conn| {
            conn.execute(
                "INSERT INTO event (id, website_id, session_id, visitor_id, created_at, url)
                 VALUES (?1, 'w', 's', 'v', now(), '/x')",
                [i as i64],
            )?;
            Ok::<_, anyhow::Error>(())
        })
        .unwrap();
    }

    let (rows, distinct) = db
        .with_read(|conn| {
            let rows: i64 = conn.query_row("SELECT count(*) FROM event", [], |row| row.get(0))?;
            let distinct: i64 =
                conn.query_row("SELECT count(DISTINCT id) FROM event", [], |row| row.get(0))?;
            Ok((rows, distinct))
        })
        .unwrap();

    assert_eq!(rows, 500);
    assert_eq!(distinct, 500, "the ids must be unique");

    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

/// An older database is migrated in place, and nothing is lost.
#[test]
fn a_v2_database_is_migrated_without_losing_rows() {
    let path = temp_db("migrate");

    // A database written by schema v2: the fact tables had a primary key.
    {
        let conn = duckdb::Connection::open(&path).unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE event (
                id              BIGINT PRIMARY KEY,
                website_id      VARCHAR NOT NULL,
                session_id      VARCHAR NOT NULL,
                visitor_id      VARCHAR NOT NULL,
                created_at      TIMESTAMP NOT NULL,
                url             VARCHAR,
                url_query       VARCHAR,
                title           VARCHAR,
                referrer        VARCHAR,
                referrer_path   VARCHAR,
                referrer_query  VARCHAR,
                hostname        VARCHAR,
                browser         VARCHAR,
                browser_version VARCHAR,
                os              VARCHAR,
                os_version      VARCHAR,
                device          VARCHAR,
                screen          VARCHAR,
                language        VARCHAR,
                country         VARCHAR,
                region          VARCHAR,
                city            VARCHAR,
                utm_source      VARCHAR,
                utm_medium      VARCHAR,
                utm_campaign    VARCHAR,
                utm_content     VARCHAR,
                utm_term        VARCHAR,
                gclid           VARCHAR,
                fbclid          VARCHAR,
                msclkid         VARCHAR,
                ttclid          VARCHAR,
                lifatid         VARCHAR,
                twclid          VARCHAR,
                lcp             DOUBLE,
                inp             DOUBLE,
                cls             DOUBLE,
                fcp             DOUBLE,
                ttfb            DOUBLE,
                event_type      VARCHAR,
                event_name      VARCHAR,
                tag             VARCHAR,
                data            VARCHAR
            );
            CREATE TABLE meta (key VARCHAR PRIMARY KEY, value VARCHAR);
            INSERT INTO meta VALUES ('schema_version', '2');
            INSERT INTO event (id, website_id, session_id, visitor_id, created_at, url, title)
            VALUES (1, 'w1', 's1', 'v1', now(), '/one', 'One'),
                   (2, 'w1', 's2', 'v2', now(), '/two', 'Two'),
                   (3, 'w2', 's3', 'v3', now(), '/three', 'Three');
            "#,
        )
        .unwrap();
    }

    // Opening the database rebuilds it into a new file and swaps it in.
    let before = std::fs::metadata(&path).unwrap().len();
    let db = Db::open(&path, "256MB").unwrap();
    db.with_read(|conn| {
        conn.execute_batch("CHECKPOINT")?;
        Ok::<_, anyhow::Error>(())
    })
    .unwrap();
    let after = std::fs::metadata(&path).unwrap().len();
    println!(
        "migrated: {before} bytes -> {after} bytes ({:+.1}%)",
        (after as f64 - before as f64) / before as f64 * 100.0
    );
    assert!(
        after <= before,
        "the rebuild must not leave the file bigger ({before} -> {after})"
    );

    // The temporary file is gone.
    let mut leftovers: Vec<String> = std::fs::read_dir(path.parent().unwrap())
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|name| name.contains("rebuilt"))
        .collect();
    leftovers.sort();
    assert!(
        leftovers.is_empty(),
        "the rebuilt file must be swapped in, not left behind: {leftovers:?}"
    );

    db.with_read(|conn| {
        let version: String = conn
            .query_row(
                "SELECT value FROM meta WHERE key = 'schema_version'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(version, SCHEMA_VERSION.to_string(), "the version must be current");

        let rows: i64 = conn.query_row("SELECT count(*) FROM event", [], |row| row.get(0))?;
        assert_eq!(rows, 3, "no row may be lost in the migration");

        let title: String = conn
            .query_row(
                "SELECT title FROM event WHERE id = 2",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(title, "Two", "the row must keep its fields");

        assert_eq!(primary_keys(conn, "event"), 0, "the index must be gone");
        Ok::<_, anyhow::Error>(())
    })
    .unwrap();

    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

/// Deleting a website deletes its events — the session table is gone, so nothing
/// else may be deleted from it.
#[test]
fn deleting_a_website_removes_its_events() {
    let path = temp_db("delete");
    let db = Db::open(&path, "256MB").unwrap();

    db.with_write(|conn| {
        conn.execute(
            "INSERT INTO website (id, name, domain, created_at, updated_at, retention_days, country_header)
             VALUES ('w1', 'One', 'one.example', now(), now(), 0, NULL),
                    ('w2', 'Two', 'two.example', now(), now(), 0, NULL)",
            [],
        )?;
        for (id, website) in [(1i64, "w1"), (2, "w1"), (3, "w2")] {
            conn.execute(
                "INSERT INTO event (id, website_id, session_id, visitor_id, created_at, url)
                 VALUES (?1, ?2, 's', 'v', now(), '/x')",
                duckdb::params![id, website],
            )?;
        }
        Ok::<_, anyhow::Error>(())
    })
    .unwrap();

    let removed = db.delete_website("w1").unwrap();
    assert!(removed, "the website must be deleted");

    let (events, websites) = db
        .with_read(|conn| {
            let events: i64 = conn.query_row("SELECT count(*) FROM event", [], |row| row.get(0))?;
            let websites: i64 =
                conn.query_row("SELECT count(*) FROM website", [], |row| row.get(0))?;
            Ok((events, websites))
        })
        .unwrap();

    assert_eq!(events, 1, "only the other website's event may remain");
    assert_eq!(websites, 1);

    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}
