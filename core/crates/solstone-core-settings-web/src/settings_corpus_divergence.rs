// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Permanent documented divergence for the frozen settings corpus after the
//! sol-initiated settings pane is removed. The corpus cannot be regenerated.

use serde_json::Value;

const REQUIRED_PROBE_KINDS: [(&str, usize); 5] = [
    ("PUT sol_voice.partial", 2),
    ("PUT sol_voice.unknown-key", 2),
    ("PUT sol_voice.not-object", 6),
    ("GET api/sol_voice", 6),
    ("GET api/sol_voice/throttled", 6),
];

/// Permanent documented divergence, introduced 2026-08-20, with no expiry
/// condition: the frozen settings corpus permanently records the deleted
/// sol-initiated settings pane. The corpus CANNOT be regenerated, so the
/// fixture is a frozen record and the divergence is absorbed here instead.
/// Narrowness is the safeguard: it is keyed to five named probe kinds and
/// two payload object keys. Never generalize this into a rule over `/api/`
/// prefixes, and never retire it.
pub fn apply_permanent_sol_initiated_settings_divergence(expected: &mut Value) {
    for (kind, count) in REQUIRED_PROBE_KINDS {
        assert_eq!(
            count_keys(expected, kind),
            count,
            "frozen settings corpus contains {count} {kind} probes"
        );
    }
    strip_probe_keys(expected);
    strip_payload_keys(expected);
    recompute_case_digests(expected);
}

/// Permanent narrow divergence introduced by the 2026-09-10 support-client
/// excision. The frozen corpus contains exactly two support mutation probes
/// and retained support objects throughout its captured config projections.
/// Remove only that named probe and object key; never generalize this rule.
pub fn apply_permanent_support_settings_divergence(expected: &mut Value) {
    assert_eq!(
        count_keys(expected, "POST support.toggle"),
        2,
        "frozen settings corpus contains exactly two support mutation probes"
    );
    strip_named_key(expected, "POST support.toggle");
    strip_named_key(expected, "support");
    recompute_case_digests(expected);
}

/// Permanent narrow divergence introduced by the 2026-10-03 journal-mark
/// naming decision. The frozen corpus contains two journal rename mutation
/// probes and five empty-journal-name refusal probes expecting "Journal name cannot be empty".
/// Because section "journal" is no longer writable, those five probes now
/// refuse with "Unknown section: journal" while the corrupt-config phase probe
/// continues to fail with 500 corrupt_config. The projected "journal" config
/// section is removed from all captured responses.
pub fn apply_permanent_journal_name_settings_divergence(expected: &mut Value) {
    assert_eq!(
        count_keys(expected, "POST journal.rename"),
        2,
        "frozen settings corpus contains exactly two journal rename mutation probes"
    );
    strip_named_key(expected, "POST journal.rename");
    assert_eq!(
        count_keys(expected, "POST config.empty-journal-name"),
        6,
        "frozen settings corpus contains exactly six empty-journal-name refusal probes"
    );
    update_empty_journal_name_probes(expected);
    strip_named_key(expected, "journal");
    recompute_case_digests(expected);
}

fn update_empty_journal_name_probes(value: &mut Value) {
    match value {
        Value::Object(map) => {
            if let Some(probe) = map.get_mut("POST config.empty-journal-name")
                && let Some(probe_map) = probe.as_object_mut()
                && probe_map.get("status") == Some(&Value::from(400))
            {
                if let Some(normalized) = probe_map.get_mut("normalized")
                    && let Some(norm_map) = normalized.as_object_mut()
                {
                    norm_map.insert(
                        "detail".to_owned(),
                        Value::String("Unknown section: journal".to_owned()),
                    );
                }
            }
            for child in map.values_mut() {
                update_empty_journal_name_probes(child);
            }
        }
        Value::Array(items) => {
            for child in items {
                update_empty_journal_name_probes(child);
            }
        }
        _ => {}
    }
}

fn count_keys(value: &Value, key: &str) -> usize {
    match value {
        Value::Object(map) => {
            usize::from(map.contains_key(key))
                + map
                    .values()
                    .map(|child| count_keys(child, key))
                    .sum::<usize>()
        }
        Value::Array(items) => items.iter().map(|child| count_keys(child, key)).sum(),
        _ => 0,
    }
}

fn strip_probe_keys(value: &mut Value) {
    match value {
        Value::Object(map) => {
            for (kind, _) in REQUIRED_PROBE_KINDS {
                map.remove(kind);
            }
            for child in map.values_mut() {
                strip_probe_keys(child);
            }
        }
        Value::Array(items) => {
            for child in items {
                strip_probe_keys(child);
            }
        }
        _ => {}
    }
}

fn strip_named_key(value: &mut Value, key: &str) {
    match value {
        Value::Object(map) => {
            map.remove(key);
            for child in map.values_mut() {
                strip_named_key(child, key);
            }
        }
        Value::Array(items) => {
            for child in items {
                strip_named_key(child, key);
            }
        }
        _ => {}
    }
}

fn strip_payload_keys(value: &mut Value) {
    match value {
        Value::Object(map) => {
            map.remove("sol_voice");
            map.remove("sol_voice_copy");
            for child in map.values_mut() {
                strip_payload_keys(child);
            }
        }
        Value::Array(items) => {
            for child in items {
                strip_payload_keys(child);
            }
        }
        _ => {}
    }
}

fn recompute_case_digests(value: &mut Value) {
    match value {
        Value::Object(map) => {
            for child in map.values_mut() {
                recompute_case_digests(child);
            }
            if map.contains_key("digest")
                && let Some(normalized) = map.get("normalized")
            {
                let digest = crate::corpus::digest(normalized);
                map.insert("digest".to_owned(), Value::String(digest));
            }
        }
        Value::Array(items) => {
            for child in items {
                recompute_case_digests(child);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::apply_permanent_sol_initiated_settings_divergence;
    use serde_json::json;

    #[test]
    fn permanent_sol_initiated_settings_divergence_requires_exact_probe_counts() {
        for (case, mut expected) in [
            ("zero", json!({})),
            (
                "extra",
                json!({
                    "PUT sol_voice.partial": {},
                    "PUT sol_voice.unknown-key": {},
                    "PUT sol_voice.not-object": {},
                    "GET api/sol_voice": {},
                    "GET api/sol_voice/throttled": {},
                    "nested": {
                        "PUT sol_voice.partial": {},
                        "PUT sol_voice.unknown-key": {},
                        "PUT sol_voice.not-object": {},
                        "GET api/sol_voice": {},
                        "GET api/sol_voice/throttled": {},
                        "again": {
                            "PUT sol_voice.not-object": {},
                            "GET api/sol_voice": {},
                            "GET api/sol_voice/throttled": {},
                            "overflow": { "PUT sol_voice.partial": {} }
                        }
                    }
                }),
            ),
        ] {
            assert!(
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    apply_permanent_sol_initiated_settings_divergence(&mut expected);
                }))
                .is_err(),
                "{case} probe counts must fail the narrow divergence"
            );
        }
    }
}
