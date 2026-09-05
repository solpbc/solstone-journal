// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Describe-local generate-session stub. Kept separate from generate's contract stub.

use std::env;
use std::ffi::OsStr;
use std::io::{self, BufRead, Write};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use solstone_core_generate::{
    ContentPart, contract, decode_session_request_line, decode_session_terminal_line,
};

fn main() {
    let args = env::args_os().collect::<Vec<_>>();
    assert_eq!(
        args.get(1).map(|value| value.as_os_str()),
        Some(OsStr::new("generate"))
    );
    let session = &contract()["framing"]["session"];
    let selector = session["selector"].as_str().unwrap();
    assert_eq!(
        args.get(2).map(|value| value.as_os_str()),
        Some(OsStr::new(selector))
    );
    let concurrency_flag = session["concurrency"]["flag"].as_str().unwrap();
    let journal_flag = session["journal"]["flag"].as_str().unwrap();
    let mut concurrency_seen = false;
    let mut journal_seen = false;
    let mut journal_path = None;
    let pairs = &args[3..];
    assert_eq!(pairs.len() % 2, 0);
    for pair in pairs.chunks_exact(2) {
        if pair[0].as_os_str() == OsStr::new(concurrency_flag) {
            assert!(!concurrency_seen);
            concurrency_seen = true;
        } else if pair[0].as_os_str() == OsStr::new(journal_flag) {
            assert!(!journal_seen);
            journal_seen = true;
            journal_path = Some(pair[1].to_string_lossy().into_owned());
        } else {
            panic!("unexpected generate session flag: {:?}", pair[0]);
        }
    }
    assert!(concurrency_seen);
    let mode =
        env::var("SOLSTONE_DESCRIBE_SESSION_STUB_MODE").unwrap_or_else(|_| "generated".to_owned());
    if let Some(path) = env::var_os("SOLSTONE_DESCRIBE_SESSION_STUB_PID_PATH") {
        std::fs::write(path, std::process::id().to_string()).unwrap();
    }
    if let Some(path) = env::var_os("SOLSTONE_DESCRIBE_SESSION_STUB_LAUNCH_LOG_PATH") {
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .unwrap();
        writeln!(file, "{}", std::process::id()).unwrap();
    }
    let mut seen = 0usize;
    let pause_after = env::var("SOLSTONE_DESCRIBE_SESSION_STUB_PAUSE_AFTER")
        .ok()
        .and_then(|value| value.parse::<usize>().ok());
    let mut held_id = None;
    let mut categorized_frame_ids = Vec::new();
    for line in io::stdin().lock().lines() {
        let line = line.unwrap();
        if decode_session_terminal_line(&line).is_ok() {
            return;
        }
        let request = decode_session_request_line(&line).unwrap();
        seen += 1;
        if request.context == "observe.describe.frame"
            && let Some(frame_id) = request
                .id
                .as_deref()
                .and_then(|id| id.strip_prefix("frame:"))
                .and_then(|id| id.split(':').next())
                .and_then(|id| id.parse::<u64>().ok())
            && !categorized_frame_ids.contains(&frame_id)
        {
            categorized_frame_ids.push(frame_id);
        }
        if let Some(path) = env::var_os("SOLSTONE_DESCRIBE_SESSION_STUB_REQUESTS_PATH") {
            let mut file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .unwrap();
            let contents = request
                .contents
                .iter()
                .map(|part| match part {
                    ContentPart::Text { text } => json!({"type":"text","text":text}),
                    ContentPart::Image { mime_type, .. } => {
                        json!({"type":"image","mime_type":mime_type})
                    }
                })
                .collect::<Vec<_>>();
            writeln!(file, "{}", json!({"id":request.id,"attempt_index":request.attempt_index,"context":request.context,"contents":contents,"json_output":request.json_output,"json_schema":request.json_schema,"temperature":request.temperature,"max_output_tokens":request.max_output_tokens,"thinking_budget":request.thinking_budget,"system_instruction":request.system_instruction,"journal":journal_path})).unwrap();
        }
        if mode == "exit_after_all" && seen == 3 {
            return;
        }
        if mode == "exit_after_one" && seen == 1 {
            return;
        }
        if mode == "hold_first_until_second" && seen == 1 {
            held_id = request.id;
            continue;
        }
        let id = request.id.unwrap();
        if request.context == "observe.extract.selection" && mode == "selection_skips_failed_first"
        {
            generated_text(&id, r#"{"frame_ids":[13]}"#);
            pause_after_response(pause_after, seen);
            continue;
        }
        if request.context == "observe.extract.selection" && mode == "generated" {
            generated_text(
                &id,
                &json!({"frame_ids": categorized_frame_ids}).to_string(),
            );
            pause_after_response(pause_after, seen);
            continue;
        }
        if request.context == "observe.extract.selection" && mode.starts_with("selection_") {
            selection_response(&id, &mode);
            pause_after_response(pause_after, seen);
            continue;
        }
        if request.context != "observe.describe.frame"
            && request.context != "observe.extract.selection"
            && mode.starts_with("extraction_")
        {
            extraction_response(&id, &mode, request.attempt_index);
            pause_after_response(pause_after, seen);
            continue;
        }
        if request.context == "observe.describe.frame"
            && (mode.starts_with("category_")
                || matches!(
                    mode.as_str(),
                    "extraction_json_retry_then_succeed"
                        | "extraction_json_unparseable"
                        | "selection_skips_failed_first"
                ))
        {
            if mode == "selection_skips_failed_first" && id.starts_with("frame:1:") {
                refused(&id, true, false, Some("brain_refresh_timeout"));
                pause_after_response(pause_after, seen);
                continue;
            }
            categorization_response(&id, &mode);
            pause_after_response(pause_after, seen);
            continue;
        }
        if mode == "always_retryable" || (mode == "retryable_then_generated" && seen == 1) {
            refused(&id, true, false, Some("brain_refresh_timeout"));
        } else if mode == "blocking_retryable" {
            refused(&id, true, true, Some("binary_missing"));
        } else if mode == "no_engine_configured" {
            no_engine(&id);
        } else if mode == "non_responsive" {
            refused(&id, false, false, Some("non_responsive"));
        } else if mode == "unknown_code" {
            refused(&id, false, true, Some("future_code"));
        } else if mode == "generated_unknown_finish" {
            generated_with_finish(&id, "unknown");
        } else {
            generated(&id);
        }
        if let Some(first_id) = held_id.take() {
            generated(&first_id);
        }
        pause_after_response(pause_after, seen);
    }
}

fn pause_after_response(pause_after: Option<usize>, seen: usize) {
    if pause_after == Some(seen) {
        wait_for_release();
    }
}

fn categorization_response(id: &str, mode: &str) {
    let (primary, secondary, overlap) = match mode {
        "category_browsing" => ("browsing", "none", true),
        "category_messaging" => ("messaging", "none", true),
        "category_meeting" => ("meeting", "none", true),
        "category_secondary" => ("code", "messaging", false),
        "category_media" => ("media", "none", true),
        "category_secondary_social" => ("code", "social", true),
        "category_unextractable" => ("not-a-real-category", "none", true),
        "category_schema_valid" | "category_schema_invalid" => ("code", "none", true),
        "extraction_json_retry_then_succeed" => ("messaging", "none", true),
        "extraction_json_unparseable" => ("messaging", "none", true),
        "selection_skips_failed_first" => ("code", "none", true),
        _ => unreachable!("category mode"),
    };
    let text = json!({"visual_description":"stub","primary":primary,"secondary":secondary,"overlap":overlap}).to_string();
    let schema_validation = if mode == "category_schema_invalid" && id.starts_with("frame:1:") {
        json!({"valid": false, "errors": [{"path": "", "constraint": "required", "message": "stub"}]})
    } else if mode == "category_schema_valid" || mode == "category_schema_invalid" {
        json!({"valid": true, "errors": []})
    } else {
        Value::Null
    };
    generated_text_with_finish(id, &text, "stop", schema_validation);
}

fn extraction_response(id: &str, mode: &str, attempt: u64) {
    match mode {
        "extraction_markdown_success" => generated_text(id, "# extracted markdown"),
        "extraction_markdown_unknown" => {
            generated_text_with_finish(id, "# extracted markdown", "unknown", Value::Null)
        }
        "extraction_markdown_empty" => {
            generated_text_with_finish(id, "# extracted markdown", "", Value::Null)
        }
        "extraction_markdown_length" => {
            generated_text_with_finish(id, "truncated", "length", Value::Null)
        }
        "extraction_markdown_blank" => generated_text_with_finish(id, "   \n", "stop", Value::Null),
        "extraction_json_retry_then_succeed" if attempt == 0 => generated_text(id, "not json"),
        "extraction_json_retry_then_succeed" => generated_text(id, r#"{"ok":true}"#),
        "extraction_json_unparseable" => generated_text(id, "not json"),
        "extraction_refusal" => refused(id, true, false, Some("brain_refresh_timeout")),
        "extraction_blocking_refusal" => refused(id, true, true, Some("binary_missing")),
        _ => generated_text(id, r#"{"ok":true}"#),
    }
}

fn selection_response(id: &str, mode: &str) {
    match mode {
        "selection_bare_array" => generated_text(id, "[3,1,2]"),
        "selection_over_cap" => generated_text(id, r#"{"frame_ids":[999,3,1,2,0]}"#),
        "selection_unparseable" => generated_text(id, "not JSON"),
        "selection_blocking_refusal" => refused(id, true, true, Some("binary_missing")),
        "selection_nonblocking_retryable_refusal" => {
            refused(id, true, false, Some("brain_refresh_timeout"));
        }
        "selection_unknown_code" => refused(id, false, false, Some("future_code")),
        _ => unreachable!("selection-only mode"),
    }
}

fn generated(id: &str) {
    generated_with_finish(id, "stop");
}
fn generated_with_finish(id: &str, finish_reason: &str) {
    let text =
        json!({"visual_description":"stub","primary":"code","secondary":"none","overlap":true})
            .to_string();
    generated_text_with_finish(id, &text, finish_reason, Value::Null);
}
fn generated_text(id: &str, text: &str) {
    generated_text_with_finish(id, text, "stop", Value::Null);
}
fn generated_text_with_finish(id: &str, text: &str, finish_reason: &str, schema_validation: Value) {
    println!(
        "{}",
        json!({"schema":contract()["schema_identifiers"]["response"],"id":id,"outcome":"generated","text":text,"model":"describe-stub","usage":{},"finish_reason":finish_reason,"thinking":null,"schema_validation":schema_validation,"input_budget":null,"request_budget":null,"inference":null})
    );
    let _ = io::stdout().flush();
}
fn refused(id: &str, retryable: bool, blocking: bool, code: Option<&str>) {
    println!(
        "{}",
        json!({"schema":contract()["schema_identifiers"]["response"],"id":id,"outcome":"refused","reason":"provider-response-invalid","reason_code":code,"retryable":retryable,"blocking":blocking,"reset_at_ms":null,"provider":"stub","detail":"stub refusal"})
    );
    let _ = io::stdout().flush();
}
fn no_engine(id: &str) {
    println!(
        "{}",
        json!({"schema":contract()["schema_identifiers"]["response"],"id":id,"outcome":"refused","reason":"no-engine-configured","reason_code":null,"retryable":false,"blocking":true,"reset_at_ms":null,"provider":null,"detail":"no engine"})
    );
    let _ = io::stdout().flush();
}

fn wait_for_release() {
    let Some(marker) = env::var_os("SOLSTONE_DESCRIBE_SESSION_STUB_RELEASE_PATH") else {
        return;
    };
    // Matches the caller's own hang-prevention ceiling (see cli.rs) so this
    // stub never self-unblocks while the caller is still legitimately
    // waiting on it under contention.
    let deadline = Instant::now() + Duration::from_secs(60);
    while !std::path::Path::new(&marker).exists() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
}
