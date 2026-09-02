// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use caseless::default_case_fold_str;
use unicode_normalization::{UnicodeNormalization, char::is_combining_mark};

const MAX_STREAM_NAME_BYTES: usize = 64;

/// Project a device display label and sub-stream source into a portable stream label.
pub fn project_stream_name(label: &str, source: &str) -> String {
    let mut projected =
        escape_reserved_stem(project_fragment(label).unwrap_or_else(|| "device".to_owned()));
    if let Some(source) = project_fragment(source) {
        projected.push('_');
        projected.push_str(&reserved_source_tail(&source));
    }
    projected.truncate(MAX_STREAM_NAME_BYTES);
    projected
}

/// Project a pairing-derived base and source, truncating the base first so the
/// source fragment stays intact (the opposite of `project_stream_name`).
pub(crate) fn project_paired_stream_base(base_input: &str, source: &str) -> String {
    assemble_paired_stream_name(base_input, source, MAX_STREAM_NAME_BYTES)
}

/// Fit a collision suffix by re-assembling base+source into a smaller budget,
/// still truncating the base before the source tail, then appending `_{hex}`.
pub(crate) fn paired_name_with_suffix(base_input: &str, source: &str, hex: &str) -> String {
    let suffix = format!("_{hex}");
    let budget = MAX_STREAM_NAME_BYTES.saturating_sub(suffix.len());
    format!(
        "{}{suffix}",
        assemble_paired_stream_name(base_input, source, budget)
    )
}

fn assemble_paired_stream_name(base_input: &str, source: &str, max_bytes: usize) -> String {
    let base =
        escape_reserved_stem(project_fragment(base_input).unwrap_or_else(|| "device".to_owned()));
    let Some(source_fragment) = project_fragment(source) else {
        let mut projected = base;
        projected.truncate(max_bytes);
        return projected;
    };
    let tail = reserved_source_tail(&source_fragment);
    // A 64-byte projected source is reachable: `validate_source` allows 64
    // bytes of already-canonical `[a-z0-9_-]`, which `project_fragment` keeps
    // intact. There is then no room for a base or separator.
    if tail.len() >= max_bytes {
        let mut projected = tail;
        projected.truncate(max_bytes);
        return projected;
    }
    let kept_base_len = max_bytes.saturating_sub(1 + tail.len());
    let mut kept = base;
    kept.truncate(kept_base_len);
    if !kept.is_empty() && is_windows_reserved(&kept) {
        kept.clear();
    }
    if kept.is_empty() {
        format!("_{tail}")
    } else {
        format!("{kept}_{tail}")
    }
}

fn escape_reserved_stem(mut projected: String) -> String {
    if is_windows_reserved(&projected) {
        projected.push('_');
    }
    projected
}

