// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The `owner_voice` member of a transcription processing record.
//!
//! An attempt that keeps only the owner's speech records what it did here: how much speech
//! the first voice-activity pass found, how much was kept, which original file it processed
//! and which saved voice scored it. Every record written by such an attempt, successful or
//! failed, also carries `owner_voice_filter: true`.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Key of the filter member inside `_solstone_processing`.
pub const OWNER_VOICE_KEY: &str = "owner_voice";
/// Key of the flag every record written by a voice-only attempt carries.
pub const OWNER_VOICE_FILTER_KEY: &str = "owner_voice_filter";
/// Current shape of the `owner_voice` member.
pub const OWNER_VOICE_VERSION: u64 = 1;

/// The original file a voice-only attempt processed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OwnerVoiceOriginal {
    pub name: String,
    pub size: u64,
    pub sha256: String,
}

/// Identity of the saved voice that scored the audio.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OwnerVoicePrint {
    pub sha256: String,
}

/// What a voice-only attempt kept and what it processed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OwnerVoiceFilter {
    pub version: u64,
    pub speech_s: f64,
    pub kept_s: f64,
    pub left_out_s: f64,
    pub windows_scored: u64,
    pub windows_kept: u64,
    pub original: OwnerVoiceOriginal,
    pub print: OwnerVoicePrint,
}

impl OwnerVoiceFilter {
    /// Serialize for insertion under [`OWNER_VOICE_KEY`].
    #[must_use]
    pub fn to_value(&self) -> Value {
        serde_json::to_value(self).expect("owner voice filter record serializes")
    }
}

/// Why a present `owner_voice` member is not valid.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OwnerVoiceRecordError(pub &'static str);

impl std::fmt::Display for OwnerVoiceRecordError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.0)
    }
}

impl std::error::Error for OwnerVoiceRecordError {}

/// Read the `owner_voice` member of a processing record.
///
/// `None` when the member is absent; an error when it is present but invalid.
#[must_use]
pub fn owner_voice(record: &Value) -> Option<Result<OwnerVoiceFilter, OwnerVoiceRecordError>> {
    let member = record.get(OWNER_VOICE_KEY)?;
    Some(validate(record, member))
}

fn validate(record: &Value, member: &Value) -> Result<OwnerVoiceFilter, OwnerVoiceRecordError> {
    let filter: OwnerVoiceFilter = serde_json::from_value(member.clone())
        .map_err(|_| OwnerVoiceRecordError("owner voice member has an invalid shape"))?;
    if filter.version != OWNER_VOICE_VERSION {
        return Err(OwnerVoiceRecordError(
            "owner voice member has an unknown version",
        ));
    }
    for duration in [filter.speech_s, filter.kept_s, filter.left_out_s] {
        if !duration.is_finite() || duration < 0.0 {
            return Err(OwnerVoiceRecordError(
                "owner voice durations must be finite and non-negative",
            ));
        }
    }
    if filter.left_out_s > filter.speech_s {
        return Err(OwnerVoiceRecordError(
            "owner voice left-out speech exceeds detected speech",
        ));
    }
    if filter.windows_kept > filter.windows_scored {
        return Err(OwnerVoiceRecordError(
            "owner voice kept windows exceed scored windows",
        ));
    }
    let name = filter.original.name.as_str();
    if name.is_empty() || name == "." || name == ".." || name.contains('/') || name.contains('\\') {
        return Err(OwnerVoiceRecordError(
            "owner voice original name must be a bare file name",
        ));
    }
    if !is_sha256_hex(&filter.original.sha256) || !is_sha256_hex(&filter.print.sha256) {
        return Err(OwnerVoiceRecordError(
            "owner voice digests must be 64 lowercase hex characters",
        ));
    }
    if record.get("input_size").and_then(Value::as_u64) != Some(filter.original.size) {
        return Err(OwnerVoiceRecordError(
            "owner voice original size differs from the record's input size",
        ));
    }
    Ok(filter)
}

fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

