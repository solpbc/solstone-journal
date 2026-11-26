// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Data types and validation for indexed audio archive placement records.

use std::fmt;
use std::path::Path;

use serde::{Deserialize, Serialize};

/// Version for the `placement.json` record written to each published tile.
pub const AUDIO_ARCHIVE_PLACEMENT_VERSION: u32 = 1;

/// The filename written inside each published audio archive chronicle segment.
pub const AUDIO_ARCHIVE_PLACEMENT_FILENAME: &str = "placement.json";

/// Placement descriptor for all parts published into a single tile.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct AudioArchivePlacement {
    pub version: u32,
    pub parts: Vec<AudioArchivePlacementPart>,
}

/// A single member's placed part within a tile.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioArchivePlacementPart {
    #[serde(deserialize_with = "deserialize_stored_name")]
    pub stored_name: String,
    pub original_occurrence: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_time_ms: Option<i64>,
    pub origin_ms: i64,
    pub offset_ms: i64,
    pub length_ms: i64,
}

/// Canonical archive part stem, independent of the original container suffix.
pub fn is_audio_archive_part_stem(stem: &str) -> bool {
    let bytes = stem.as_bytes();
    bytes.len() == 22
        && bytes[0] == b'p'
        && bytes[5] == b'_'
        && bytes[1..5].iter().all(u8::is_ascii_digit)
        && bytes[6..].iter().all(u8::is_ascii_hexdigit)
}

/// Audio sidecar produced by the journal's native archive materializer.
/// Arbitrary JSONL files are not transcript inputs.
pub fn is_audio_archive_transcript_name(name: &str) -> bool {
    name.strip_suffix(".jsonl")
        .is_some_and(is_audio_archive_part_stem)
}

fn deserialize_stored_name<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<String, D::Error> {
    let name = String::deserialize(deserializer)?;
    validate_stored_name(&name).map_err(serde::de::Error::custom)?;
    Ok(name)
}

impl<'de> Deserialize<'de> for AudioArchivePlacement {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Wire {
            version: u32,
            parts: Vec<AudioArchivePlacementPart>,
        }
        let wire = Wire::deserialize(deserializer)?;
        if wire.version != AUDIO_ARCHIVE_PLACEMENT_VERSION {
            return Err(serde::de::Error::custom(
                "unsupported audio archive placement version",
            ));
        }
        let mut names = std::collections::HashSet::new();
        for part in &wire.parts {
            if !names.insert(&part.stored_name)
                || part.origin_ms < 0
                || part.offset_ms < 0
                || part.length_ms <= 0
                || part
                    .offset_ms
                    .checked_add(part.length_ms)
                    .is_none_or(|end| end > 300_000)
            {
                return Err(serde::de::Error::custom(
                    "invalid or duplicate audio archive part",
                ));
            }
        }
        Ok(Self {
            version: wire.version,
            parts: wire.parts,
        })
    }
}

/// Errors occurring while constructing or validating stored part names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StoredNameError {
    Empty,
    Absolute,
    ContainsSeparators,
    ParentTraversal,
    InvalidShape(String),
}

impl fmt::Display for StoredNameError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => formatter.write_str("stored name cannot be empty"),
            Self::Absolute => formatter.write_str("stored name cannot be an absolute path"),
            Self::ContainsSeparators => {
                formatter.write_str("stored name cannot contain path separators")
            }
            Self::ParentTraversal => formatter.write_str("stored name cannot contain '..'"),
            Self::InvalidShape(name) => {
                write!(
                    formatter,
                    "stored name does not match expected shape: {name}"
                )
            }
        }
    }
}

impl std::error::Error for StoredNameError {}

/// Construct the canonical stored raw name for a part of an audio archive member:
/// `p{part:04}_{sha256[0..16]}{optional .ext}`.
///
/// An extension is included only when the original relative name's final suffix matches
/// `^[A-Za-z0-9]{1,8}$`.
pub fn build_stored_name(
    part_index: u32,
    sha256: &str,
    original_relative_name: &str,
) -> Result<String, StoredNameError> {
    if sha256.len() < 16 || !sha256.bytes().take(16).all(|b| b.is_ascii_hexdigit()) {
        return Err(StoredNameError::InvalidShape(sha256.to_owned()));
    }
    let hash_prefix = &sha256[..16].to_ascii_lowercase();

    let extension = extract_safe_extension(original_relative_name);
    let name = match extension {
        Some(ext) => format!("p{part_index:04}_{hash_prefix}.{ext}"),
        None => format!("p{part_index:04}_{hash_prefix}"),
    };
    validate_stored_name(&name)?;
    Ok(name)
}

