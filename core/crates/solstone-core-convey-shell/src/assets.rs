// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

/// A compile-time embedded response asset.
pub struct EmbeddedAsset {
    pub path: &'static str,
    pub content_type: &'static str,
    pub bytes: &'static [u8],
}

include!(concat!(env!("OUT_DIR"), "/embedded_assets.rs"));

pub fn lookup(path: &str) -> Option<&'static EmbeddedAsset> {
    GENERATED_ASSETS
        .binary_search_by_key(&path, |asset| asset.path)
        .ok()
        .map(|index| &GENERATED_ASSETS[index])
}

pub fn speaker_copy_json() -> &'static str {
    SPEAKER_COPY_JSON
}

pub fn network_copy_json() -> &'static str {
    NETWORK_COPY_JSON
}

pub fn spl_outcome_strings_json() -> &'static str {
    SPL_OUTCOME_STRINGS_JSON
}

pub fn home_address_strings_json() -> &'static str {
    HOME_ADDRESS_STRINGS_JSON
}

pub fn not_in_new_voices_copy() -> &'static str {
    NOT_IN_NEW_VOICES_COPY
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use serde_json::Value;

    use super::*;

    fn payload(source: &str) -> serde_json::Map<String, Value> {
        serde_json::from_str::<Value>(source)
            .expect("copy payload parses")
            .as_object()
            .expect("copy payload is an object")
            .clone()
    }

    fn is_network_constant_name(name: &str) -> bool {
        let bytes = name.as_bytes();
        matches!(bytes.first(), Some(b'A'..=b'Z'))
            && bytes[1..]
                .iter()
                .all(|byte| matches!(byte, b'A'..=b'Z' | b'0'..=b'9' | b'_'))
    }

    fn is_network_dictionary_key(name: &str) -> bool {
        !name.is_empty()
            && name
                .bytes()
                .all(|byte| matches!(byte, b'a'..=b'z' | b'0'..=b'9' | b'_'))
    }

    fn check_network_dictionary_keys(value: &Value, keys_seen: &mut usize) {
        match value {
            Value::Array(values) => {
                for value in values {
                    check_network_dictionary_keys(value, keys_seen);
                }
            }
            Value::Object(values) => {
                for (key, value) in values {
                    *keys_seen += 1;
                    assert!(
                        is_network_dictionary_key(key),
                        "network dictionary key has the expected shape: {key}"
                    );
                    check_network_dictionary_keys(value, keys_seen);
                }
            }
            _ => {}
        }
    }

    fn collect_whitespace_exceptions(value: &Value, root: &str, exceptions: &mut BTreeSet<String>) {
        match value {
            Value::String(value) => {
                if value.contains("  ") || value.contains('\n') || value.contains('\t') {
                    exceptions.insert(root.to_owned());
                }
            }
            Value::Array(values) => {
                for value in values {
                    collect_whitespace_exceptions(value, root, exceptions);
                }
            }
            Value::Object(values) => {
                for value in values.values() {
                    collect_whitespace_exceptions(value, root, exceptions);
                }
            }
            _ => {}
        }
    }

    #[test]
    fn network_copy_payload_keeps_digit_bearing_step_constants() {
        let network = payload(network_copy_json());
        for name in ["STEP_1", "STEP_2", "STEP_3"] {
            assert!(
                network[name]
                    .as_str()
                    .is_some_and(|value| !value.is_empty()),
                "{name} is present and non-empty"
            );
        }
    }

    #[test]
    fn network_copy_payload_has_expected_status_sentences() {
        let network = payload(network_copy_json());
        let status_keys = network["STATUS_SENTENCES"]
            .as_object()
            .expect("status sentences is an object")
            .keys()
            .map(String::as_str)
            .collect::<BTreeSet<_>>();
        assert_eq!(
            status_keys,
            BTreeSet::from([
                "direct_online",
                "direct_online_vpn",
                "reconnecting",
                "offline",
                "lan_unreachable",
                "spl_online",
                "spl_not_enrolled",
                "spl_finishing_setup",
                "spl_offline",
                "checking",
            ])
        );
    }

    #[test]
    fn network_copy_payload_names_and_dictionary_keys_have_expected_shapes() {
        let network = payload(network_copy_json());
        for name in network.keys() {
            assert!(
                is_network_constant_name(name),
                "network constant has the expected shape: {name}"
            );
        }
        let mut dictionary_keys_seen = 0;
        for value in network.values() {
            check_network_dictionary_keys(value, &mut dictionary_keys_seen);
        }
        assert!(
            dictionary_keys_seen >= 17,
            "expected at least 17 network dictionary keys, found {dictionary_keys_seen}"
        );
    }

    #[test]
    fn copy_payloads_only_allow_the_named_whitespace_exception() {
        let network = payload(network_copy_json());
        let speakers = payload(speaker_copy_json());
        let mut network_exceptions = BTreeSet::new();
        for (name, value) in &network {
            collect_whitespace_exceptions(value, name, &mut network_exceptions);
        }
        assert_eq!(
            network_exceptions,
            BTreeSet::from(["UNPAIR_AMBIGUOUS_LABEL_COMMAND_FORMAT".to_owned()])
        );
        assert!(
            network["UNPAIR_AMBIGUOUS_LABEL_COMMAND_FORMAT"]
                .as_str()
                .expect("whitespace exception is a string")
                .contains("  ")
        );
        let mut speaker_exceptions = BTreeSet::new();
        for (name, value) in &speakers {
            collect_whitespace_exceptions(value, name, &mut speaker_exceptions);
        }
        assert!(speaker_exceptions.is_empty());
    }
}
