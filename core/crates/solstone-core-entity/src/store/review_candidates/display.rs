// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::{BTreeSet, HashMap, HashSet};
use std::ffi::OsStr;
use std::io;
use std::path::Path;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use solstone_core_journal_io::JournalRoot;
use solstone_core_journal_io::media::{FileReadVersion, MediaDirectory};

pub(super) const FACE_MESSAGE: &str = "journal.window.entities.merge.evidence.meeting.photos";
const FILE: &str = "review-candidates.jsonl";
pub(in crate::store) const FIELD: &str = "review_display";
pub(in crate::store) const MAX_BYTES: usize = 4 * 1024 * 1024;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Receipt {
    version: u32,
    file: FileReadVersion,
    meeting_photos: BTreeSet<String>,
}

// Decisions, suppression and resurfacing do not author new evidence. A caller
// explicitly replacing the evidence always removes this identity, even if its
// words and second-resolution timestamp happen to agree with the previous row.
fn identity(row: &Value) -> String {
    let fields = [
        &row["facet"],
        &row["source_slug"],
        &row["target_slug"],
        &row["source"],
        &row["target"],
        &row["created_at"],
        &row["first_surfaced"],
        &row["evidence"],
    ];
    format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&fields).expect("JSON values serialize"))
    )
}

pub(super) fn directory(root: &Path) -> Option<Arc<MediaDirectory>> {
    MediaDirectory::open(
        Arc::new(JournalRoot::open(root).ok()?),
        Path::new("entities"),
    )
    .ok()
}

pub(super) fn read(dir: &MediaDirectory) -> Option<(FileReadVersion, BTreeSet<String>)> {
    let bytes = dir
        .read_file_bounded(OsStr::new(crate::faceprints::cross::FILE), MAX_BYTES)
        .ok()??;
    let derived: Value = serde_json::from_slice(&bytes.bytes).ok()?;
    verified(dir, &derived)
}

fn verified(dir: &MediaDirectory, derived: &Value) -> Option<(FileReadVersion, BTreeSet<String>)> {
    if crate::faceprints::cross::Derived::deserialize(derived)
        .ok()?
        .schema_version
        != 1
    {
        return None;
    }
    let receipt: Receipt = serde_json::from_value(derived.get(FIELD)?.clone()).ok()?;
    let version = version(dir)?;
    (receipt.version == 1 && receipt.file == version).then_some((version, receipt.meeting_photos))
}

pub(in crate::store) fn field_for_republish(root: &Path, current: &str) -> Option<Value> {
    if current.len() > MAX_BYTES {
        return None;
    }
    let mut value: Value = serde_json::from_str(current).ok()?;
    verified(directory(root)?.as_ref(), &value)?;
    value.as_object_mut()?.remove(FIELD)
}

pub(super) fn version(dir: &MediaDirectory) -> Option<FileReadVersion> {
    dir.read_versioned(OsStr::new(FILE), |_, version| version)
        .ok()
}

pub(super) fn project(rows: &mut [Value], identities: &BTreeSet<String>, locale: &str) {
    for row in rows {
        if row.pointer("/evidence/basis").and_then(Value::as_str)
            == Some(crate::faceprints::cross::MERGE_BASIS)
            && identities.contains(&identity(row))
        {
            row["evidence"]["summary"] =
                solstone_core_window_catalog::text(locale, FACE_MESSAGE).into();
        }
    }
}

/// Held under the candidate writer's lock. The file version makes a stale,
/// missing or failed companion harmless, even after a byte-identical rewrite
/// by an older journal. A copied file without its original version reads raw.
pub(super) fn write(
    dir: &MediaDirectory,
    previous: &BTreeSet<String>,
    rows: &[Value],
    authored: Option<(&str, bool)>,
) -> io::Result<()> {
    if previous.is_empty() && !authored.is_some_and(|(_, face)| face) {
        return Ok(());
    }
    let mut seen = HashSet::new();
    let mut duplicated = HashSet::new();
    for row in rows {
        let key = super::candidate_key_for_row(row);
        if !seen.insert(key.clone()) {
            duplicated.insert(key);
        }
    }
    let mut next = BTreeSet::new();
    for row in rows {
        let key = super::candidate_key_for_row(row);
        if duplicated.contains(&key) {
            continue;
        }
        let authored = authored.filter(|(written, _)| *written == key);
        if row.pointer("/evidence/basis").and_then(Value::as_str)
            != Some(crate::faceprints::cross::MERGE_BASIS)
        {
            continue;
        }
        let hash = identity(row);
        if authored.map_or_else(|| previous.contains(&hash), |(_, face)| face) {
            next.insert(hash);
        }
    }
    if next.is_empty() && previous.is_empty() {
        return Ok(());
    }
    let field = if next.is_empty() {
        None
    } else {
        let file = dir.read_versioned(OsStr::new(FILE), |_, version| version)?;
        Some(serde_json::to_value(Receipt {
            version: 1,
            file,
            meeting_photos: next,
        })?)
    };
    crate::faceprints::cross::publish_candidate_display(dir, field)
}

