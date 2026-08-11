// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::{Path, PathBuf};

use jsonschema::{Draft, options};
use serde_json::Value;

use super::bundle::repo_relative;

#[derive(Debug)]
pub(crate) struct ContractIssue {
    pub(crate) path: String,
    pub(crate) message: String,
}

pub(crate) struct ValidationReport {
    pub(crate) issues: Vec<ContractIssue>,
    pub(crate) matched: usize,
}

pub(crate) fn validate_schema(schema: &Value) -> Result<(), String> {
    options()
        .with_draft(Draft::Draft202012)
        .build(schema)
        .map(|_| ())
        .map_err(|error| error.to_string())
}

pub(crate) fn validate_contract_file(
    filename: &str,
    content: &[u8],
    schema: &Value,
) -> Vec<ContractIssue> {
    let kind = schema
        .get("x-journal-contract")
        .and_then(Value::as_object)
        .and_then(|meta| meta.get("file_kind"))
        .and_then(Value::as_str)
        .unwrap_or("");
    match kind {
        "json" | "ingest_envelope" => validate_json(filename, content, schema),
        "headered_jsonl" => validate_headered_jsonl(filename, content, schema),
        _ => vec![ContractIssue {
            path: filename.into(),
            message: format!("unsupported contract file kind: {kind}"),
        }],
    }
}

fn validate_json(filename: &str, content: &[u8], schema: &Value) -> Vec<ContractIssue> {
    let value = match serde_json::from_slice::<Value>(content) {
        Ok(value) => value,
        Err(error) => {
            return vec![ContractIssue {
                path: filename.into(),
                message: format!("invalid JSON: {error}"),
            }];
        }
    };
    format_errors(filename, schema, &value)
}

fn validate_headered_jsonl(filename: &str, content: &[u8], schema: &Value) -> Vec<ContractIssue> {
    let text = match std::str::from_utf8(content) {
        Ok(text) => text,
        Err(error) => {
            return vec![ContractIssue {
                path: filename.into(),
                message: format!("invalid UTF-8: {error}"),
            }];
        }
    };
    let lines = python_splitlines(text)
        .into_iter()
        .filter(|line| !python_strip_empty(line))
        .collect::<Vec<_>>();
    if lines.is_empty() {
        return vec![ContractIssue {
            path: filename.into(),
            message: "headered JSONL requires a header line".into(),
        }];
    }
    let header = schema
        .get("$defs")
        .and_then(Value::as_object)
        .and_then(|defs| defs.get("header"));
    let record = schema
        .get("$defs")
        .and_then(Value::as_object)
        .and_then(|defs| defs.get("record"));
    let mut issues = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        let label = format!("{filename}:{}", index + 1);
        let value = match serde_json::from_str::<Value>(line) {
            Ok(value) => value,
            Err(error) => {
                issues.push(ContractIssue {
                    path: label,
                    message: format!("invalid JSON: {error}"),
                });
                continue;
            }
        };
        if !value.is_object() {
            issues.push(ContractIssue {
                path: label,
                message: "line must be a JSON object".into(),
            });
            continue;
        }
        let selected = if index == 0 { header } else { record };
        match selected {
            Some(selected) => issues.extend(format_errors(&label, selected, &value)),
            None => issues.push(ContractIssue {
                path: label,
                message: "missing schema definition".into(),
            }),
        }
    }
    issues
}

fn format_errors(path: &str, schema: &Value, value: &Value) -> Vec<ContractIssue> {
    let validator = match options().with_draft(Draft::Draft202012).build(schema) {
        Ok(validator) => validator,
        Err(error) => {
            return vec![ContractIssue {
                path: path.into(),
                message: format!("invalid JSON Schema: {error}"),
            }];
        }
    };
    validator
        .iter_errors(value)
        .map(|error| {
            let location = error.instance_path().as_str();
            ContractIssue {
                path: if location.is_empty() {
                    path.into()
                } else {
                    format!(
                        "{path}:{}",
                        location.trim_start_matches('/').replace('/', ".")
                    )
                },
                message: error.to_string(),
            }
        })
        .collect()
}

pub(crate) fn validate_journal_tree(
    root: &Path,
    bundle: &Value,
) -> Result<ValidationReport, String> {
    let chronicle = root.join("chronicle");
    if !chronicle.is_dir() {
        return Ok(ValidationReport {
            issues: Vec::new(),
            matched: 0,
        });
    }
    let mut paths = Vec::new();
    walk_four_levels(&chronicle, 0, &mut paths)?;
    paths.sort();
    let mut issues = Vec::new();
    let mut matched = 0;
    for path in paths {
        let Some(schema) = schema_for_filename(
            path.file_name()
                .and_then(|name| name.to_str())
                .unwrap_or(""),
            bundle,
        ) else {
            continue;
        };
        matched += 1;
        let bytes = fs::read(&path)
            .map_err(|error| format!("contract: cannot read {}: {error}", path.display()))?;
        issues.extend(validate_contract_file(
            &repo_relative(&path, root),
            &bytes,
            schema,
        ));
    }
    Ok(ValidationReport { issues, matched })
}

