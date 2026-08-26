// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::{Value, json};

const SOURCE: &str = include_str!("../assets/static/websocket.js");

fn status_map() -> Vec<Value> {
    const START: &str = "const STATUS_MAP = [";
    let start = SOURCE.find(START).unwrap_or_else(|| {
        panic!("the production table could not be extracted: missing `const STATUS_MAP = [`")
    });
    let json_start = start + "const STATUS_MAP = ".len();
    let rest = &SOURCE[json_start..];
    let end = rest
        .match_indices('\n')
        .find_map(|(at, _)| {
            let after_newline = rest.get(at + 1..)?;
            let trimmed = after_newline.trim_start_matches(' ');
            trimmed.starts_with("];").then(|| {
                let spaces = after_newline.len() - trimmed.len();
                at + 1 + spaces
            })
        })
        .unwrap_or_else(|| {
            panic!("the production table could not be extracted: missing closing `];`")
        });
    let json = &rest[..=end];
    let parsed: Value = serde_json::from_str(json)
        .unwrap_or_else(|error| panic!("the production table could not be extracted: {error}"));
    match parsed {
        Value::Array(rows) => rows,
        other => panic!(
            "the production table could not be extracted: expected a JSON array, got {other}"
        ),
    }
}

fn assert_row(row: &Value, ws: &str, capture: Value, unviewed: Value, variant: &str) {
    assert_eq!(row["ws"], ws, "ws");
    assert_eq!(row["capture"], capture, "capture");
    assert_eq!(row["unviewed"], unviewed, "unviewed");
    assert_eq!(row["variant"], variant, "variant");
}

fn row_with_capture<'a>(rows: &'a [Value], capture: &str) -> &'a Value {
    rows.iter()
        .find(|row| row["capture"] == capture)
        .unwrap_or_else(|| panic!("STATUS_MAP has no capture={capture} row"))
}

#[test]
fn status_mark_table_maps_every_contract_condition_to_its_stem() {
    let rows = status_map();
    assert_eq!(rows.len(), 10, "STATUS_MAP must have the ten contract rows");
    assert_row(
        &rows[0],
        "connecting",
        json!("*"),
        json!("*"),
        "mark-connecting",
    );
    assert_row(
        &rows[1],
        "connected",
        Value::Null,
        json!("*"),
        "mark-connecting",
    );
    assert_row(
        &rows[2],
        "disconnected",
        json!("*"),
        json!("*"),
        "mark-offline",
    );
    assert_row(&rows[3], "*", json!("offline"), json!("*"), "mark-offline");
    assert_row(
        &rows[4],
        "*",
        json!("degraded"),
        json!("*"),
        "mark-attention",
    );
    assert_row(&rows[5], "*", json!("*"), json!(true), "mark-attention");
    assert_row(&rows[6], "*", json!("stale"), json!("*"), "mark-attention");
    assert_row(
        &rows[7],
        "*",
        json!("no_clients"),
        json!("*"),
        "mark-paused",
    );
    assert_row(&rows[8], "*", json!("active"), json!("*"), "mark");
    assert_row(&rows[9], "*", json!("*"), json!("*"), "mark-offline");
}

#[test]
fn status_mark_table_rejects_error_stem_and_offline_attention_inversions() {
    let rows = status_map();
    let offline = row_with_capture(&rows, "offline");
    assert_eq!(offline["variant"], "mark-offline");
    assert_ne!(offline["variant"], "mark-error");

    let degraded = row_with_capture(&rows, "degraded");
    assert_eq!(degraded["variant"], "mark-attention");
    assert_ne!(degraded["variant"], "mark-offline");

    let unviewed = rows
        .iter()
        .find(|row| row["unviewed"] == true)
        .expect("STATUS_MAP has no unviewed row");
    assert_eq!(unviewed["variant"], "mark-attention");
    assert_ne!(unviewed["variant"], "mark-offline");

    let stale = row_with_capture(&rows, "stale");
    assert_eq!(stale["variant"], "mark-attention");
    assert_ne!(stale["variant"], "mark-offline");

    let catch_all = rows
        .iter()
        .find(|row| row["ws"] == "*" && row["capture"] == "*" && row["unviewed"] == "*")
        .expect("STATUS_MAP has no catch-all row");
    assert_eq!(catch_all["variant"], "mark-offline");
    assert_ne!(catch_all["variant"], "mark-error");

    let disconnected = rows
        .iter()
        .find(|row| row["ws"] == "disconnected")
        .expect("STATUS_MAP has no disconnected row");
    assert_ne!(disconnected["variant"], "mark-error");

    assert!(
        rows.iter().all(|row| row["variant"] != "mark-error"),
        "STATUS_MAP must not name mark-error"
    );
}
