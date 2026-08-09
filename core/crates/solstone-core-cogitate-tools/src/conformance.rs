// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;

use serde_json::{Map, Value, json};

use crate::bed::{Bed, manifest, normalize_expected};
use crate::oracle::{ReadToolVector, fixture, sha256_hex};
use crate::*;

#[test]
fn read_tool_vectors_match_the_oracle() {
    let fixture = fixture();
    assert_eq!(fixture.read_tools.len(), 106);
    let bed = Bed::new();
    assert_eq!(
        manifest(&bed.root),
        normalize_expected(&fixture.bed_manifest.entries),
        "bed manifest"
    );
    let mut budgets = BTreeMap::new();
    for vector in &fixture.read_tools {
        let actual = invoke_vector(vector, &bed.root, &mut budgets);
        assert_eq!(actual.ok, vector.expect.ok, "{} ok", vector.id);
        assert_eq!(
            actual.refusal.map(str::to_owned),
            vector.expect.refusal,
            "{} refusal",
            vector.id
        );
        assert_eq!(
            actual.truncated, vector.expect.truncated,
            "{} truncated",
            vector.id
        );
        assert_eq!(
            actual.notice.map(str::to_owned),
            vector.expect.notice,
            "{} notice",
            vector.id
        );
        assert_eq!(
            payload_value(&actual.payload),
            vector.expect.payload,
            "{} payload",
            vector.id
        );
    }
}

#[test]
fn limits_and_refusals_match_the_oracle() {
    let fixture = fixture();
    let limits = &fixture.read_tool_limits;
    assert_eq!(limits["read_file_max_lines"], json!(READ_FILE_MAX_LINES));
    assert_eq!(limits["read_file_max_bytes"], json!(READ_FILE_MAX_BYTES));
    assert_eq!(
        limits["list_directory_max_entries"],
        json!(LIST_DIRECTORY_MAX_ENTRIES)
    );
    assert_eq!(limits["glob_max_matches"], json!(GLOB_MAX_MATCHES));
    assert_eq!(limits["grep_max_matches"], json!(GREP_MAX_MATCHES));
    assert_eq!(limits["grep_max_files"], json!(GREP_MAX_FILES));
    assert_eq!(
        limits["grep_max_bytes_per_file"],
        json!(GREP_MAX_BYTES_PER_FILE)
    );
    assert_eq!(limits["default_read_call_budget"], json!(200));
    let mut components = DENIED_PATH_COMPONENTS.to_vec();
    components.sort_unstable();
    assert_eq!(limits["denied_path_components"], json!(components));
    assert_eq!(
        limits["denied_credential_patterns"],
        json!(DENIED_CREDENTIAL_PATTERNS)
    );
    for (name, actual) in refusal_constants() {
        assert_eq!(fixture.refusal_strings[name], actual, "{name}");
    }
}

fn invoke_vector(
    vector: &ReadToolVector,
    journal: &std::path::Path,
    budgets: &mut BTreeMap<i64, ReadBudget>,
) -> ReadResult {
    let budget_cap = number(&vector.args, "budget_cap");
    let budget = budget_cap.map(|cap| budgets.entry(cap).or_insert_with(|| ReadBudget::new(cap)));
    match vector.tool.as_str() {
        "read_file" => {
            let options = ReadFileOptions {
                start_line: number_or(&vector.args, "start_line", 1),
                max_lines: number_or(&vector.args, "max_lines", READ_FILE_MAX_LINES),
                max_bytes: number_or(&vector.args, "max_bytes", READ_FILE_MAX_BYTES),
            };
            read_file(
                journal,
                string_or(&vector.args, "path", "."),
                &options,
                budget,
            )
        }
        "list_directory" => {
            let options = ListDirectoryOptions {
                recursive: boolean_or(&vector.args, "recursive", false),
                max_entries: number_or(&vector.args, "max_entries", LIST_DIRECTORY_MAX_ENTRIES),
                include_hidden: boolean_or(&vector.args, "include_hidden", false),
                pattern: optional_string(&vector.args, "pattern"),
            };
            list_directory(
                journal,
                string_or(&vector.args, "path", "."),
                &options,
                budget,
            )
        }
        "glob" => {
            let options = GlobOptions {
                max_matches: number_or(&vector.args, "max_matches", GLOB_MAX_MATCHES),
                include_hidden: boolean_or(&vector.args, "include_hidden", false),
            };
            glob(
                journal,
                string_or(&vector.args, "pattern", ""),
                string_or(&vector.args, "root", "."),
                &options,
                budget,
            )
        }
        "grep_search" => {
            let options = GrepSearchOptions {
                regex: boolean_or(&vector.args, "regex", false),
                case_sensitive: boolean_or(&vector.args, "case_sensitive", false),
                file_glob: optional_string(&vector.args, "file_glob"),
                context_lines: number_or(&vector.args, "context_lines", 0),
                max_matches: number_or(&vector.args, "max_matches", GREP_MAX_MATCHES),
                max_files: number_or(&vector.args, "max_files", GREP_MAX_FILES),
                max_bytes_per_file: number_or(
                    &vector.args,
                    "max_bytes_per_file",
                    GREP_MAX_BYTES_PER_FILE,
                ),
                include_hidden: boolean_or(&vector.args, "include_hidden", false),
            };
            grep_search(
                journal,
                string_or(&vector.args, "pattern", ""),
                string_or(&vector.args, "path", "."),
                &options,
                budget,
            )
        }
        tool => panic!("{}: unknown tool {tool}", vector.id),
    }
}

