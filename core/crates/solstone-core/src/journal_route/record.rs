// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Stable stdout records for the hidden journal-route protocol.

use std::path::Path;

use solstone_core_installation_identity::lower_hex;

pub const MAX_RECORD_BYTES: usize = 65_536;

const INSPECT_KEYS: [&str; 36] = [
    "record_version",
    "command",
    "outcome",
    "platform",
    "prefix_hex",
    "current_bin_hex",
    "current_state",
    "identity_state",
    "identity_namespace",
    "identity_id",
    "identity_generation",
    "identity_journal_token_hex",
    "tuple_state",
    "refusal",
    "journal_wrapper_state",
    "journal_wrapper_path_hex",
    "journal_wrapper_target_hex",
    "journal_wrapper_guard_namespace",
    "journal_wrapper_guard_id",
    "journal_wrapper_guard_generation",
    "journal_wrapper_guard_journal_token_hex",
    "solstone_wrapper_state",
    "solstone_wrapper_path_hex",
    "solstone_wrapper_target_hex",
    "solstone_wrapper_guard_namespace",
    "solstone_wrapper_guard_id",
    "solstone_wrapper_guard_generation",
    "solstone_wrapper_guard_journal_token_hex",
    "service_state",
    "service_path_hex",
    "service_launcher_hex",
    "service_runtime_dir_hex",
    "service_guard_namespace",
    "service_guard_id",
    "service_guard_generation",
    "service_guard_journal_token_hex",
];

const REPAIR_KEYS: [&str; 40] = [
    "record_version",
    "command",
    "outcome",
    "platform",
    "prefix_hex",
    "current_bin_hex",
    "current_state",
    "identity_state",
    "identity_namespace",
    "identity_id",
    "identity_generation",
    "identity_journal_token_hex",
    "tuple_state",
    "refusal",
    "route_lock_state",
    "repair_wrapper",
    "repair_service",
    "terminal_identity_state",
    "journal_wrapper_state",
    "journal_wrapper_path_hex",
    "journal_wrapper_target_hex",
    "journal_wrapper_guard_namespace",
    "journal_wrapper_guard_id",
    "journal_wrapper_guard_generation",
    "journal_wrapper_guard_journal_token_hex",
    "solstone_wrapper_state",
    "solstone_wrapper_path_hex",
    "solstone_wrapper_target_hex",
    "solstone_wrapper_guard_namespace",
    "solstone_wrapper_guard_id",
    "solstone_wrapper_guard_generation",
    "solstone_wrapper_guard_journal_token_hex",
    "service_state",
    "service_path_hex",
    "service_launcher_hex",
    "service_runtime_dir_hex",
    "service_guard_namespace",
    "service_guard_id",
    "service_guard_generation",
    "service_guard_journal_token_hex",
];

/// Ordered, complete inspection record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InspectRecord {
    record: OrderedRecord,
}

impl InspectRecord {
    #[must_use]
    pub fn success() -> Self {
        let mut record = Self {
            record: OrderedRecord::new(&INSPECT_KEYS),
        };
        record.set("record_version", "1");
        record.set("command", "inspect");
        record.set("outcome", "success");
        record.set("refusal", "none");
        record
    }

    #[must_use]
    pub fn refusal(reason: &str) -> Self {
        let mut record = Self::success();
        record.set("outcome", "refused");
        record.set("refusal", reason);
        record
    }

    pub fn set(&mut self, key: &'static str, value: impl Into<String>) {
        self.record.set(key, value);
    }

    pub fn set_path_hex(&mut self, key: &'static str, path: Option<&Path>) {
        self.record.set_path_hex(key, path);
    }

    #[must_use]
    pub fn get(&self, key: &str) -> Option<&str> {
        self.record.get(key)
    }

    #[must_use]
    pub fn encode(&self) -> String {
        let encoded = self.record.encode();
        if encoded.len() <= MAX_RECORD_BYTES {
            encoded
        } else {
            Self::refusal("record-too-large").record.encode()
        }
    }

    #[allow(dead_code)] // Retained as the shared decoder for the later repair command.
    pub fn decode(input: &str) -> Result<Self, &'static str> {
        OrderedRecord::decode(&INSPECT_KEYS, input).map(|record| Self { record })
    }
}

/// Ordered, complete repair record. It shares inspection's framing and codecs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RepairRecord {
    record: OrderedRecord,
}

impl RepairRecord {
    #[must_use]
    pub fn success() -> Self {
        let mut record = Self {
            record: OrderedRecord::new(&REPAIR_KEYS),
        };
        record.set("record_version", "1");
        record.set("command", "repair");
        record.set("outcome", "success");
        record.set("refusal", "none");
        record.set("route_lock_state", "not-applicable");
        record.set("repair_wrapper", "not-run");
        record.set("repair_service", "not-run");
        record.set("terminal_identity_state", "not-run");
        record
    }

    #[must_use]
    pub fn refusal(reason: &str) -> Self {
        let mut record = Self::success();
        record.set("outcome", "refused");
        record.set("refusal", reason);
        record
    }

    pub fn set(&mut self, key: &'static str, value: impl Into<String>) {
        self.record.set(key, value);
    }