fn reserved_source_tail(source: &str) -> String {
    if is_windows_reserved(source) {
        format!("{source}_")
    } else {
        source.to_owned()
    }
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

    use super::{paired_name_with_suffix, project_paired_stream_base, project_stream_name};
    use crate::is_safe_stream_component;

    const PAIRED_AGREES_WITH_GENERIC: [(&str, &str); 5] = [
        ("Desk.01", "tmux"),
        ("Desk.01", ""),
        ("linux", ""),
        ("studio-mac", ""),
        ("studio-mac", "tmux"),
    ];

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
    fn paired_and_generic_agree_on_short_non_truncating_vectors() {
        let fixture: Value = serde_json::from_str(include_str!(
            "../../../fixtures/stream-name-projection-vectors.json"
        ))
        .unwrap();
        for (label, source) in PAIRED_AGREES_WITH_GENERIC {
            let vector = fixture["project"]
                .as_array()
                .unwrap()
                .iter()
                .find(|vector| {
                    vector["label"].as_str() == Some(label)
                        && vector["source"].as_str() == Some(source)
                })
                .unwrap_or_else(|| panic!("fixture contains {label:?}/{source:?}"));
            let generic = project_stream_name(label, source);
            let paired = project_paired_stream_base(label, source);
            assert_eq!(generic, vector["expect"].as_str().unwrap(), "{vector}");
            assert_eq!(paired, generic, "{label:?}/{source:?}");
            assert!(is_safe_stream_component(&paired), "{paired}");
        }
    }

    #[test]
    fn paired_projection_protects_a_max_length_source_that_generic_clips() {
        // 253-byte client_label is the pairing ceremony max; 64-byte source is
        // validate_source's max. project_fragment keeps both as ASCII.
        let base = "a".repeat(253);
        let source = "b".repeat(64);
        let paired = project_paired_stream_base(&base, &source);
        // Zero-room case: tail is already 64 bytes, so the base and separator
        // are dropped and the full source fragment is the name.
        assert_eq!(paired, source);
        assert_eq!(paired.len(), 64);
        assert!(is_safe_stream_component(&paired));
        let generic = project_stream_name(&base, &source);
        // Generic truncates the assembled string from the end, so the source
        // is clipped entirely and only the base prefix remains — the reason
        // the paired projector exists (AC3).
        assert_eq!(generic, "a".repeat(64));
        assert_ne!(paired, generic);
    }

    #[test]
    fn paired_projection_truncates_base_before_source_and_uses_a_leading_underscore() {
        // 60-byte base + 1-byte separator + 10-byte source = 71; keep 10-byte
        // source plus separator, so 53 bytes of base remain: 64 - 1 - 10 = 53.
        let base = "a".repeat(60);
        let source = "c".repeat(10);
        let paired = project_paired_stream_base(&base, &source);
        assert_eq!(paired, format!("{}_{}", "a".repeat(53), source));
        assert_eq!(paired.len(), 64);
        assert!(is_safe_stream_component(&paired));

        // 63-byte source: 64 - 1 - 63 = 0 base budget -> leading underscore + tail.
        let long_source = "d".repeat(63);
        let empty_base = project_paired_stream_base(&base, &long_source);
        assert_eq!(empty_base, format!("_{long_source}"));
        assert_eq!(empty_base.len(), 64);
        assert!(is_safe_stream_component(&empty_base));
        assert!(empty_base.starts_with('_'));
    }

    #[test]
    fn suffix_fit_shrinks_base_before_touching_source_tail() {
        // Stage-1 assembly: 50 + 1 + 4 = 55 <= 64, so no source clipping yet.
        // 16-hex suffix is 17 bytes, leaving a 47-byte budget. Old
        // `projected[..budget]` kept the 50-byte base prefix and chopped
        // `_tmux` off the end. Base-first refit keeps `tmux` and shortens
        // the base: 47 - 1 - 4 = 42.
        let base = "a".repeat(50);
        let source = "tmux";
        let hex = "0123456789abcdef";
        let projected = project_paired_stream_base(&base, source);
        assert_eq!(projected, format!("{base}_{source}"));
        assert_eq!(projected.len(), 55);
        let fitted = paired_name_with_suffix(&base, source, hex);
        assert_eq!(fitted, format!("{}_{source}_{hex}", "a".repeat(42)));
        assert_eq!(fitted.len(), 64);
        assert!(is_safe_stream_component(&fitted));
    }

    #[test]
    fn suffix_fit_eats_source_only_after_base_is_exhausted() {
        // 64-byte source already dropped the base at stage 1. An 8-hex suffix
        // (9 bytes) must then take 9 bytes from the source tail.
        let source = "b".repeat(64);
        let hex = "01234567";
        let projected = project_paired_stream_base("label", &source);
        assert_eq!(projected, source);
        let fitted = paired_name_with_suffix("label", &source, hex);
        assert_eq!(fitted, format!("{}_{hex}", "b".repeat(55)));
        assert_eq!(fitted.len(), 64);
        assert!(is_safe_stream_component(&fitted));
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
            let paired = project_paired_stream_base(input, input);
            assert!(is_safe_stream_component(&paired), "{paired}");
            assert!(paired.len() <= 64);
        }
    }
}