/// Validate that a stored name is a single safe component matching the placement contract.
pub fn validate_stored_name(name: &str) -> Result<(), StoredNameError> {
    if name.is_empty() {
        return Err(StoredNameError::Empty);
    }
    if name.starts_with('/') || name.starts_with('\\') || Path::new(name).is_absolute() {
        return Err(StoredNameError::Absolute);
    }
    if name.contains('/') || name.contains('\\') {
        return Err(StoredNameError::ContainsSeparators);
    }
    if name.contains("..") {
        return Err(StoredNameError::ParentTraversal);
    }

    let bytes = name.as_bytes();
    if bytes.len() < 22 || bytes[0] != b'p' || bytes[5] != b'_' {
        return Err(StoredNameError::InvalidShape(name.to_owned()));
    }
    if !bytes[1..5].iter().all(|b| b.is_ascii_digit()) {
        return Err(StoredNameError::InvalidShape(name.to_owned()));
    }
    if !bytes[6..22].iter().all(|b| b.is_ascii_hexdigit()) {
        return Err(StoredNameError::InvalidShape(name.to_owned()));
    }

    if bytes.len() > 22 {
        if bytes[22] != b'.' {
            return Err(StoredNameError::InvalidShape(name.to_owned()));
        }
        let ext = &name[23..];
        if ext.is_empty() || ext.len() > 8 || !ext.bytes().all(|b| b.is_ascii_alphanumeric()) {
            return Err(StoredNameError::InvalidShape(name.to_owned()));
        }
    }

    Ok(())
}

