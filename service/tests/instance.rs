//! The instance store: the accounts, the log, the imports, the cache, the snapshots
//! and the chat.
//!
//! These are the things an instance keeps that are **not** analytics: small, mutable
//! and transactional. They live in their own file, so they survive an analytics
//! rebuild.

use sumami_service::instance::{
    Account, ChatMessage, ChatSession, GscRow, ImportReport, LogEntry, Snapshot, SqliteInstance,
};

/// A store in memory.
fn store() -> SqliteInstance {
    SqliteInstance::open_in_memory().unwrap()
}

/// The accounts are created once and their password replaced afterwards.
#[test]
fn the_account_is_created_and_its_password_replaced() {
    let store = store();

    assert!(store.account("admin").unwrap().is_none());

    let account = store.set_password("admin", "hash-1").unwrap();
    assert_eq!(account.username, "admin");
    assert_eq!(account.role, "admin");
    assert_eq!(account.password_hash, "hash-1");

    let account = store.set_password("admin", "hash-2").unwrap();
    assert_eq!(account.password_hash, "hash-2");
    assert_eq!(account.id, account.id, "the account keeps its id");

    // One account, not two.
    assert_eq!(store.accounts().unwrap().len(), 1);
    assert_eq!(store.account("admin").unwrap().unwrap().password_hash, "hash-2");
}

/// The usernames are unique — the database says so, not the application.
#[test]
fn the_usernames_are_unique() {
    let store = store();
    store.set_password("admin", "hash").unwrap();

    // A different account with the same name is refused by the unique index.
    let result = store.with_connection(|conn| {
        conn.execute(
            "INSERT INTO \"user\" (id, username, password_hash, created_at, role)
             VALUES ('x', 'admin', 'hash', 0, 'admin')",
            [],
        )?;
        Ok::<_, anyhow::Error>(())
    });
    assert!(result.is_err(), "the username must be unique");
}

/// Every sign-in attempt is recorded, successful or not.
#[test]
fn the_logins_are_recorded() {
    let store = store();

    store.record_login("admin", true, Some("10.0.0.1"), Some("curl")).unwrap();
    store.record_login("admin", false, Some("10.0.0.2"), Some("curl")).unwrap();
    store.record_login("nobody", false, None, None).unwrap();

    let logins = store.logins(10).unwrap();
    assert_eq!(logins.len(), 3);
    // Newest first. The three attempts happened in the same second, so the row id
    // breaks the tie: the last one written comes first.
    assert_eq!(logins[0].username, "nobody");
    assert!(!logins[0].ok);
    assert_eq!(logins[1].username, "admin");
    assert!(!logins[1].ok);
    assert!(logins[2].ok);
    assert_eq!(logins[2].ip.as_deref(), Some("10.0.0.1"));
    assert_eq!(logins[2].agent.as_deref(), Some("curl"));
}

/// The log keeps what happened, and can be filtered by kind.
#[test]
fn the_log_records_what_happened() {
    let store = store();
    let now = chrono::Utc::now().naive_utc();

    store
        .log(&LogEntry {
            at: now,
            level: "info".to_string(),
            kind: "website".to_string(),
            actor: Some("admin".to_string()),
            website_id: Some("w1".to_string()),
            message: "created the website".to_string(),
            data: None,
        })
        .unwrap();
    store
        .log(&LogEntry {
            at: now,
            level: "warn".to_string(),
            kind: "import".to_string(),
            actor: None,
            website_id: None,
            message: "the import was short".to_string(),
            data: Some("{\"rows\":3}".to_string()),
        })
        .unwrap();

    assert_eq!(store.logs(None, 10).unwrap().len(), 2);
    let imports = store.logs(Some("import"), 10).unwrap();
    assert_eq!(imports.len(), 1);
    assert_eq!(imports[0].level, "warn");
    assert_eq!(imports[0].data.as_deref(), Some("{\"rows\":3}"));
}

