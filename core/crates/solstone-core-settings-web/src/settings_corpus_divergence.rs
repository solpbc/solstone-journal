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

/// Permanent narrow divergence introduced by the 2026-10-10 deferred-processing
/// gate restore. The frozen corpus captured "POST processing.valid" writing the
/// default gate (02:00-06:00 window on, display sleep off) beside the mode. The
/// handler now writes only the supplied fields, and the reader supplies those
/// same defaults for an absent gate, so the effective settings are unchanged.
/// Remove only that exact default gate from that named probe's normalized
/// response and saved config; never generalize this rule.
pub fn apply_permanent_processing_default_gate_divergence(expected: &mut Value) {
    const PROBE: &str = "POST processing.valid";
    let default_gate = serde_json::json!({
        "display_powersave": {"enabled": false},
        "time_window": {"enabled": true, "end": "06:00", "start": "02:00"}
    });
    assert_eq!(
        count_keys(expected, PROBE),
        2,
        "frozen settings corpus contains exactly two processing.valid mutation probes"
    );
    for collection in ["mutations", "mutations_malformed"] {
        let probe = &mut expected[collection][PROBE];
        for pointer in ["/normalized/config/processing", "/config_after/processing"] {
            let processing = probe
                .pointer_mut(pointer)
                .and_then(Value::as_object_mut)
                .expect("captured processing section");
            assert_eq!(
                processing.remove("gate"),
                Some(default_gate.clone()),
                "{collection} {PROBE} {pointer} captured the default gate"
            );
        }
    }
    recompute_case_digests(expected);
}

/// Permanent narrow divergence introduced 2026-11-18, with no expiry
/// condition. The frozen corpus captured a custom activity added to a facet
/// that stored no activities: it stored the custom alone, and so the facet
/// stopped showing every default activity it had shown but the three always-on
/// ones. A custom add there now stores the facet's defaults first, bare and in
/// their order, then the custom, so the facet keeps showing what it showed.
/// Keyed to the one file `ac13` produces by two custom adds on a new facet. The
/// populated phase's tree and its read probes stay as frozen: a journal storing
/// customs alone is a real one, which added them before. Never generalize this.
pub fn permanent_custom_add_keeps_shown_defaults(path: &str, expected: &str) -> String {
    const PATH: &str = "facets/work-life/activities/activities.jsonl";
    if path != PATH {
        return expected.to_owned();
    }
    assert!(
        expected.starts_with("{\"id\": \"deep_work\", \"custom\": true"),
        "frozen settings corpus stores the first custom alone in {PATH}"
    );
    let defaults: Vec<Value> =
        serde_json::from_str(include_str!("../assets/default_activities.json"))
            .expect("default activities");
    let bare = defaults
        .iter()
        .map(|record| format!("{{\"id\": {}}}\n", record["id"]))
        .collect::<String>();
    format!("{bare}{expected}")
}

/// Permanent narrow divergence introduced 2026-11-21: a facet's emoji is no
/// longer guessed into a Lucide icon. The frozen corpus captured the muted
/// facet (🔇, no icon chosen) with a guessed `icon_svg` in exactly two
/// listings; a facet with no chosen icon now reads `icon_svg: null`. Facets
/// with a chosen icon keep theirs. Never generalize this rule.
pub fn apply_permanent_facet_emoji_icon_divergence(expected: &mut Value) {
    let phase = &mut expected["phases"]["populated"];
    let mut changed = 0;
    for case in ["GET api/facets/muted", "GET api/facets?all=true"] {
        let facets = phase[case]["normalized"]["facets"]
            .as_array_mut()
            .expect("captured facet listing");
        for facet in facets {
            if facet["icon"] == "" && facet["icon_svg"].is_string() {
                assert_eq!(
                    facet["emoji"], "🔇",
                    "{case} guessed only the muted facet's icon"
                );
                facet["icon_svg"] = Value::Null;
                changed += 1;
            }
        }
    }
    assert_eq!(changed, 2, "frozen settings corpus guessed two facet icons");
    recompute_case_digests(expected);
}

/// Permanent narrow divergence introduced 2026-11-21: facet listings now carry
/// each facet's `description` so Settings can show descriptions and flag
/// missing ones on multi-facet journals without N extra reads. Applies only to
/// the three populated facet listings in the frozen corpus.
pub fn apply_permanent_facet_description_divergence(expected: &mut Value) {
    let phase = &mut expected["phases"]["populated"];
    let mut changed = 0;
    for case in [
        "GET api/facets",
        "GET api/facets/muted",
        "GET api/facets?all=true",
    ] {
        let facets = phase[case]["normalized"]["facets"]
            .as_array_mut()
            .expect("captured facet listing");
        for facet in facets {
            let desc = match facet["name"].as_str() {
                Some("work-life") => "Work Life desc",
                Some("zeta-project") => "Zeta Project desc",
                Some("muted-thing") => "Muted Thing desc",
                other => panic!("{case} held unexpected corpus facet {other:?}"),
            };
            facet
                .as_object_mut()
                .expect("facet record object")
                .insert("description".to_owned(), Value::String(desc.to_owned()));
            changed += 1;
        }
    }
    assert_eq!(changed, 6, "frozen settings corpus lists six facet records");
    recompute_case_digests(expected);
}

fn update_empty_journal_name_probes(value: &mut Value) {
    match value {
        Value::Object(map) => {
            if let Some(probe) = map.get_mut("POST config.empty-journal-name")
                && let Some(probe_map) = probe.as_object_mut()
                && probe_map.get("status") == Some(&Value::from(400))
                && let Some(normalized) = probe_map.get_mut("normalized")
                && let Some(norm_map) = normalized.as_object_mut()
            {
                norm_map.insert(
                    "detail".to_owned(),
                    Value::String("Unknown section: journal".to_owned()),
                );
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