    pub fn set_path_hex(&mut self, key: &'static str, path: Option<&Path>) {
        self.record.set_path_hex(key, path);
    }

    #[must_use]
    pub fn get(&self, key: &str) -> Option<&str> {
        self.record.get(key)
    }

    #[must_use]
    pub fn encode(&self) -> String {
        let encoded = self.record.encode();
        if encoded.len() <= MAX_RECORD_BYTES {
            encoded
        } else {
            Self::refusal("record-too-large").record.encode()
        }
    }

    #[allow(dead_code)]
    pub fn decode(input: &str) -> Result<Self, &'static str> {
        OrderedRecord::decode(&REPAIR_KEYS, input).map(|record| Self { record })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct OrderedRecord {
    keys: &'static [&'static str],
    values: Vec<String>,
}

impl OrderedRecord {
    fn new(keys: &'static [&'static str]) -> Self {
        Self {
            keys,
            values: vec![String::new(); keys.len()],
        }
    }

    fn set(&mut self, key: &'static str, value: impl Into<String>) {
        let value = value.into();
        assert!(
            value.is_ascii() && !value.contains(['\n', '\r', '=']),
            "route record values must be ASCII one-line key=value atoms"
        );
        let index = self
            .keys
            .iter()
            .position(|candidate| *candidate == key)
            .expect("route record key is declared");
        self.values[index] = value;
    }

    fn set_path_hex(&mut self, key: &'static str, path: Option<&Path>) {
        self.set(key, path.map_or_else(String::new, path_hex));
    }

    fn get(&self, key: &str) -> Option<&str> {
        self.keys
            .iter()
            .position(|candidate| *candidate == key)
            .map(|index| self.values[index].as_str())
    }

    fn encode(&self) -> String {
        encode_values(self.keys, &self.values)
    }

    fn decode(keys: &'static [&'static str], input: &str) -> Result<Self, &'static str> {
        if !input.is_ascii() {
            return Err("record must be ASCII");
        }
        if !input.ends_with('\n') || input.ends_with("\n\n") {
            return Err("record must have exactly one trailing newline");
        }
        let lines = input[..input.len() - 1].split('\n').collect::<Vec<_>>();
        if lines.len() != keys.len() {
            return Err("record field count differs from protocol");
        }
        let mut values = Vec::with_capacity(keys.len());
        for (line, key) in lines.into_iter().zip(keys) {
            let (found, value) = line.split_once('=').ok_or("record field has no equals")?;
            if found != *key {
                return Err("record key order differs from protocol");
            }
            values.push(value.to_owned());
        }
        Ok(Self { keys, values })
    }
}

#[cfg(unix)]
fn path_hex(path: &Path) -> String {
    use std::os::unix::ffi::OsStrExt;

    lower_hex(path.as_os_str().as_bytes())
}

#[cfg(not(unix))]
fn path_hex(path: &Path) -> String {
    lower_hex(path.as_os_str().to_string_lossy().as_bytes())
}

fn encode_values(keys: &[&str], values: &[String]) -> String {
    keys.iter()
        .zip(values)
        .map(|(key, value)| format!("{key}={value}\n"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{INSPECT_KEYS, InspectRecord, MAX_RECORD_BYTES, RepairRecord};

    #[test]
    fn full_record_round_trips_in_protocol_order() {
        let mut record = InspectRecord::success();
        for key in INSPECT_KEYS {
            record.set(key, format!("value_{key}"));
        }
        record.set("record_version", "1");
        record.set("command", "inspect");
        record.set("outcome", "success");
        record.set("refusal", "none");
        let encoded = record.encode();
        assert_eq!(InspectRecord::decode(&encoded), Ok(record));
        assert!(encoded.ends_with('\n'));
        assert!(!encoded.ends_with("\n\n"));
    }

    #[test]
    fn oversize_record_falls_back_to_a_complete_refusal() {
        let mut record = InspectRecord::success();
        record.set("prefix_hex", "a".repeat(MAX_RECORD_BYTES));
        let encoded = record.encode();
        assert!(encoded.len() <= MAX_RECORD_BYTES);
        let decoded = InspectRecord::decode(&encoded).expect("fallback must round-trip");
        assert_eq!(decoded.get("outcome"), Some("refused"));
        assert_eq!(decoded.get("refusal"), Some("record-too-large"));
        for key in INSPECT_KEYS {
            assert!(decoded.get(key).is_some(), "missing {key}");
        }
    }

    #[test]
    fn every_declared_field_is_present_even_when_optional_values_are_empty() {
        let encoded = InspectRecord::success().encode();
        let lines = encoded.lines().collect::<Vec<_>>();
        assert_eq!(lines.len(), INSPECT_KEYS.len());
        for (line, key) in lines.into_iter().zip(INSPECT_KEYS) {
            assert!(line.starts_with(&format!("{key}=")));
        }
    }

    #[test]
    fn repair_record_round_trips_with_its_completion_fields() {
        let mut record = RepairRecord::success();
        record.set("repair_wrapper", "rewritten");
        record.set("repair_service", "unchanged");
        record.set("terminal_identity_state", "matched");
        let encoded = record.encode();
        assert_eq!(RepairRecord::decode(&encoded), Ok(record));
    }
}