fn extract_safe_extension(original_relative_name: &str) -> Option<&str> {
    let filename = Path::new(original_relative_name).file_name()?.to_str()?;
    let (_stem, ext) = filename.rsplit_once('.')?;
    if !ext.is_empty() && ext.len() <= 8 && ext.bytes().all(|b| b.is_ascii_alphanumeric()) {
        Some(ext)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn untrusted_placement_rejects_unsafe_targets_and_out_of_tile_offsets() {
        let part = serde_json::json!({"stored_name":"../outside.ogg","original_occurrence":"utc|1","origin_ms":0,"offset_ms":0,"length_ms":1000});
        assert!(
            serde_json::from_value::<AudioArchivePlacement>(
                serde_json::json!({"version":1,"parts":[part]})
            )
            .is_err()
        );
        let part = serde_json::json!({"stored_name":"p0000_0123456789abcdef.ogg","original_occurrence":"utc|1","origin_ms":0,"offset_ms":299_500,"length_ms":1000});
        assert!(
            serde_json::from_value::<AudioArchivePlacement>(
                serde_json::json!({"version":1,"parts":[part]})
            )
            .is_err()
        );
    }

    #[test]
    fn valid_stored_names_and_extension_rules() {
        let name = build_stored_name(0, "0123456789abcdef0123456789abcdef", "clip.ogg").unwrap();
        assert_eq!(name, "p0000_0123456789abcdef.ogg");
        assert!(validate_stored_name(&name).is_ok());

        // Part index formatting
        let name_p1 =
            build_stored_name(1, "0123456789abcdef0123456789abcdef", "nested/dir/clip.ogg")
                .unwrap();
        assert_eq!(name_p1, "p0001_0123456789abcdef.ogg");

        // Two members named clip.ogg with different hashes have different names
        let name2 =
            build_stored_name(0, "fedcba98765432100123456789abcdef", "other/clip.ogg").unwrap();
        assert_eq!(name2, "p0000_fedcba9876543210.ogg");
        assert_ne!(name, name2);

        // Extension > 8 chars is omitted
        let long_ext =
            build_stored_name(0, "0123456789abcdef0123456789abcdef", "clip.audioformat").unwrap();
        assert_eq!(long_ext, "p0000_0123456789abcdef");

        // Non-alphanumeric extension is omitted
        let bad_ext =
            build_stored_name(0, "0123456789abcdef0123456789abcdef", "clip.og-g").unwrap();
        assert_eq!(bad_ext, "p0000_0123456789abcdef");

        // No extension
        let no_ext = build_stored_name(0, "0123456789abcdef0123456789abcdef", "clip").unwrap();
        assert_eq!(no_ext, "p0000_0123456789abcdef");
    }

    #[test]
    fn invalid_stored_names_are_refused() {
        assert_eq!(validate_stored_name(""), Err(StoredNameError::Empty));
        assert_eq!(
            validate_stored_name("/p0000_0123456789abcdef.ogg"),
            Err(StoredNameError::Absolute)
        );
        assert_eq!(
            validate_stored_name("a/p0000_0123456789abcdef.ogg"),
            Err(StoredNameError::ContainsSeparators)
        );
        assert_eq!(
            validate_stored_name(".."),
            Err(StoredNameError::ParentTraversal)
        );
        assert_eq!(
            validate_stored_name("p0000_0123456789abcdef/.."),
            Err(StoredNameError::ContainsSeparators)
        );
        assert!(matches!(
            validate_stored_name("wrong_prefix_0123456789abcdef"),
            Err(StoredNameError::InvalidShape(_))
        ));
    }

    #[test]
    fn placement_reads_past_fields_from_a_later_version() {
        let placement: AudioArchivePlacement = serde_json::from_value(serde_json::json!({
            "version": AUDIO_ARCHIVE_PLACEMENT_VERSION,
            "later_placement_field": true,
            "parts": [{
                "stored_name": "p0000_0123456789abcdef.ogg",
                "original_occurrence": "utc|1",
                "origin_ms": 0,
                "offset_ms": 500,
                "length_ms": 7500,
                "later_part_field": {"kept": false},
            }],
        }))
        .unwrap();
        assert_eq!(placement.parts.len(), 1);
        assert_eq!(placement.parts[0].stored_name, "p0000_0123456789abcdef.ogg");
        assert_eq!(
            (placement.parts[0].offset_ms, placement.parts[0].length_ms),
            (500, 7500)
        );
    }

    #[test]
    fn placement_json_serialization_round_trip() {
        let placement = AudioArchivePlacement {
            version: AUDIO_ARCHIVE_PLACEMENT_VERSION,
            parts: vec![
                AudioArchivePlacementPart {
                    stored_name: "p0000_0123456789abcdef.ogg".to_owned(),
                    original_occurrence: "utc|1768424400500".to_owned(),
                    original_time_ms: Some(1768424400500),
                    origin_ms: 0,
                    offset_ms: 500,
                    length_ms: 7500,
                },
                AudioArchivePlacementPart {
                    stored_name: "p0001_0123456789abcdef.ogg".to_owned(),
                    original_occurrence: "local|America/New_York|2026-11-01T01:30:00.000"
                        .to_owned(),
                    original_time_ms: None,
                    origin_ms: 149750,
                    offset_ms: 0,
                    length_ms: 250250,
                },
            ],
        };

        let json = serde_json::to_string(&placement).unwrap();
        assert!(!json.contains("original_time_ms\":null"));

        let deserialized: AudioArchivePlacement = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, placement);
    }
    #[test]
    fn archive_transcript_names_are_strict_and_raw_speech_stays_unindexed() {
        use crate::content::{ContentResolution, RawPerceptFamily, classify};
        let name = "p0001_0123456789abcdef.jsonl";
        assert!(is_audio_archive_transcript_name(name));
        for bad in [
            "p001_0123456789abcdef.jsonl",
            "p0001_0123456789abcdeg.jsonl",
            "p0001_0123456789abcdef.ogg.jsonl",
            "../p0001_0123456789abcdef.jsonl",
            "p0001_0123456789abcdef.json",
        ] {
            assert!(!is_audio_archive_transcript_name(bad));
        }
        assert_eq!(
            classify(&format!("20260101/import.archive/120000_300/{name}")),
            ContentResolution::Unindexed(RawPerceptFamily::Audio)
        );
        for prefix in [
            "facets/import.archive/120000_300",
            "20260101/import.archive/bad",
            "20260101/other/120000_300",
        ] {
            assert_eq!(
                classify(&format!("{prefix}/{name}")),
                ContentResolution::Unrecognized
            );
        }
    }
}