/// A Search Console export lands whole or not at all.
#[test]
fn the_search_console_import_is_one_transaction() {
    let store = store();

    let rows: Vec<GscRow> = (0..5)
        .map(|i| GscRow {
            date: format!("2026-03-{:02}", i + 1),
            query: Some(format!("query {i}")),
            page: Some("/page".to_string()),
            clicks: i * 10,
            impressions: i * 100,
            ctr: Some(0.1),
            position: Some(3.5),
        })
        .collect();

    let report: ImportReport = store
        .import_gsc("w1", &rows, Some("gsc.csv"))
        .unwrap();
    assert_eq!(report.rows, 5);

    let stored = store.gsc_rows("w1", 100).unwrap();
    assert_eq!(stored.len(), 5);
    // Newest day first.
    assert_eq!(stored[0].date, "2026-03-05");
    assert_eq!(stored[0].clicks, 40);
    assert_eq!(stored[0].impressions, 400);

    let imports = store.gsc_imports("w1").unwrap();
    assert_eq!(imports.len(), 1);
    assert_eq!(imports[0].rows, 5);

    // A second import adds to the first.
    store.import_gsc("w1", &rows[..2], None).unwrap();
    assert_eq!(store.gsc_rows("w1", 100).unwrap().len(), 7);
    assert_eq!(store.gsc_imports("w1").unwrap().len(), 2);
}

/// The cache holds a value until it expires.
#[test]
fn the_cache_expires() {
    let store = store();

    assert!(store.cache_get("k").unwrap().is_none());

    store.cache_put("k", "value", None).unwrap();
    assert_eq!(store.cache_get("k").unwrap().as_deref(), Some("value"));
    assert_eq!(store.cache_len().unwrap(), 1);

    // A value with a lifetime in the past is already gone.
    store.cache_put("old", "value", Some(-10)).unwrap();
    assert!(store.cache_get("old").unwrap().is_none());

    // Putting the same key again replaces it.
    store.cache_put("k", "other", None).unwrap();
    assert_eq!(store.cache_get("k").unwrap().as_deref(), Some("other"));
    assert_eq!(store.cache_len().unwrap(), 2);

    assert_eq!(store.cache_purge().unwrap(), 1);
    assert_eq!(store.cache_len().unwrap(), 1);
}

/// A saved report keeps its query and its result.
#[test]
fn the_snapshots_are_saved_by_name() {
    let store = store();

    let snapshot: Snapshot = store
        .save_snapshot("monday", "{\"website\":\"w1\"}", "{\"pageviews\":42}")
        .unwrap();
    assert_eq!(snapshot.name, "monday");
    assert_eq!(snapshot.query, "{\"website\":\"w1\"}");

    let snapshots = store.snapshots().unwrap();
    assert_eq!(snapshots.len(), 1);
    assert_eq!(snapshots[0].result, "{\"pageviews\":42}");

    // Saving under the same name replaces it.
    store
        .save_snapshot("monday", "{\"website\":\"w1\"}", "{\"pageviews\":43}")
        .unwrap();
    let snapshots = store.snapshots().unwrap();
    assert_eq!(snapshots.len(), 1);
    assert_eq!(snapshots[0].result, "{\"pageviews\":43}");

    assert!(store.delete_snapshot("monday").unwrap());
    assert!(store.snapshots().unwrap().is_empty());
    assert!(!store.delete_snapshot("monday").unwrap());
}

/// A conversation keeps its messages, in order.
#[test]
fn the_chat_keeps_its_messages() {
    let store = store();

    let session: ChatSession = store
        .chat_session(Some("w1"), Some("Why did the traffic drop?"))
        .unwrap();
    assert_eq!(session.website_id.as_deref(), Some("w1"));

    let question: ChatMessage = store
        .chat_message(&session.id, "user", "Why did the traffic drop?", None)
        .unwrap();
    assert_eq!(question.role, "user");

    store
        .chat_message(
            &session.id,
            "assistant",
            "Because the referrer changed.",
            Some("{\"tool\":\"stats\"}"),
        )
        .unwrap();

    let messages = store.chat_messages(&session.id).unwrap();
    assert_eq!(messages.len(), 2);
    // Oldest first.
    assert_eq!(messages[0].role, "user");
    assert_eq!(messages[1].role, "assistant");
    assert_eq!(messages[1].data.as_deref(), Some("{\"tool\":\"stats\"}"));

    let sessions = store.chat_sessions().unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].id, session.id);
}

/// The account the store hands back is the one that was written.
#[test]
fn the_account_round_trips() {
    let store = store();
    store.set_password("admin", "hash").unwrap();

    let account: Account = store.account("admin").unwrap().unwrap();
    assert_eq!(account.username, "admin");
    assert_eq!(account.role, "admin");
    assert!(account.team_id.is_none());
    assert!(account.created_at.and_utc().timestamp() > 0);
}