/// Prepared batches own changed rows. Their source does not establish face
/// evidence, even if a proposed after-image repeats a prior sentence.
pub(super) fn write_prepared(
    dir: &MediaDirectory,
    previous: &BTreeSet<String>,
    before: &str,
    after: &str,
) -> io::Result<()> {
    // Preparing an unrelated proposal also applies automatic review policy to
    // every existing row. Suppressing or releasing it does not reauthor evidence.
    let stamped_key = |row: &Value| {
        (row.pointer("/evidence/basis").and_then(Value::as_str)
            == Some(crate::faceprints::cross::MERGE_BASIS))
        .then(|| identity(row))
        .filter(|hash| previous.contains(hash))
    };
    let without_review = |mut row: Value| {
        if let Some(fields) = row.as_object_mut() {
            fields.remove("review");
        }
        row
    };
    let mut stamped = HashMap::new();
    for line in before.lines().filter(|line| !line.trim().is_empty()) {
        let row: Value = serde_json::from_str(line)?;
        if let Some(hash) = stamped_key(&row) {
            stamped.insert(hash, without_review(row));
        }
    }
    let mut retained = BTreeSet::new();
    let mut rows = Vec::new();
    for line in after.lines().filter(|line| !line.trim().is_empty()) {
        let row: Value = serde_json::from_str(line)?;
        if let Some(hash) = stamped_key(&row)
            && stamped.get(&hash) == Some(&without_review(row.clone()))
        {
            retained.insert(hash);
        }
        rows.push(row);
    }
    write(dir, &retained, &rows, None)
}

// Existing canonical publication uses an atomic replacement under its writer
// lock. Record origins only after that replacement. Interrupted publication
// leaves English and the preceding (now inapplicable) version witness.
pub(super) fn origins(dir: Option<&MediaDirectory>) -> BTreeSet<String> {
    dir.and_then(read)
        .map(|(_, names)| names)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::review_candidates::{
        PreparedMergeProposals, load_merge_candidates, publish_merge_proposals,
        record_meeting_photo_candidate, record_merge_candidate_checked,
    };
    use solstone_core_journal_io::{AtomicWriteOptions, read_optional_text, write_text};

    fn record(root: &Path, source: &str, target: &str) {
        record_meeting_photo_candidate(root, "work", "20261101", source, source, target, target)
            .unwrap();
    }

    #[test]
    fn identical_duplicate_rows_stay_verbatim_after_one_is_authored() {
        let journal = tempfile::tempdir().unwrap();
        let root = journal.path();
        record(root, "a", "b");
        let path = root.join("entities").join(FILE);
        let first = read_optional_text(&path).unwrap().unwrap();
        write_text(
            &path,
            &first.repeat(2),
            AtomicWriteOptions { mode: Some(0o600) },
        )
        .unwrap();
        record(root, "c", "d");
        record(root, "a", "b");
        let origins = origins(directory(root).as_deref());
        let rows = load_merge_candidates(root, None, None).unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(origins, BTreeSet::from([identity(&rows[2])]));
    }

    #[test]
    fn final_checkpoint_refusal_preserves_other_evidence_and_generation() {
        let journal = tempfile::tempdir().unwrap();
        let root = journal.path();
        record(root, "a", "b");
        record(root, "c", "d");
        let dir = directory(root).unwrap();
        let before = read(&dir).unwrap();
        let side = root.join("entities").join(crate::faceprints::cross::FILE);
        let before_bytes = read_optional_text(&side).unwrap().unwrap();
        let count = std::cell::Cell::new(0);
        let result = record_merge_candidate_checked(
            root,
            "work",
            "20261101",
            "a",
            "a",
            "b",
            "b",
            "fixture",
            None,
            None,
            None,
            &|| {
                count.set(count.get() + 1);
                if count.get() == 2 {
                    Err("fixture refusal".into())
                } else {
                    Ok(())
                }
            },
        );
        assert!(result.is_err());
        assert_eq!(count.get(), 2);
        assert_eq!(before.1.len(), 2);
        assert_eq!(read(&dir).unwrap(), before);
        assert_eq!(read_optional_text(side).unwrap().unwrap(), before_bytes);
    }

    #[test]
    fn face_wipe_retires_stale_receipt_and_interrupted_stage_after_rows_disappear() {
        let journal = tempfile::tempdir().unwrap();
        let root = journal.path();
        record(root, "a", "b");
        let store = root.join("entities").join(crate::faceprints::cross::FILE);
        let receipt = read_optional_text(&store).unwrap().unwrap();
        let stage = root
            .join("entities")
            .join(crate::faceprints::cross::DISPLAY_STAGES)
            .join(".tmp_7_31.tmp");
        let options = AtomicWriteOptions { mode: Some(0o600) };
        write_text(&stage, &receipt, options).unwrap();
        write_text(root.join("entities").join(FILE), "", options).unwrap();
        let other = root.join("health").join("other.json");
        write_text(&other, "{}", options).unwrap();
        write_text(
            root.join("config/journal.json"),
            r#"{"faces":{"enabled":false}}"#,
            options,
        )
        .unwrap();
        crate::faceprints::accumulation::clear(root, solstone_core_removals::Surface::Cli).unwrap();
        assert!(!solstone_core_journal_io::path_lexists(&store).unwrap());
        assert!(!solstone_core_journal_io::path_lexists(&stage).unwrap());
        assert_eq!(read_optional_text(other).unwrap().as_deref(), Some("{}"));
    }

    #[test]
    fn prepared_replacement_with_identical_evidence_only_retains_untouched_rows() {
        let journal = tempfile::tempdir().unwrap();
        let root = journal.path();
        record(root, "a", "b");
        record(root, "c", "d");
        let before = read_optional_text(root.join("entities").join(FILE))
            .unwrap()
            .unwrap();
        let mut rows = load_merge_candidates(root, None, None).unwrap();
        rows[0]["last_surfaced"] = "20261102".into();
        let after = rows.iter().map(|row| format!("{row}\n")).collect();
        let batch = PreparedMergeProposals {
            before: Some(before),
            after,
        };
        publish_merge_proposals(root, &batch, true, || Ok(()), || Ok(())).unwrap();
        assert_eq!(
            origins(directory(root).as_deref()),
            BTreeSet::from([identity(&rows[1])])
        );
    }
}
