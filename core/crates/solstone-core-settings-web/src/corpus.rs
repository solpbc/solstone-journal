// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::Value;
use sha2::{Digest, Sha256};

const NORMALIZE: [(&str, &str); 13] = [
    ("runtime_label", "<HOST:runtime_label>"),
    ("parakeet_uses_cpp", "<HOST:parakeet_uses_cpp>"),
    ("resource.*", "<HOST:resource>"),
    ("backends.*.available", "<HOST:backend_available>"),
    ("api_keys.*", "<HOST:api_key_present>"),
    ("runtime_env.*", "<HOST:runtime_env>"),
    ("identity.timezone", "<HOST:timezone>"),
    ("dashboard_url", "<HOST:dashboard_url>"),
    ("status_text", "<HOST:status_text>"),
    ("warnings", "<VOLATILE:storage_warnings>"),
    ("key_validation.*.timestamp", "<VOLATILE:timestamp>"),
    ("entries.*.timestamp", "<VOLATILE:log_timestamp>"),
    ("category_mute_state.*", "<VOLATILE:mute_state>"),
];

#[test]
fn ac11_normalizer_path_sets_equal_the_corpus_per_case() {
    let corpus = crate::test_support::corpus();
    for phase in corpus["phases"].as_object().expect("phases").values() {
        for case in phase
            .as_object()
            .expect("phase")
            .values()
            .filter(|case| case.get("normalized_paths").is_some())
        {
            let (normalized, mut hits) =
                normalize(case["normalized"].clone(), "", "<JOURNAL_ROOT>");
            hits.sort();
            hits.dedup();
            let mut expected = case["normalized_paths"]
                .as_array()
                .expect("paths")
                .iter()
                .filter_map(Value::as_str)
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>();
            expected.sort();
            expected.dedup();
            assert_eq!(hits, expected);
            assert_eq!(
                digest(&normalized),
                case["digest"].as_str().expect("digest")
            );
        }
    }
}

fn normalize(value: Value, path: &str, root: &str) -> (Value, Vec<String>) {
    let mut hits = Vec::new();
    if let Value::String(value) = value {
        if value.contains(root) {
            hits.push(format!("{path}#journal_root"));
            return (Value::String(value.replace(root, "<JOURNAL_ROOT>")), hits);
        }
        if let Some((_, replacement)) = NORMALIZE
            .iter()
            .find(|(pattern, _)| !path.is_empty() && matches(path, pattern))
        {
            hits.push(path.to_owned());
            return (Value::String((*replacement).to_owned()), hits);
        }
        return (Value::String(value), hits);
    }
    if let Some((_, replacement)) = NORMALIZE
        .iter()
        .find(|(pattern, _)| !path.is_empty() && matches(path, pattern))
    {
        hits.push(path.to_owned());
        return (Value::String((*replacement).to_owned()), hits);
    }
    match value {
        Value::Object(values) => {
            let mut output = serde_json::Map::new();
            let mut entries = values.into_iter().collect::<Vec<_>>();
            entries.sort_by(|left, right| left.0.cmp(&right.0));
            for (key, value) in entries {
                let child = if path.is_empty() {
                    key.clone()
                } else {
                    format!("{path}.{key}")
                };
                let (value, child_hits) = normalize(value, &child, root);
                output.insert(key, value);
                hits.extend(child_hits);
            }
            (Value::Object(output), hits)
        }
        Value::Array(values) => {
            let mut output = Vec::new();
            for value in values {
                let child = if path.is_empty() {
                    "*".to_owned()
                } else {
                    format!("{path}.*")
                };
                let (value, child_hits) = normalize(value, &child, root);
                output.push(value);
                hits.extend(child_hits);
            }
            (Value::Array(output), hits)
        }
        value => (value, hits),
    }
}

fn matches(path: &str, pattern: &str) -> bool {
    let path = path.split('.').collect::<Vec<_>>();
    let pattern = pattern.split('.').collect::<Vec<_>>();
    path.len() == pattern.len()
        && path
            .iter()
            .zip(pattern)
            .all(|(part, pattern)| pattern == "*" || *part == pattern)
}

fn digest(value: &Value) -> String {
    let bytes = python_json(value);
    format!("{:x}", Sha256::digest(bytes))
        .chars()
        .take(16)
        .collect()
}

fn python_json(value: &Value) -> Vec<u8> {
    fn write(value: &Value, output: &mut String) {
        match value {
            Value::Object(values) => {
                output.push('{');
                let mut entries = values.iter().collect::<Vec<_>>();
                entries.sort_by(|left, right| left.0.cmp(right.0));
                for (index, (key, value)) in entries.into_iter().enumerate() {
                    if index > 0 {
                        output.push_str(", ");
                    }
                    output.push_str(&serde_json::to_string(key).expect("key"));
                    output.push_str(": ");
                    write(value, output);
                }
                output.push('}');
            }
            Value::Array(values) => {
                output.push('[');
                for (index, value) in values.iter().enumerate() {
                    if index > 0 {
                        output.push_str(", ");
                    }
                    write(value, output);
                }
                output.push(']');
            }
            value => output.push_str(&serde_json::to_string(value).expect("value")),
        }
    }
    let mut output = String::new();
    write(value, &mut output);
    output.into_bytes()
}
