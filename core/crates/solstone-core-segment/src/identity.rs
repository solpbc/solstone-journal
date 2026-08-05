// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;
use std::fs;

use crate::manifest::{ManifestRead, read_manifest};
use crate::write::descriptor;
use crate::{ContentDescriptor, ContentName, SegmentDir, SegmentError, is_reserved_name};

const MEDIA_EXTENSIONS: [&str; 9] = [
    ".flac", ".opus", ".ogg", ".m4a", ".mp3", ".wav", ".webm", ".mp4", ".mov",
];

/// Evidence supporting an identity file.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ContentIdentityEvidence {
    Present,
    TerminalProof,
}

/// One validated item in a segment content identity.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContentIdentityFile {
    pub descriptor: ContentDescriptor,
    pub evidence: ContentIdentityEvidence,
}

/// A nonempty content identity suitable for duplicate grouping.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContentIdentity {
    files: BTreeMap<ContentName, ContentIdentityFile>,
}

impl ContentIdentity {
    /// Return all identity files ordered by validated content name.
    pub fn files(&self) -> &BTreeMap<ContentName, ContentIdentityFile> {
        &self.files
    }
}

/// Validate terminal processing proof owned by the segment-sense strand.
pub trait TerminalProofVerifier {
    fn has_terminal_proof(&self, name: &ContentName, size: u64) -> bool;
}

impl<F> TerminalProofVerifier for F
where
    F: Fn(&ContentName, u64) -> bool,
{
    fn has_terminal_proof(&self, name: &ContentName, size: u64) -> bool {
        self(name, size)
    }
}

/// Load a strict, nonempty identity from a manifest or legacy media files.
pub fn load_content_identity(
    segment: &SegmentDir,
    terminal_proof: &dyn TerminalProofVerifier,
) -> Result<ContentIdentity, SegmentError> {
    match read_manifest(segment)? {
        ManifestRead::Present(files) => identity_from_manifest(segment, files, terminal_proof),
        ManifestRead::Missing => identity_from_legacy_media(segment),
    }
}

fn identity_from_manifest(
    segment: &SegmentDir,
    files: BTreeMap<ContentName, crate::manifest::ManifestEntry>,
    terminal_proof: &dyn TerminalProofVerifier,
) -> Result<ContentIdentity, SegmentError> {
    let mut identity = BTreeMap::new();
    for (name, entry) in files {
        let path = segment.path.join(name.as_str());
        let item = match fs::read(&path) {
            Ok(bytes) => {
                let descriptor = descriptor(name.clone(), &bytes);
                if descriptor.sha256 != entry.sha256 || descriptor.size != entry.size {
                    return Err(SegmentError::IdentityRefusal {
                        name: name.to_string(),
                        reason: "disk bytes do not match ingest manifest",
                    });
                }
                ContentIdentityFile {
                    descriptor,
                    evidence: ContentIdentityEvidence::Present,
                }
            }
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
                if !terminal_proof.has_terminal_proof(&name, entry.size) {
                    return Err(SegmentError::IdentityRefusal {
                        name: name.to_string(),
                        reason: "content is absent without terminal processing proof",
                    });
                }
                ContentIdentityFile {
                    descriptor: ContentDescriptor {
                        name: name.clone(),
                        sha256: entry.sha256,
                        size: entry.size,
                    },
                    evidence: ContentIdentityEvidence::TerminalProof,
                }
            }
            Err(source) => {
                return Err(SegmentError::Io { path, source });
            }
        };
        identity.insert(name, item);
    }
    nonempty_identity(identity)
}

fn identity_from_legacy_media(segment: &SegmentDir) -> Result<ContentIdentity, SegmentError> {
    let entries = match fs::read_dir(&segment.path) {
        Ok(entries) => entries,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
            return Err(SegmentError::IdentityRefusal {
                name: "ingest.json".to_owned(),
                reason: "segment has no ingest manifest or media files",
            });
        }
        Err(source) => {
            return Err(SegmentError::Io {
                path: segment.path.clone(),
                source,
            });
        }
    };
    let mut identity = BTreeMap::new();
    for entry in entries {
        let entry = entry.map_err(|source| SegmentError::Io {
            path: segment.path.clone(),
            source,
        })?;
        let path = entry.path();
        if !entry
            .file_type()
            .map_err(|source| SegmentError::Io {
                path: path.clone(),
                source,
            })?
            .is_file()
        {
            continue;
        }
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if is_reserved_name(&name) || !is_media_name(&name) {
            continue;
        }
        let name = ContentName::new(&name)?;
        let bytes = fs::read(&path).map_err(|source| SegmentError::Io {
            path: path.clone(),
            source,
        })?;
        let descriptor = descriptor(name.clone(), &bytes);
        identity.insert(
            name,
            ContentIdentityFile {
                descriptor,
                evidence: ContentIdentityEvidence::Present,
            },
        );
    }
    nonempty_identity(identity)
}

