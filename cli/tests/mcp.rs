//! The MCP server — the protocol, and the three tools.
//!
//! The server is spoken to the way a client speaks to it: JSON-RPC 2.0, one
//! message per line, over stdio. These tests drive it the same way.

use std::path::PathBuf;

use serde_json::{json, Value};

use sumimi_cli::mcp::Server;
use sumimi_service::models::Hit;
use sumimi_service::pool::Db;
use sumimi_service::testing::HitBuilder;
use sumimi_service::storage::Storage;

/// A temporary database with one website and a few events.
fn instance(name: &str) -> (PathBuf, String) {
    let dir = std::env::temp_dir().join(format!("sumimi-mcp-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("sumimi.duckdb");

    let db = Db::open(&path, "256MB").unwrap();
    db.with_write(|conn| {
        conn.execute(
            "INSERT INTO website (id, name, domain, created_at, updated_at, retention_days, country_header)
             VALUES ('w1', 'Example', 'example.com', now(), now(), 0, NULL)",
            [],
        )?;
        Ok::<_, anyhow::Error>(())
    })
    .unwrap();

    let now = chrono::Utc::now().naive_utc();
    let hits = vec![
        hit("w1", "/one", "s1", "v1", now),
        hit("w1", "/two", "s1", "v1", now),
        hit("w1", "/one", "s2", "v2", now),
    ];
    db.write_batch(&hits).unwrap();

    (path, "w1".to_string())
}

/// One page view.
fn hit(website: &str, url: &str, session: &str, visitor: &str, now: chrono::NaiveDateTime) -> Hit {
    HitBuilder::new(website, session, visitor)
        .at(now)
        .url(url)
        .hostname("example.com")
        .browser("Chrome")
        .build()
}

/// Send a request and get the answer.
fn call(server: &Server, request: Value) -> Value {
    let response = server
        .handle_line(&request.to_string())
        .expect("a request with an id gets an answer");
    serde_json::from_str(&response).expect("the answer is json")
}

/// The text of a tool result.
fn tool_text(value: &Value) -> String {
    value["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// The handshake answers with the protocol version that was asked for.
#[test]
fn the_handshake_answers_with_the_version_that_was_asked_for() {
    let (path, _) = instance("handshake");
    let server = Server::open_local(&path, None).unwrap();

    let answer = call(
        &server,
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": { "protocolVersion": "2025-03-26" },
        }),
    );

    assert_eq!(answer["jsonrpc"], "2.0");
    assert_eq!(answer["id"], 1);
    assert_eq!(answer["result"]["protocolVersion"], "2025-03-26");
    assert_eq!(answer["result"]["serverInfo"]["name"], "sumimi");
    assert!(answer["result"]["capabilities"]["tools"].is_object());
}

/// A client that asks for a version this server does not know gets the newest one.
#[test]
fn an_unknown_protocol_version_gets_the_newest_one() {
    let (path, _) = instance("version");
    let server = Server::open_local(&path, None).unwrap();

    let answer = call(
        &server,
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": { "protocolVersion": "1999-01-01" },
        }),
    );

    assert_eq!(
        answer["result"]["protocolVersion"],
        sumimi_cli::mcp::PROTOCOL_VERSION
    );
}

/// A notification gets no answer.
#[test]
fn a_notification_gets_no_answer() {
    let (path, _) = instance("notification");
    let server = Server::open_local(&path, None).unwrap();

    let answer = server.handle_line(
        &json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }).to_string(),
    );
    assert!(answer.is_none(), "a notification must not be answered");
}

/// There are three tools, and they are the three of them.
#[test]
fn the_tools_are_the_three_of_them() {
    let (path, _) = instance("tools");
    let server = Server::open_local(&path, None).unwrap();

    let answer = call(
        &server,
        json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }),
    );

    let tools = answer["result"]["tools"].as_array().expect("the tools");
    let names: Vec<&str> = tools
        .iter()
        .map(|tool| tool["name"].as_str().unwrap_or_default())
        .collect();

    assert_eq!(names, ["sumimi_report", "sumimi_record", "sumimi_ask"]);

    // Every tool says what it is and what it takes.
    for tool in tools {
        assert!(
            !tool["description"].as_str().unwrap_or_default().is_empty(),
            "{} has no description",
            tool["name"]
        );
        assert_eq!(tool["inputSchema"]["type"], "object");
    }
}

/// A report returns the numbers of the database.
#[test]
fn a_report_returns_the_numbers() {
    let (path, website) = instance("report");
    let server = Server::open_local(&path, None).unwrap();

    let answer = call(
        &server,
        json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "tools/call",
            "params": {
                "name": "sumimi_report",
                "arguments": { "website": website, "report": "summary" },
            },
        }),
    );

    assert_eq!(answer["result"]["isError"], false);
    let text = tool_text(&answer);
    let value: Value = serde_json::from_str(&text).expect("the report is json");
    assert_eq!(value["pageviews"]["value"], 3);
    assert_eq!(value["visitors"]["value"], 2);
}

