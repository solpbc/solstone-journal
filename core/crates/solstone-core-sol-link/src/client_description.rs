// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde::{Deserialize, Serialize};

#[cfg(feature = "host")]
use crate::ledger::ClientEntry;

/// Reported device metadata from client self-publication.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReportedDescription {
    pub name: Option<String>,
    pub platform: Option<String>,
    pub device_type: Option<String>,
    pub app_id: Option<String>,
    pub app_version: Option<String>,
}

/// Journal-local stored linked-device description record.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StoredClientDescription {
    pub protocol_version: u32,
    pub revision: u64,
    pub reported: Option<ReportedDescription>,
    pub owner_label: Option<String>,
    pub updated_at: Option<String>,
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
        }
    }
}

/// Journal identity metadata attached to client description responses.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JournalIdentityMeta {
    pub name: Option<String>,
    pub version: String,
}

/// HTTP response payload for GET/PUT /api/clients/self and PATCH /api/clients/{cid}/label.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClientDescriptionResponse {
    pub protocol_version: u32,
    pub revision: u64,
    pub reported: Option<ReportedDescription>,
    pub owner_label: Option<String>,
    pub display_label: String,
    pub updated_at: Option<String>,
    pub journal: JournalIdentityMeta,
}

/// HTTP request payload for PUT /api/clients/self.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PutSelfDescriptionRequest {
    pub protocol_version: u32,
    pub expected_revision: u64,
    pub reported: Option<ReportedDescription>,
}

/// HTTP request payload for PATCH /api/clients/{cid}/label.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PatchClientLabelRequest {
    #[serde(default)]
    pub label: Option<String>,
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
            if trimmed.bytes().any(|b| b < 0x20 || b == 0x7f) {
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
}
