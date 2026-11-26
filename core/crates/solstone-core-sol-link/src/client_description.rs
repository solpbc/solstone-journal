// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Linked-device description messages and the record kept for each device.
//!
//! A paired app, or the journal on the other end, updates on its own schedule
//! and may be newer than this one. Every type here reads past a field it does
//! not know instead of refusing the whole message; the fields it does know
//! are checked exactly as before. A message field this version does not know
//! is not stored. A stored record's own unknown fields were written by a later
//! version of this journal and are written back as read.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[cfg(feature = "host")]
use crate::ledger::ClientEntry;

/// Reported device metadata from client self-publication.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct ReportedDescription {
    #[serde(deserialize_with = "required_nullable_string")]
    pub name: Option<String>,
    #[serde(deserialize_with = "required_nullable_string")]
    pub platform: Option<String>,
    #[serde(deserialize_with = "required_nullable_string")]
    pub device_type: Option<String>,
    #[serde(deserialize_with = "required_nullable_string")]
    pub app_id: Option<String>,
    #[serde(deserialize_with = "required_nullable_string")]
    pub app_version: Option<String>,
}

/// Journal-local stored linked-device description record.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct StoredClientDescription {
    pub protocol_version: u32,
    pub revision: u64,
    #[serde(deserialize_with = "required_nullable_reported")]
    pub reported: Option<ReportedDescription>,
    #[serde(deserialize_with = "required_nullable_string")]
    pub owner_label: Option<String>,
    #[serde(deserialize_with = "required_nullable_string")]
    pub updated_at: Option<String>,
    /// Fields a later version wrote. Never interpreted; written back as read.
    #[serde(flatten)]
    pub(crate) extra: BTreeMap<String, serde_json::Value>,
}

impl StoredClientDescription {
    #[must_use]
    pub fn initial() -> Self {
        Self {
            protocol_version: 1,
            revision: 0,
            reported: None,
            owner_label: None,
            updated_at: None,
            extra: BTreeMap::new(),
        }
    }
}

/// Journal identity metadata attached to client description responses.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct JournalIdentityMeta {
    #[serde(deserialize_with = "required_nullable_string")]
    pub name: Option<String>,
    pub version: String,
}

/// HTTP response payload for GET/PUT /api/clients/self and PATCH /api/clients/{cid}/label.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ClientDescriptionResponse {
    pub protocol_version: u32,
    pub revision: u64,
    #[serde(deserialize_with = "required_nullable_reported")]
    pub reported: Option<ReportedDescription>,
    #[serde(deserialize_with = "required_nullable_string")]
    pub owner_label: Option<String>,
    pub display_label: String,
    #[serde(deserialize_with = "required_nullable_string")]
    pub updated_at: Option<String>,
    pub journal: JournalIdentityMeta,
}

/// HTTP request payload for PUT /api/clients/self.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PutSelfDescriptionRequest {
    pub protocol_version: u32,
    pub expected_revision: u64,
    #[serde(deserialize_with = "required_reported")]
    pub reported: Option<ReportedDescription>,
}

/// HTTP request payload for PATCH /api/clients/{cid}/label.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct PatchClientLabelRequest {
    #[serde(deserialize_with = "required_nullable_string")]
    pub label: Option<String>,
}

fn required_nullable_string<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<String>, D::Error> {
    Option::<String>::deserialize(deserializer)
}

fn required_nullable_reported<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<ReportedDescription>, D::Error> {
    Option::<ReportedDescription>::deserialize(deserializer)
}

fn required_reported<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<ReportedDescription>, D::Error> {
    ReportedDescription::deserialize(deserializer).map(Some)
}

/// Validate and sanitize a descriptive string field.
pub fn sanitize_string(
    value: Option<String>,
    max_bytes: usize,
) -> Result<Option<String>, &'static str> {
    match value {
        None => Ok(None),
        Some(s) => {
            let trimmed = s.trim();
            if trimmed.is_empty() {
                return Ok(None);
            }
            if trimmed.chars().any(char::is_control) {
                return Err("string contains control characters");
            }
            if trimmed.len() > max_bytes {
                return Err("string exceeds maximum byte limit");
            }
            Ok(Some(trimmed.to_owned()))
        }
    }
}

/// Sanitize all fields in a ReportedDescription.
pub fn sanitize_reported(
    reported: ReportedDescription,
) -> Result<ReportedDescription, &'static str> {
    Ok(ReportedDescription {
        name: sanitize_string(reported.name, 80)?,
        platform: sanitize_string(reported.platform, 64)?,
        device_type: sanitize_string(reported.device_type, 64)?,
        app_id: sanitize_string(reported.app_id, 64)?,
        app_version: sanitize_string(reported.app_version, 64)?,
    })
}

