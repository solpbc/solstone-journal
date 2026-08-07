// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Describe-local generate-session stub. Kept separate from generate's contract stub.

use std::env;
use std::io::{self, BufRead, Write};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::json;
use solstone_core_generate::{contract, decode_session_request_line, decode_session_terminal_line};

fn main() {
    let args = env::args().collect::<Vec<_>>();
    assert_eq!(args.get(1).map(String::as_str), Some("--session"));
    let mode =
        env::var("SOLSTONE_DESCRIBE_SESSION_STUB_MODE").unwrap_or_else(|_| "generated".to_owned());
    if let Some(path) = env::var_os("SOLSTONE_DESCRIBE_SESSION_STUB_PID_PATH") {
        std::fs::write(path, std::process::id().to_string()).unwrap();
    }
    let mut seen = 0usize;
    let mut held_id = None;
    for line in io::stdin().lock().lines() {
        let line = line.unwrap();
        if decode_session_terminal_line(&line).is_ok() {
            return;
        }
        let request = decode_session_request_line(&line).unwrap();
        seen += 1;
        if let Some(path) = env::var_os("SOLSTONE_DESCRIBE_SESSION_STUB_REQUESTS_PATH") {
            let mut file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .unwrap();
            writeln!(file, "{}", json!({"id":request.id,"attempt_index":request.attempt_index,"context":request.context,"json_output":request.json_output,"temperature":request.temperature,"max_output_tokens":request.max_output_tokens,"thinking_budget":request.thinking_budget,"system_instruction":request.system_instruction})).unwrap();
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
        if request.context == "observe.extract.selection" && mode.starts_with("selection_") {
            selection_response(&id, &mode);
            if let Some(path) = env::var_os("SOLSTONE_DESCRIBE_SESSION_STUB_STATS_PATH") {
                std::fs::write(path, json!({"requests": seen}).to_string()).unwrap();
            }
            continue;
        }
        if request.context != "observe.describe.frame"
            && request.context != "observe.extract.selection"
            && mode.starts_with("extraction_")
        {
            extraction_response(&id, &mode, request.attempt_index);
            continue;
        }
        if request.context == "observe.describe.frame"
            && (mode.starts_with("category_")
                || matches!(
                    mode.as_str(),
                    "extraction_json_retry_then_succeed" | "extraction_json_unparseable"
                ))
        {
            categorization_response(&id, &mode);
            continue;
        }
        if mode == "always_retryable" || (mode == "retryable_then_generated" && seen == 1) {
            refused(&id, true, false, Some("chat_timeout"));
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
        if mode == "pause_after_first"
            && request.context != "observe.describe.frame"
            && request.context != "observe.extract.selection"
            && seen >= 3
        {
            wait_for_release();
        }
        if let Some(path) = env::var_os("SOLSTONE_DESCRIBE_SESSION_STUB_STATS_PATH") {
            std::fs::write(path, json!({"requests": seen}).to_string()).unwrap();
        }
    }
}

fn categorization_response(id: &str, mode: &str) {
    let (primary, secondary, overlap) = match mode {
        "category_browsing" => ("browsing", "none", true),
        "category_messaging" => ("messaging", "none", true),
        "category_meeting" => ("meeting", "none", true),
        "category_secondary" => ("code", "messaging", false),
        "extraction_json_retry_then_succeed" => ("messaging", "none", true),
        "extraction_json_unparseable" => ("messaging", "none", true),
        _ => unreachable!("category mode"),
    };
    let text = json!({"visual_description":"stub","primary":primary,"secondary":secondary,"overlap":overlap}).to_string();
    generated_text(id, &text);
}

fn extraction_response(id: &str, mode: &str, attempt: u64) {
    match mode {
        "extraction_markdown_success" => generated_text(id, "# extracted markdown"),
        "extraction_markdown_unknown" => {
            generated_text_with_finish(id, "# extracted markdown", "unknown")
        }
        "extraction_markdown_length" => generated_text_with_finish(id, "truncated", "length"),
        "extraction_json_retry_then_succeed" if attempt == 0 => generated_text(id, "not json"),
        "extraction_json_retry_then_succeed" => generated_text(id, r#"{"ok":true}"#),
        "extraction_json_unparseable" => generated_text(id, "not json"),
        "extraction_refusal" => refused(id, true, false, Some("chat_timeout")),
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
            refused(id, true, false, Some("chat_timeout"));
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
    generated_text_with_finish(id, &text, finish_reason);
}
fn generated_text(id: &str, text: &str) {
    generated_text_with_finish(id, text, "stop");
}
fn generated_text_with_finish(id: &str, text: &str, finish_reason: &str) {
    println!(
        "{}",
        json!({"schema":contract()["schema_identifiers"]["response"],"id":id,"outcome":"generated","text":text,"model":"describe-stub","usage":{},"finish_reason":finish_reason,"thinking":null,"schema_validation":null,"input_budget":null,"request_budget":null,"inference":null})
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
    let deadline = Instant::now() + Duration::from_secs(10);
    while !std::path::Path::new(&marker).exists() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
}
