// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde::Serialize;
use serde_json::{Value, value::RawValue};
use solstone_core_journal_io::{AtomicWriteOptions, write_bytes_exclusive};

use crate::{SegmentDir, SegmentError};

/// The producer classification supplied by authenticated ingest.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub enum Kind {
    Observed,
    Browser,
    Imported(ImportSource),
    Unknown,
}

impl Kind {
    /// Whether this producer is an import.
    pub fn is_imported(&self) -> bool {
        matches!(self, Self::Imported(_))
    }

    /// The import source, if this producer is an import.
    pub fn import_source(&self) -> Option<&ImportSource> {
        match self {
            Self::Imported(source) => Some(source),
            Self::Observed | Self::Browser | Self::Unknown => None,
        }
    }

    /// Whether this producer is one of the supported AI-chat imports.
    pub fn is_ai_chat(&self) -> bool {
        matches!(self, Self::Imported(ImportSource::AiChat(_)))
    }

    /// Whether this producer is a browser stream.
    pub fn is_browser(&self) -> bool {
        matches!(self, Self::Browser)
    }

    pub(crate) fn compat_label(&self) -> &'static str {
        match self {
            Self::Observed => "observer",
            Self::Browser => "browser",
            Self::Imported(_) => "import",
            Self::Unknown => "unknown",
        }
    }
}

/// The source category for an imported stream.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub enum ImportSource {
    AiChat(AiChatSource),
    Named(String),
}

/// The explicitly recognized AI-chat import sources.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub enum AiChatSource {
    ChatGpt,
    Claude,
    Gemini,
}

/// Journal-authored sidecar input. Device metadata remains verbatim JSON.
pub struct DeviceSidecarInput<'a> {
    pub cid: Option<&'a Value>,
    pub jid: Option<&'a Value>,
    pub kind: Kind,
    pub device: &'a RawValue,
}

/// Write the journal-authored device sidecar without parsing opaque metadata.
pub fn write_device(
    segment: &SegmentDir,
    input: &DeviceSidecarInput<'_>,
) -> Result<(), SegmentError> {
    let cid = validate_device_cid_value(input.cid)?;
    let jid = validate_device_jid_value(input.jid)?;
    let document = DeviceDocument {
        cid,
        jid,
        kind: &input.kind,
        device: input.device,
    };
    let path = segment.path.join("device.json");
    let bytes = serde_json::to_vec(&document).map_err(|source| SegmentError::Serialization {
        path: path.clone(),
        source,
    })?;
    write_bytes_exclusive(path, &bytes, AtomicWriteOptions::default())?;
    Ok(())
}

pub(crate) fn validate_cid(cid: &str) -> Result<(), SegmentError> {
    let Some(hex) = cid.strip_prefix("sha256:") else {
        return Err(SegmentError::InvalidDeviceCid("missing sha256: prefix"));
    };
    if hex.len() != 64
        || !hex
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        return Err(SegmentError::InvalidDeviceCid(
            "must have 64 lowercase hexadecimal digits",
        ));
    }
    Ok(())
}

/// Return whether a device identifier has the supported canonical form.
pub fn is_valid_device_cid(cid: &str) -> bool {
    validate_cid(cid).is_ok()
}

fn validate_device_cid_value(cid: Option<&Value>) -> Result<&str, SegmentError> {
    let cid = match cid {
        None => return Err(SegmentError::InvalidDeviceCid("missing")),
        Some(Value::String(cid)) => cid.as_str(),
        Some(_) => return Err(SegmentError::InvalidDeviceCid("must be a string")),
    };
    validate_cid(cid)?;
    Ok(cid)
}

fn validate_device_jid_value(jid: Option<&Value>) -> Result<Option<&str>, SegmentError> {
    match jid {
        None => Ok(None),
        Some(Value::String(jid)) if !jid.is_empty() => Ok(Some(jid)),
        Some(Value::String(_)) => Err(SegmentError::InvalidDeviceJid("must not be empty")),
        Some(_) => Err(SegmentError::InvalidDeviceJid("must be a string")),
    }
}

#[derive(Serialize)]
struct DeviceDocument<'a> {
    #[serde(rename = "cid")]
    cid: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    jid: Option<&'a str>,
    kind: &'a Kind,
    device: &'a RawValue,
}

#[cfg(test)]
#[allow(clippy::disallowed_methods, clippy::disallowed_types)]
mod tests {
    use std::fs;
    use std::path::Path;

    use serde_json::json;

    use crate::test_support::TempDir;

    use super::*;

    const CID: &str = "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn segment(root: &Path) -> SegmentDir {
        SegmentDir::resolve(root, "20260804", "120000_60", "workstation").unwrap()
    }

    fn raw_device() -> Box<RawValue> {
        RawValue::from_string(r#"{"battery": 1, "battery": 2, "posture": "flat"}"#.to_owned())
            .unwrap()
    }

    #[test]
    fn malformed_or_missing_device_cid_is_refused() {
        let cases = [
            None,
            Some(json!("")),
            Some(json!(123)),
            Some(json!(
                "sha256:0123456789ABCDEF0123456789ABCDEF0123456789ABCDEF0123456789ABCDEF"
            )),
            Some(json!(
                "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
            )),
        ];
        for cid in cases {
            let temporary = TempDir::new();
            let raw = raw_device();
            let input = DeviceSidecarInput {
                cid: cid.as_ref(),
                jid: None,
                kind: Kind::Observed,
                device: &raw,
            };
            assert!(matches!(
                write_device(&segment(temporary.path()), &input),
                Err(SegmentError::InvalidDeviceCid(_))
            ));
        }
    }

    #[test]
    fn opaque_device_metadata_preserves_duplicate_keys_verbatim() {
        let temporary = TempDir::new();
        let raw = raw_device();
        let cid = json!(CID);
        let input = DeviceSidecarInput {
            cid: Some(&cid),
            jid: None,
            kind: Kind::Imported(ImportSource::AiChat(AiChatSource::ChatGpt)),
            device: &raw,
        };
        let segment = segment(temporary.path());
        write_device(&segment, &input).unwrap();
        let bytes = fs::read(segment.path.join("device.json")).unwrap();
        assert!(
            bytes
                .windows(br#""device":{"battery": 1, "battery": 2, "posture": "flat"}"#.len())
                .any(|window| {
                    window == br#""device":{"battery": 1, "battery": 2, "posture": "flat"}"#
                })
        );
        let parsed: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(parsed["cid"], CID);
    }

    #[test]
    fn device_write_failure_is_returned() {
        let temporary = TempDir::new();
        let segment = segment(temporary.path());
        let blocked = temporary.path().join("chronicle/20260804/workstation");
        fs::create_dir_all(blocked.parent().unwrap()).unwrap();
        fs::write(&blocked, b"not a directory").unwrap();
        let raw = raw_device();
        let cid = json!(CID);
        let input = DeviceSidecarInput {
            cid: Some(&cid),
            jid: None,
            kind: Kind::Observed,
            device: &raw,
        };
        assert!(write_device(&segment, &input).is_err());
        assert!(!segment.path.join("device.json").exists());
    }

    #[test]
    fn kind_answers_provenance_without_stream_name_parsing() {
        let imported = Kind::Imported(ImportSource::AiChat(AiChatSource::Claude));
        assert!(imported.is_imported());
        assert_eq!(
            imported.import_source(),
            Some(&ImportSource::AiChat(AiChatSource::Claude))
        );
        assert!(imported.is_ai_chat());
        assert!(!imported.is_browser());
        assert!(Kind::Browser.is_browser());
        assert!(!Kind::Browser.is_imported());
    }
}
