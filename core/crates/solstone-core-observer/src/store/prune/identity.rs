// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::Path;

use solstone_core_ingest_resolve::SegmentTerminalProof;
use solstone_core_segment::{
    ContentIdentity, ContentIdentityEvidence, RESERVED_SEGMENT_FILENAMES, SegmentDir, SegmentError,
    load_content_identity,
};

use super::marker::read_segment_marker;
use super::types::{Refusal, SegmentAnalysis};

const MEDIA_EXTENSIONS: [&str; 9] = [
    ".flac", ".opus", ".ogg", ".m4a", ".mp3", ".wav", ".webm", ".mp4", ".mov",
];

fn is_media_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    MEDIA_EXTENSIONS.iter().any(|ext| lower.ends_with(ext))
}

/// Ordered `(name, sha256, size)` triples, the equality key for byte-identical
/// duplicate grouping. `ContentIdentity` already orders files by name.
pub fn identity_key(identity: &ContentIdentity) -> Vec<(String, String, u64)> {
    identity
        .files()
        .values()
        .map(|file| {
            (
                file.descriptor.name.as_str().to_owned(),
                file.descriptor.sha256.clone(),
                file.descriptor.size,
            )
        })
        .collect()
}

/// Recognized journal-derived per-segment outputs: same-stem media sidecars,
/// `events.jsonl`, `timeline.json`, and anything under `talents/`.
pub fn is_structural_derived_file(rel_name: &str, identity: &ContentIdentity) -> bool {
    if let Some(rest) = rel_name.strip_prefix("talents/") {
        return !rest.is_empty();
    }
    if rel_name.contains('/') {
        return false;
    }
    if rel_name == "events.jsonl" || rel_name == "timeline.json" {
        return true;
    }
    let Some(dot) = rel_name.rfind('.') else {
        return false;
    };
    let suffix = rel_name[dot..].to_ascii_lowercase();
    if suffix != ".jsonl" && suffix != ".npz" {
        return false;
    }
    let stem = &rel_name[..dot];
    identity.files().values().any(|file| {
        let name = file.descriptor.name.as_str();
        is_media_name(name) && name.rfind('.').is_some_and(|cut| &name[..cut] == stem)
    })
}

/// Files present on disk that are neither reserved, nor part of the
/// established content identity, nor a recognized derived output.
pub fn unknown_files(segment_dir: &Path, identity: &ContentIdentity) -> Vec<String> {
    let mut unknown = Vec::new();
    walk_files(segment_dir, segment_dir, &mut unknown, identity);
    unknown.sort();
    unknown
}

fn walk_files(root: &Path, dir: &Path, out: &mut Vec<String>, identity: &ContentIdentity) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut paths: Vec<_> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .collect();
    paths.sort();
    for path in paths {
        let Ok(metadata) = fs::symlink_metadata(&path) else {
            continue;
        };
        if metadata.is_dir() {
            walk_files(root, &path, out, identity);
            continue;
        }
        if !metadata.is_file() {
            continue;
        }
        let Ok(rel) = path.strip_prefix(root) else {
            continue;
        };
        let rel = rel.to_string_lossy().replace('\\', "/");
        if RESERVED_SEGMENT_FILENAMES.contains(&rel.as_str()) {
            continue;
        }
        if identity
            .files()
            .keys()
            .any(|name| name.as_str() == rel.as_str())
        {
            continue;
        }
        if is_structural_derived_file(&rel, identity) {
            continue;
        }
        out.push(rel);
    }
}

/// Whether `candidate_dir` still holds real bytes for content the canonical
/// only holds via terminal processing proof -- the "last physical copy" case.
pub fn is_last_physical_copy(canonical_identity: &ContentIdentity, candidate_dir: &Path) -> bool {
    canonical_identity.files().values().any(|file| {
        file.evidence == ContentIdentityEvidence::TerminalProof
            && candidate_dir.join(file.descriptor.name.as_str()).is_file()
    })
}

/// The first differing (or missing/extra) content name between two
/// identities, for refusal messages.
pub fn first_identity_difference(
    canonical: &ContentIdentity,
    candidate: &ContentIdentity,
) -> Option<String> {
    let canonical_names: std::collections::BTreeSet<_> =
        canonical.files().keys().map(|name| name.as_str()).collect();
    let candidate_names: std::collections::BTreeSet<_> =
        candidate.files().keys().map(|name| name.as_str()).collect();
    if let Some(extra) = candidate_names.difference(&canonical_names).next() {
        return Some((*extra).to_owned());
    }
    if let Some(missing) = canonical_names.difference(&candidate_names).next() {
        return Some((*missing).to_owned());
    }
    for name in canonical_names {
        let left = canonical
            .files()
            .iter()
            .find(|(key, _)| key.as_str() == name)
            .map(|(_, file)| file);
        let right = candidate
            .files()
            .iter()
            .find(|(key, _)| key.as_str() == name)
            .map(|(_, file)| file);
        if let (Some(left), Some(right)) = (left, right)
            && (left.descriptor.sha256 != right.descriptor.sha256
                || left.descriptor.size != right.descriptor.size)
        {
            return Some(name.to_owned());
        }
    }
    None
}

fn identity_refusal(label: &str, error: SegmentError) -> Refusal {
    match error {
        SegmentError::IdentityRefusal { name, reason } => {
            Refusal::new(label, "canonical-heldness", Some(name), reason)
        }
        SegmentError::Tombstoned { path } => Refusal::new(
            label,
            "canonical-heldness",
            Some("tombstone.json".to_owned()),
            format!("segment is tombstoned at {}", path.display()),
        ),
        other => Refusal::new(
            label,
            "canonical-heldness",
            None::<String>,
            other.to_string(),
        ),
    }
}

/// Analyze one segment: content identity, chain marker, and any unknown
/// (non-derived, non-identity) files on disk.
pub fn analyze_segment(
    journal: &Path,
    day: &str,
    stream: &str,
    segment: &str,
    path: &Path,
) -> SegmentAnalysis {
    let label = format!("{day}/{stream}/{segment}");
    let segment_dir = match SegmentDir::resolve(journal, day, segment, stream) {
        Ok(segment_dir) => segment_dir,
        Err(error) => {
            return SegmentAnalysis {
                day: day.to_owned(),
                stream: stream.to_owned(),
                segment: segment.to_owned(),
                path: path.to_owned(),
                marker: None,
                marker_error: None,
                identity: None,
                identity_issue: Some(identity_refusal(&label, error)),
                unknown_files: Vec::new(),
            };
        }
    };
    let proof = SegmentTerminalProof::new(&segment_dir);
    let (identity, identity_issue) = match load_content_identity(&segment_dir, &proof) {
        Ok(identity) => (Some(identity), None),
        Err(error) => (None, Some(identity_refusal(&label, error))),
    };
    let unknown = identity
        .as_ref()
        .map(|identity| unknown_files(path, identity))
        .unwrap_or_default();
    let marker = read_segment_marker(path);
    let marker_error = if marker.is_none() {
        Some("restore a readable stream.json marker before pruning".to_owned())
    } else if marker
        .as_ref()
        .is_some_and(|marker| marker.stream != stream)
    {
        Some(
            "rewrite stream.json so its stream matches the segment directory before pruning"
                .to_owned(),
        )
    } else {
        None
    };
    SegmentAnalysis {
        day: day.to_owned(),
        stream: stream.to_owned(),
        segment: segment.to_owned(),
        path: path.to_owned(),
        marker,
        marker_error,
        identity,
        identity_issue,
        unknown_files: unknown,
    }
}