/// Whether a record was written by a voice-only attempt.
///
/// Fails closed: true when the filter flag is present with any value other than JSON `false`,
/// or when an `owner_voice` member is present at all, valid or not.
#[must_use]
pub fn owner_voice_filtered(record: &Value) -> bool {
    let flagged = record
        .get(OWNER_VOICE_FILTER_KEY)
        .is_some_and(|flag| flag != &Value::Bool(false));
    flagged || record.get(OWNER_VOICE_KEY).is_some()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn record() -> Value {
        json!({
            "schema": "solstone.processing.v1",
            "state": "analyzed",
            "handler": "transcribe",
            "input_size": 4_831_002,
            "owner_voice_filter": true,
            "owner_voice": {
                "version": 1,
                "speech_s": 123.4,
                "kept_s": 98.0,
                "left_out_s": 40.2,
                "windows_scored": 140,
                "windows_kept": 77,
                "original": {"name": "120000_300.m4a", "size": 4_831_002, "sha256": "a".repeat(64)},
                "print": {"sha256": "0123456789abcdef".repeat(4)},
            },
        })
    }

    #[test]
    fn a_valid_member_round_trips() {
        let record = record();
        let filter = owner_voice(&record).expect("present").expect("valid");
        assert_eq!(filter.windows_kept, 77);
        assert_eq!(filter.original.name, "120000_300.m4a");
        assert_eq!(filter.to_value(), record["owner_voice"]);
    }

    #[test]
    fn an_absent_member_reads_none() {
        let mut record = record();
        record.as_object_mut().unwrap().remove("owner_voice");
        assert!(owner_voice(&record).is_none());
    }

    #[test]
    fn each_invalid_field_is_an_error() {
        type Mutation = Box<dyn Fn(&mut Value)>;
        let cases: Vec<Mutation> = vec![
            Box::new(|r| r["owner_voice"]["original"]["name"] = json!("")),
            Box::new(|r| r["owner_voice"]["original"]["name"] = json!(".")),
            Box::new(|r| r["owner_voice"]["original"]["name"] = json!("..")),
            Box::new(|r| r["owner_voice"]["original"]["name"] = json!("day/120000.m4a")),
            Box::new(|r| r["owner_voice"]["speech_s"] = json!(-1.0)),
            Box::new(|r| r["owner_voice"]["original"]["sha256"] = json!("A".repeat(64))),
            Box::new(|r| r["owner_voice"]["original"]["sha256"] = json!("a".repeat(63))),
            Box::new(|r| {
                r["owner_voice"].as_object_mut().unwrap().remove("original");
            }),
            Box::new(|r| {
                r["owner_voice"].as_object_mut().unwrap().remove("print");
            }),
            Box::new(|r| r["owner_voice"]["print"]["sha256"] = json!("xyz")),
            Box::new(|r| r["owner_voice"]["version"] = json!(2)),
            Box::new(|r| r["owner_voice"]["left_out_s"] = json!(200.0)),
            Box::new(|r| r["owner_voice"]["windows_kept"] = json!(141)),
            Box::new(|r| r["owner_voice"]["original"]["size"] = json!(1)),
            Box::new(|r| r["owner_voice"]["extra"] = json!(1)),
        ];
        for (index, mutate) in cases.iter().enumerate() {
            let mut record = record();
            mutate(&mut record);
            assert!(
                owner_voice(&record).expect("present").is_err(),
                "case {index} should be invalid"
            );
        }
    }

    #[test]
    fn filtered_fails_closed() {
        assert!(owner_voice_filtered(&record()));
        assert!(owner_voice_filtered(&json!({"owner_voice_filter": true})));
        assert!(owner_voice_filtered(&json!({"owner_voice_filter": "yes"})));
        assert!(owner_voice_filtered(
            &json!({"owner_voice": {"broken": true}})
        ));
        assert!(owner_voice_filtered(
            &json!({"owner_voice_filter": false, "owner_voice": 1})
        ));
        assert!(!owner_voice_filtered(&json!({"owner_voice_filter": false})));
        assert!(!owner_voice_filtered(&json!({"state": "analyzed"})));
    }
}