fn nonempty_identity(
    files: BTreeMap<ContentName, ContentIdentityFile>,
) -> Result<ContentIdentity, SegmentError> {
    if files.is_empty() {
        return Err(SegmentError::IdentityRefusal {
            name: "ingest.json".to_owned(),
            reason: "identity has no non-reserved content files",
        });
    }
    Ok(ContentIdentity { files })
}

fn is_media_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    MEDIA_EXTENSIONS
        .iter()
        .any(|extension| lower.ends_with(extension))
}

#[cfg(test)]
#[allow(clippy::disallowed_methods, clippy::disallowed_types)]
mod tests {
    use std::fs;
    use std::path::Path;

    use serde_json::json;

    use crate::test_support::TempDir;

    use super::*;

    fn segment(root: &Path) -> SegmentDir {
        let segment = SegmentDir::resolve(root, "20260804", "120000_60", "workstation").unwrap();
        fs::create_dir_all(&segment.path).unwrap();
        segment
    }

    fn write_manifest(segment: &SegmentDir, value: serde_json::Value) {
        fs::write(segment.path.join("ingest.json"), value.to_string()).unwrap();
    }

    fn no_proof(_: &ContentName, _: u64) -> bool {
        false
    }

    #[test]
    fn present_malformed_manifests_never_fall_back_to_media() {
        let cases = [
            "not-json".to_owned(),
            json!([]).to_string(),
            json!({"schema_version": 1, "files": []}).to_string(),
            json!({"schema_version": 1, "files": {"audio.flac": []}}).to_string(),
            json!({"schema_version": 1, "files": {"audio.flac": {"sha256": "a", "size": true}}})
                .to_string(),
            json!({"schema_version": 2, "files": {}}).to_string(),
        ];
        for raw in cases {
            let temporary = TempDir::new();
            let segment = segment(temporary.path());
            fs::write(segment.path.join("audio.flac"), b"media").unwrap();
            fs::write(segment.path.join("ingest.json"), raw).unwrap();
            let error = load_content_identity(&segment, &no_proof).unwrap_err();
            assert!(error.to_string().contains("manifest"));
        }
    }

    #[test]
    fn rejects_plain_dotdot_manifest_content_name() {
        let temporary = TempDir::new();
        let segment = segment(temporary.path());
        write_manifest(
            &segment,
            json!({"schema_version": 1, "files": {"..": {"sha256": "a", "size": 1}}}),
        );
        assert!(matches!(
            load_content_identity(&segment, &no_proof),
            Err(SegmentError::MalformedManifest { .. })
        ));
    }

    #[test]
    fn accepts_absent_manifest_content_only_with_terminal_proof() {
        let temporary = TempDir::new();
        let segment = segment(temporary.path());
        write_manifest(
            &segment,
            json!({"schema_version": 1, "files": {"audio.flac": {"sha256": "abc", "size": 4}}}),
        );
        let error = load_content_identity(&segment, &no_proof).unwrap_err();
        match error {
            SegmentError::IdentityRefusal { name, .. } => assert_eq!(name, "audio.flac"),
            other => panic!("expected identity refusal, got {other:?}"),
        }
        let proof = |name: &ContentName, size: u64| name.as_str() == "audio.flac" && size == 4;
        let identity = load_content_identity(&segment, &proof).unwrap();
        assert_eq!(identity.files().len(), 1);
        assert_eq!(
            identity.files().values().next().unwrap().evidence,
            ContentIdentityEvidence::TerminalProof
        );
    }

    #[test]
    fn present_manifest_content_mismatch_refuses_the_named_file() {
        let temporary = TempDir::new();
        let segment = segment(temporary.path());
        write_manifest(
            &segment,
            json!({"schema_version": 1, "files": {"audio.flac": {"sha256": "deadbeef", "size": 999}}}),
        );
        fs::write(segment.path.join("audio.flac"), b"real content").unwrap();

        let error = load_content_identity(&segment, &no_proof).unwrap_err();
        match error {
            SegmentError::IdentityRefusal { name, .. } => assert_eq!(name, "audio.flac"),
            other => panic!("expected identity refusal, got {other:?}"),
        }
    }

    #[test]
    fn legacy_media_scan_requires_media() {
        let temporary = TempDir::new();
        let segment = segment(temporary.path());
        fs::write(segment.path.join("notes.txt"), b"not media").unwrap();
        assert!(matches!(
            load_content_identity(&segment, &no_proof),
            Err(SegmentError::IdentityRefusal { .. })
        ));
    }

    #[test]
    fn reserved_manifest_names_are_unconditionally_excluded() {
        let temporary = TempDir::new();
        let segment = segment(temporary.path());
        write_manifest(
            &segment,
            json!({"schema_version": 1, "files": {"events.jsonl": []}}),
        );
        assert!(matches!(
            load_content_identity(&segment, &no_proof),
            Err(SegmentError::IdentityRefusal { .. })
        ));
    }
}
