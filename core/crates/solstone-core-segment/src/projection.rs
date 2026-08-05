// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use caseless::default_case_fold_str;
use unicode_normalization::{UnicodeNormalization, char::is_combining_mark};

const MAX_STREAM_NAME_BYTES: usize = 64;

/// Project a device display label and sub-stream source into a portable stream label.
pub fn project_stream_name(label: &str, source: &str) -> String {
    let mut projected = project_fragment(label).unwrap_or_else(|| "device".to_owned());
    if is_windows_reserved(&projected) {
        projected.push('_');
    }
    if let Some(source) = project_fragment(source) {
        if is_windows_reserved(&source) {
            projected.push('_');
            projected.push_str(&source);
            projected.push('_');
        } else {
            projected.push('_');
            projected.push_str(&source);
        }
    }
    projected.truncate(MAX_STREAM_NAME_BYTES);
    projected
}

pub(crate) fn name_with_ordinal(base: &str, ordinal: u64) -> String {
    if ordinal == 1 {
        return base.to_owned();
    }
    let suffix = format!("_{ordinal}");
    let prefix_len = MAX_STREAM_NAME_BYTES.saturating_sub(suffix.len());
    format!("{}{}", &base[..base.len().min(prefix_len)], suffix)
}

fn project_fragment(value: &str) -> Option<String> {
    let decomposed: String = value.nfkd().collect();
    let folded = default_case_fold_str(&decomposed);
    let mut output = String::new();
    let mut separator_pending = false;
    for ch in folded.chars().filter(|ch| !is_combining_mark(*ch)) {
        if ch.is_ascii_lowercase() || ch.is_ascii_digit() || matches!(ch, '_' | '-') {
            if separator_pending && !output.is_empty() {
                output.push('_');
            }
            separator_pending = false;
            output.push(ch);
        } else {
            separator_pending = true;
        }
    }
    let trimmed = output.trim_matches(|ch| matches!(ch, '_' | '-' | '.'));
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

fn is_windows_reserved(value: &str) -> bool {
    matches!(value, "con" | "prn" | "aux" | "nul")
        || (value.len() == 4
            && matches!(&value[..3], "com" | "lpt")
            && matches!(value.as_bytes()[3], b'1'..=b'9'))
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use super::project_stream_name;

    #[test]
    fn fixture_projection_vectors_are_exact() {
        let fixture: Value = serde_json::from_str(include_str!(
            "../../../fixtures/stream-name-projection-vectors.json"
        ))
        .unwrap();
        for vector in fixture["project"].as_array().unwrap() {
            let actual = project_stream_name(
                vector["label"].as_str().unwrap(),
                vector["source"].as_str().unwrap(),
            );
            assert_eq!(actual, vector["expect"].as_str().unwrap(), "{vector}");
        }
    }

    #[test]
    fn projection_is_total_and_well_formed() {
        let inputs = ["", "\0\u{fffd}", &"A?".repeat(5_120), "Владимир", "..."];
        for input in inputs {
            let projected = project_stream_name(input, input);
            assert!(!projected.is_empty());
            assert!(projected.len() <= 64);
            assert!(!projected.contains('.'));
            assert!(
                projected.as_bytes()[0].is_ascii_lowercase()
                    || projected.as_bytes()[0].is_ascii_digit()
            );
            assert!(projected.chars().all(|ch| {
                ch.is_ascii_lowercase() || ch.is_ascii_digit() || matches!(ch, '_' | '-')
            }));
        }
    }
}