fn string_or<'a>(args: &'a Map<String, Value>, key: &str, default: &'a str) -> &'a str {
    args.get(key).and_then(Value::as_str).unwrap_or(default)
}
fn optional_string(args: &Map<String, Value>, key: &str) -> Option<String> {
    args.get(key).and_then(Value::as_str).map(str::to_owned)
}
fn boolean_or(args: &Map<String, Value>, key: &str, default: bool) -> bool {
    args.get(key).and_then(Value::as_bool).unwrap_or(default)
}
fn number(args: &Map<String, Value>, key: &str) -> Option<i64> {
    args.get(key).and_then(Value::as_i64)
}
fn number_or(args: &Map<String, Value>, key: &str, default: i64) -> i64 {
    number(args, key).unwrap_or(default)
}

fn payload_value(payload: &ReadPayload) -> Value {
    match payload {
        ReadPayload::Text(text) if text.chars().count() > 400 => json!({"kind":"sha256","byte_length":text.len(),"digest":sha256_hex(text.as_bytes()),"head":head(text, 120),"tail":tail(text, 120)}),
        ReadPayload::Text(text) => Value::String(text.clone()),
        ReadPayload::Entries(entries) => summarize(entries.iter().map(|entry| json!({"path":entry.path,"is_dir":entry.is_dir})).collect()),
        ReadPayload::Paths(paths) => summarize(paths.iter().cloned().map(Value::String).collect()),
        ReadPayload::Matches(matches) => summarize(matches.iter().map(|item| json!({"path":item.path,"lineno":item.lineno,"line":item.line,"before":item.before,"after":item.after})).collect()),
    }
}
fn summarize(items: Vec<Value>) -> Value {
    if items.len() > 25 {
        json!({"kind":"list_summary","count":items.len(),"head":items[..5].to_vec(),"tail":items[items.len()-5..].to_vec()})
    } else {
        Value::Array(items)
    }
}
fn head(value: &str, count: usize) -> String {
    value.chars().take(count).collect()
}
fn tail(value: &str, count: usize) -> String {
    let chars: Vec<char> = value.chars().collect();
    chars[chars.len().saturating_sub(count)..].iter().collect()
}

fn refusal_constants() -> [(&'static str, &'static str); 16] {
    [
        ("REFUSAL_PATH_ESCAPE", REFUSAL_PATH_ESCAPE),
        ("REFUSAL_DENIED_COMPONENT", REFUSAL_DENIED_COMPONENT),
        ("REFUSAL_CREDENTIAL_FILE", REFUSAL_CREDENTIAL_FILE),
        ("REFUSAL_NOT_FILE", REFUSAL_NOT_FILE),
        ("REFUSAL_BINARY", REFUSAL_BINARY),
        ("REFUSAL_SPECIAL_FILE", REFUSAL_SPECIAL_FILE),
        ("REFUSAL_MISSING", REFUSAL_MISSING),
        ("REFUSAL_PERMISSION_DENIED", REFUSAL_PERMISSION_DENIED),
        ("REFUSAL_BAD_PATH", REFUSAL_BAD_PATH),
        ("REFUSAL_BAD_PATTERN", REFUSAL_BAD_PATTERN),
        ("REFUSAL_BUDGET_EXHAUSTED", REFUSAL_BUDGET_EXHAUSTED),
        ("REFUSAL_BROAD_ROOT", REFUSAL_BROAD_ROOT),
        ("NOTICE_READ_FILE_TRUNCATED", NOTICE_READ_FILE_TRUNCATED),
        (
            "NOTICE_LIST_DIRECTORY_TRUNCATED",
            NOTICE_LIST_DIRECTORY_TRUNCATED,
        ),
        ("NOTICE_GLOB_TRUNCATED", NOTICE_GLOB_TRUNCATED),
        ("NOTICE_GREP_TRUNCATED", NOTICE_GREP_TRUNCATED),
    ]
}