fn walk_four_levels(path: &Path, level: usize, output: &mut Vec<PathBuf>) -> Result<(), String> {
    for entry in fs::read_dir(path)
        .map_err(|error| format!("contract: cannot read {}: {error}", path.display()))?
    {
        let path = entry
            .map_err(|error| format!("contract: cannot read journal entry: {error}"))?
            .path();
        if level == 3 {
            if path.is_file() {
                output.push(path);
            }
        } else if path.is_dir() {
            walk_four_levels(&path, level + 1, output)?;
        }
    }
    Ok(())
}

fn schema_for_filename<'a>(filename: &str, bundle: &'a Value) -> Option<&'a Value> {
    let id = if filename == "stream.json" {
        "stream-json"
    } else if filename == "ingest.json" {
        "observer-ingest-json"
    } else if filename == "audio.jsonl" || filename.ends_with("_audio.jsonl") {
        "audio-jsonl"
    } else if filename == "screen.jsonl" || filename.ends_with("_screen.jsonl") {
        "screen-jsonl"
    } else if filename.starts_with("browser_") && filename.ends_with(".jsonl") {
        "browser-jsonl"
    } else {
        return None;
    };
    bundle.get("schemas")?.get(id)?.get("schema")
}

fn python_splitlines(text: &str) -> Vec<&str> {
    let mut lines = Vec::new();
    let mut start = 0;
    let mut iter = text.char_indices().peekable();
    while let Some((index, character)) = iter.next() {
        if !matches!(
            character,
            '\n' | '\r'
                | '\u{000b}'
                | '\u{000c}'
                | '\u{001c}'
                | '\u{001d}'
                | '\u{001e}'
                | '\u{0085}'
                | '\u{2028}'
                | '\u{2029}'
        ) {
            continue;
        }
        lines.push(&text[start..index]);
        if character == '\r' && iter.peek().is_some_and(|(_, next)| *next == '\n') {
            iter.next();
        }
        start = iter.peek().map_or(text.len(), |(next, _)| *next);
    }
    if start < text.len() {
        lines.push(&text[start..]);
    }
    lines
}

fn python_strip_empty(value: &str) -> bool {
    value.chars().all(is_python_whitespace)
}
fn is_python_whitespace(character: char) -> bool {
    matches!(character, '\u{0009}'..='\u{000d}' | '\u{001c}'..='\u{001f}' | '\u{0020}' | '\u{0085}' | '\u{00a0}' | '\u{1680}' | '\u{2000}'..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{202f}' | '\u{205f}' | '\u{3000}')
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::TempDir;

    fn jsonl_schema() -> Value {
        json!({"x-journal-contract": {"file_kind": "headered_jsonl"}, "$defs": {
            "header": {"type": "object", "required": ["header"]},
            "record": {"type": "object", "required": ["record"]}
        }})
    }

    #[test]
    fn headered_jsonl_matches_python_splitlines_and_post_filter_labels() {
        let schema = jsonl_schema();
        let content = " \u{00a0}\u{2028}{\"header\":true}\u{000c}{\"bad\":true}\n";
        let issues = validate_contract_file("screen.jsonl", content.as_bytes(), &schema);
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].path, "screen.jsonl:2");
        assert!(issues[0].message.contains("record"));
    }

    #[test]
    fn validates_ingest_envelope_directly() {
        let schema = json!({"x-journal-contract": {"file_kind": "ingest_envelope"}, "type": "object", "required": ["event"]});
        assert!(validate_contract_file("envelope", br#"{"event":"ok"}"#, &schema).is_empty());
        let issues = validate_contract_file("envelope", br#"{}"#, &schema);
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].path, "envelope");
    }

    #[test]
    fn filename_routing_keeps_hidden_screen_and_skips_dot_stream() {
        let bundle = json!({"schemas": {
            "screen-jsonl": {"schema": {}}, "stream-json": {"schema": {}}
        }});
        assert!(schema_for_filename(".x_screen.jsonl", &bundle).is_some());
        assert!(schema_for_filename(".stream.json", &bundle).is_none());
    }

    #[test]
    fn tree_walk_reports_only_the_matching_malformed_file() {
        let temp = TempDir::new().unwrap();
        let segment = temp.path().join("chronicle/20260811/12/34");
        std::fs::create_dir_all(&segment).unwrap();
        std::fs::write(segment.join("screen.jsonl"), b"not json\n").unwrap();
        std::fs::write(segment.join("notes.txt"), b"ignored").unwrap();
        let bundle = json!({"schemas": {"screen-jsonl": {"schema": jsonl_schema()}}});

        let report = validate_journal_tree(temp.path(), &bundle).unwrap();

        assert_eq!(report.matched, 1);
        assert_eq!(report.issues.len(), 1);
        assert_eq!(
            report.issues[0].path,
            "chronicle/20260811/12/34/screen.jsonl:1"
        );
        assert!(report.issues[0].message.contains("invalid JSON"));
    }
}