/// The website can be named by its domain.
#[test]
fn a_website_can_be_named_by_its_domain() {
    let (path, _) = instance("domain");
    let server = Server::open_local(&path, None).unwrap();

    let answer = call(
        &server,
        json!({
            "jsonrpc": "2.0",
            "id": 4,
            "method": "tools/call",
            "params": {
                "name": "sumimi_report",
                "arguments": { "website": "example.com", "report": "pages" },
            },
        }),
    );

    assert_eq!(answer["result"]["isError"], false);
    let text = tool_text(&answer);
    assert!(text.contains("/one"), "{text}");
}

/// A question is answered from the database.
#[test]
fn a_question_is_answered_from_the_database() {
    let (path, website) = instance("ask");
    let server = Server::open_local(&path, None).unwrap();

    let answer = call(
        &server,
        json!({
            "jsonrpc": "2.0",
            "id": 5,
            "method": "tools/call",
            "params": {
                "name": "sumimi_ask",
                "arguments": { "website": website, "question": "how many page views in the last 7 days" },
            },
        }),
    );

    assert_eq!(answer["result"]["isError"], false);
    let text = tool_text(&answer);
    assert!(text.contains('3'), "the answer does not have the number: {text}");
}

/// An event that is recorded is in the database afterwards.
#[test]
fn an_event_that_is_recorded_is_in_the_database() {
    let (path, website) = instance("record");
    let server = Server::open_local(&path, None).unwrap();

    let answer = call(
        &server,
        json!({
            "jsonrpc": "2.0",
            "id": 6,
            "method": "tools/call",
            "params": {
                "name": "sumimi_record",
                "arguments": {
                    "website": website,
                    "type": "event",
                    "name": "signup",
                    "url": "/pricing",
                    "value": "pro",
                },
            },
        }),
    );

    assert_eq!(answer["result"]["isError"], false);
    assert!(tool_text(&answer).contains("signup"));

    // It is in the events report now.
    let answer = call(
        &server,
        json!({
            "jsonrpc": "2.0",
            "id": 7,
            "method": "tools/call",
            "params": {
                "name": "sumimi_report",
                "arguments": { "website": website, "report": "events" },
            },
        }),
    );
    let text = tool_text(&answer);
    assert!(text.contains("signup"), "the event is not in the report: {text}");
}

/// A tool that fails is reported to the model, not to the protocol.
#[test]
fn a_failing_tool_is_reported_to_the_model() {
    let (path, _) = instance("failing");
    let server = Server::open_local(&path, None).unwrap();

    let answer = call(
        &server,
        json!({
            "jsonrpc": "2.0",
            "id": 8,
            "method": "tools/call",
            "params": {
                "name": "sumimi_report",
                "arguments": { "website": "nope", "report": "summary" },
            },
        }),
    );

    assert_eq!(answer["result"]["isError"], true);
    assert!(tool_text(&answer).contains("nope"));
}

/// An unknown tool is a protocol error.
#[test]
fn an_unknown_tool_is_a_protocol_error() {
    let (path, _) = instance("unknown");
    let server = Server::open_local(&path, None).unwrap();

    let answer = call(
        &server,
        json!({
            "jsonrpc": "2.0",
            "id": 9,
            "method": "tools/call",
            "params": { "name": "sumimi_delete_everything" },
        }),
    );

    assert_eq!(answer["error"]["code"], -32602);
    assert!(answer["error"]["message"]
        .as_str()
        .unwrap_or_default()
        .contains("sumimi_delete_everything"));
}

/// An unknown method is a protocol error.
#[test]
fn an_unknown_method_is_a_protocol_error() {
    let (path, _) = instance("method");
    let server = Server::open_local(&path, None).unwrap();

    let answer = call(
        &server,
        json!({ "jsonrpc": "2.0", "id": 10, "method": "resources/list" }),
    );

    assert_eq!(answer["error"]["code"], -32601);
}

/// Broken json is a protocol error, not a crash.
#[test]
fn broken_json_is_a_protocol_error() {
    let (path, _) = instance("broken");
    let server = Server::open_local(&path, None).unwrap();

    let response = server.handle_line("{not json").expect("an answer");
    let answer: Value = serde_json::from_str(&response).unwrap();
    assert_eq!(answer["error"]["code"], -32700);
}

/// The window of a call is honoured.
#[test]
fn the_window_of_a_call_is_honoured() {
    let (path, website) = instance("window");
    let server = Server::open_local(&path, None).unwrap();

    // The events are from now, so a window that ends in the past has none.
    let answer = call(
        &server,
        json!({
            "jsonrpc": "2.0",
            "id": 11,
            "method": "tools/call",
            "params": {
                "name": "sumimi_report",
                "arguments": {
                    "website": website,
                    "report": "summary",
                    "from": "2020-01-01",
                    "to": "2020-01-02",
                },
            },
        }),
    );

    let text = tool_text(&answer);
    let value: Value = serde_json::from_str(&text).expect("the report is json");
    assert_eq!(value["pageviews"]["value"], 0);
}