/// Compute the effective display label following precedence:
/// 1. owner_label if present and non-empty (no ordinal)
/// 2. reported.name if present and non-empty (no ordinal)
/// 3. ClientEntry pairing fallback (including ordinal)
#[cfg(feature = "host")]
#[must_use]
pub fn current_display_label(
    entry: &ClientEntry,
    stored: Option<&StoredClientDescription>,
) -> String {
    if let Some(owner) = stored
        .and_then(|s| s.owner_label.as_deref())
        .filter(|s| !s.is_empty())
    {
        return owner.to_owned();
    }
    if let Some(reported_name) = stored
        .and_then(|s| s.reported.as_ref())
        .and_then(|r| r.name.as_deref())
        .filter(|s| !s.is_empty())
    {
        return reported_name.to_owned();
    }
    entry.display_label()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn string_sanitization_rules() {
        assert_eq!(sanitize_string(None, 80).unwrap(), None);
        assert_eq!(sanitize_string(Some("".into()), 80).unwrap(), None);
        assert_eq!(sanitize_string(Some("   ".into()), 80).unwrap(), None);
        assert_eq!(
            sanitize_string(Some("  MacBook Pro  ".into()), 80).unwrap(),
            Some("MacBook Pro".into())
        );
        assert!(sanitize_string(Some("bad\nstring".into()), 80).is_err());
        assert!(sanitize_string(Some("bad\x1fstring".into()), 80).is_err());
        assert!(sanitize_string(Some("a".repeat(81)), 80).is_err());
        assert_eq!(
            sanitize_string(Some("a".repeat(80)), 80).unwrap(),
            Some("a".repeat(80))
        );
    }

    #[test]
    fn reported_sanitization_rules() {
        let raw = ReportedDescription {
            name: Some("  My Phone  ".into()),
            platform: Some(" iOS ".into()),
            device_type: Some("phone".into()),
            app_id: Some("solstone".into()),
            app_version: Some("1.0.0".into()),
        };
        let sanitized = sanitize_reported(raw).unwrap();
        assert_eq!(sanitized.name.as_deref(), Some("My Phone"));
        assert_eq!(sanitized.platform.as_deref(), Some("iOS"));
        assert_eq!(sanitized.device_type.as_deref(), Some("phone"));
        assert_eq!(sanitized.app_id.as_deref(), Some("solstone"));
        assert_eq!(sanitized.app_version.as_deref(), Some("1.0.0"));
    }

    #[test]
    fn messages_from_a_newer_peer_are_read_past_their_unknown_fields() {
        let put: PutSelfDescriptionRequest = serde_json::from_str(
            r#"{"protocol_version":1,"expected_revision":3,"later":true,
                "reported":{"name":"Phone","platform":"ios","device_type":null,
                            "app_id":null,"app_version":"9.0","later":{"a":1}}}"#,
        )
        .unwrap();
        assert_eq!((put.protocol_version, put.expected_revision), (1, 3));
        let reported = put.reported.unwrap();
        assert_eq!(reported.name.as_deref(), Some("Phone"));
        assert_eq!(reported.platform.as_deref(), Some("ios"));
        assert_eq!(reported.app_version.as_deref(), Some("9.0"));
        // What is written carries only the fields this version knows.
        assert!(
            serde_json::to_value(&reported)
                .unwrap()
                .get("later")
                .is_none()
        );

        let patch: PatchClientLabelRequest =
            serde_json::from_str(r#"{"label":"Desk","later":1}"#).unwrap();
        assert_eq!(patch.label.as_deref(), Some("Desk"));

        let response: ClientDescriptionResponse = serde_json::from_str(
            r#"{"protocol_version":1,"revision":2,"reported":null,"owner_label":"Desk",
                "display_label":"Desk","updated_at":null,"later":[1],
                "journal":{"name":null,"version":"1.2.3","later":"x"}}"#,
        )
        .unwrap();
        assert_eq!(response.revision, 2);
        assert_eq!(response.owner_label.as_deref(), Some("Desk"));
        assert_eq!(response.journal.version, "1.2.3");

        // A known field that is missing or of the wrong type still refuses.
        for refused in [
            r#"{"protocol_version":1,"expected_revision":0,"later":true}"#,
            r#"{"protocol_version":1,"expected_revision":0,"reported":null}"#,
            r#"{"protocol_version":"1","expected_revision":0,"reported":{"name":null,"platform":null,"device_type":null,"app_id":null,"app_version":null}}"#,
            r#"{"protocol_version":1,"expected_revision":0,"reported":{"name":null,"later":1}}"#,
        ] {
            assert!(
                serde_json::from_str::<PutSelfDescriptionRequest>(refused).is_err(),
                "{refused}"
            );
        }
        assert!(serde_json::from_str::<PatchClientLabelRequest>(r#"{"later":1}"#).is_err());
    }
}
