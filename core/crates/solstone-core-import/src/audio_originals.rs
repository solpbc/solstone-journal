// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Retire audio input only after all parts of a recording have durable transcripts.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use solstone_core_processing_record::vocab;
use solstone_core_retention::{
    ContentName, ProvenRaw, RawReleaseClass,
    content::ClosedHandlerSet,
    eligibility::{Evidence, FileVerdict, resolve_file},
    recorded_original_deletions, release_raw,
    scan::read_sidecar,
};
use solstone_core_segment::output_attempt::OutputClaims;

use crate::{AttemptState, ProjectionStatus, originals};

const PLAN: &str = "audio_originals_v1";
const RELEASED: &str = "audio_originals_released_generation";

pub(crate) fn preserve_release_evidence(
    merged: &mut crate::ImportMetadata,
    existing: &crate::ImportMetadata,
) {
    for key in [PLAN, RELEASED] {
        if let Some(value) = existing.get(key) {
            merged.insert(key.to_owned(), value.clone());
        } else {
            merged.remove(key);
        }
    }
}

#[derive(Deserialize, Serialize)]
struct Part {
    day: String,
    stream: String,
    segment: String,
    name: String,
    sha256: String,
    original: Option<Original>,
}

#[derive(Clone, Deserialize, Serialize)]
struct Original {
    source: PathBuf,
    relative: PathBuf,
    sha256: String,
    offset_ms: i64,
}

/// Original playback uses the segment's existing access route and locates
/// the chunk within the exact bytes retained by this import.
#[derive(Deserialize, Serialize)]
pub struct OriginalPlayback {
    import_id: String,
    relative: PathBuf,
    pub offset_ms: i64,
}

fn playback_reference(segment: &Path, raw: &str) -> Option<PathBuf> {
    let mut parts = Path::new(raw).components();
    if !matches!(parts.next(), Some(Component::Normal(_))) || parts.next().is_some() {
        return None;
    }
    Some(segment.join(raw).with_extension("original-audio.json"))
}

pub fn retained_playback(journal: &Path, segment: &Path, raw: &str) -> Option<(PathBuf, i64)> {
    let reference = playback_reference(segment, raw)?;
    if !std::fs::symlink_metadata(&reference).ok()?.is_file() {
        return None;
    }
    let reference: OriginalPlayback =
        serde_json::from_slice(&std::fs::read(reference).ok()?).ok()?;
    crate::validate_timestamp(&reference.import_id).ok()?;
    if reference.relative.as_os_str().is_empty()
        || !reference
            .relative
            .components()
            .all(|part| matches!(part, Component::Normal(_)))
    {
        return None;
    }
    let path = Path::new("imports")
        .join(&reference.import_id)
        .join("originals")
        .join(reference.relative);
    let relative = originals::journal_relative(&path)?;
    let resolved = solstone_core_journal_io::contained_path(journal, &relative).ok()?;
    if resolved != journal.canonicalize().ok()?.join(&path)
        || !std::fs::symlink_metadata(&resolved).ok()?.is_file()
    {
        return None;
    }
    Some((resolved, reference.offset_ms))
}

impl Part {
    fn directory(&self, journal: &Path) -> PathBuf {
        journal
            .join("chronicle")
            .join(&self.day)
            .join(&self.stream)
            .join(&self.segment)
    }
}

#[derive(Deserialize, Serialize)]
struct ReleasePlan {
    generation: u64,
    keep_audio: bool,
    parts: Vec<Part>,
    #[serde(default)]
    archive_root: Option<PathBuf>,
}

/// Bind the exact parts published by this import. Legacy imports have no
/// captured choice and never get a release plan.
pub fn plan(
    journal: &Path,
    id: &str,
    generation: u64,
    source: &Path,
    files: &[(PathBuf, i64)],
) -> Result<(), String> {
    if originals::captured_choice(journal, id).is_none() {
        return Ok(());
    }
    let sha256 = crate::hash_source(source)
        .map_err(|error| error.to_string())?
        .into_inner();
    let relative = source
        .file_name()
        .ok_or("audio source has no filename")?
        .into();
    let original = Original {
        source: source.to_path_buf(),
        relative,
        sha256,
        offset_ms: 0,
    };
    let files = files
        .iter()
        .map(|(file, offset_ms)| {
            (
                file.clone(),
                Original {
                    offset_ms: *offset_ms,
                    ..original.clone()
                },
            )
        })
        .collect::<Vec<_>>();
    plan_parts(journal, id, generation, &files, None)
}

fn plan_parts(
    journal: &Path,
    id: &str,
    generation: u64,
    files: &[(PathBuf, Original)],
    archive_root: Option<PathBuf>,
) -> Result<(), String> {
    let Some(keep_audio) = originals::captured_choice(journal, id) else {
        return Ok(());
    };
    let _lock = crate::hold_import_lock(journal, id).map_err(|error| error.to_string())?;
    let mut metadata =
        crate::read_import_metadata(journal, id).map_err(|error| error.to_string())?;
    if !eligible(&metadata, generation)
        || metadata
            .get(PLAN)
            .and_then(|plan| plan["generation"].as_u64())
            == Some(generation)
        || metadata.get(RELEASED).and_then(serde_json::Value::as_u64) == Some(generation)
    {
        return Ok(());
    }
    let mut parts = Vec::with_capacity(files.len());
    for (file, original) in files {
        let components: Vec<_> = file
            .strip_prefix(journal)
            .map_err(|error| error.to_string())?
            .components()
            .collect();
        let [
            std::path::Component::Normal(chronicle),
            std::path::Component::Normal(day),
            std::path::Component::Normal(stream),
            std::path::Component::Normal(segment),
            std::path::Component::Normal(name),
        ] = components.as_slice()
        else {
            return Err("audio import published an unexpected relative path".to_owned());
        };
        if chronicle != &"chronicle"
            || !stream
                .to_str()
                .is_some_and(|name| name.starts_with("import."))
        {
            return Err("audio import published an unexpected stream".to_owned());
        }
        let sha256 = match crate::hash_source(file) {
            Ok(hash) => hash.into_inner(),
            Err(error) => {
                let segment = file.parent().ok_or_else(|| error.to_string())?;
                if recorded_original_deletions(segment).get(name.to_string_lossy().as_ref())
                    != Some(&RawReleaseClass::OwnerVoice)
                {
                    return Err(error.to_string());
                }
                String::new()
            }
        };
        parts.push(Part {
            day: day.to_string_lossy().into_owned(),
            stream: stream.to_string_lossy().into_owned(),
            segment: segment.to_string_lossy().into_owned(),
            name: name.to_string_lossy().into_owned(),
            sha256,
            original: keep_audio.then(|| original.clone()),
        });
    }
    if !parts.is_empty() {
        let release = ReleasePlan {
            generation,
            keep_audio,
            parts,
            archive_root,
        };
        metadata.insert(
            PLAN.to_owned(),
            serde_json::to_value(release).map_err(|error| error.to_string())?,
        );
        crate::metadata::write_import_metadata_unlocked(journal, id, &metadata)
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

/// Bind the published archive's exact parts, leaving incomplete runs available
/// for normal resume. The archive frontier has its own processing receipt.
pub fn plan_archive(journal: &Path, id: &str, source: &Path) -> Result<(), String> {
    let Some(archive) =
        crate::audio_archive::read_audio_archive(journal, id).map_err(|error| error.to_string())?
    else {
        return Ok(());
    };
    let metadata = crate::read_import_metadata(journal, id).map_err(|error| error.to_string())?;
    let Some(attempt) = crate::get_attempt_facts(&metadata) else {
        return Ok(());
    };
    if archive.parts.is_empty()
        || archive
            .parts
            .iter()
            .any(|part| part.outcome != crate::audio_archive::PartOutcome::Saved)
        || archive
            .members
            .iter()
            .any(|member| member.save_rejection.is_some())
    {
        return Ok(());
    }
    let mut files = Vec::new();
    for part in archive.parts {
        let member = archive
            .members
            .iter()
            .find(|member| {
                member.canonical_occurrence == part.member_occurrence
                    && member.sha256 == part.member_sha256
            })
            .ok_or("saved archive part has no original member")?;
        let path = journal
            .join("chronicle")
            .join(part.day)
            .join(&archive.settings.stream)
            .join(part.segment_key)
            .join(part.stored_name);
        let relative = PathBuf::from(&member.original_relative_name);
        files.push((
            path,
            Original {
                source: source.join(&relative),
                relative,
                sha256: member.sha256.clone(),
                offset_ms: part.origin_ms,
            },
        ));
    }
    plan_parts(
        journal,
        id,
        attempt.generation,
        &files,
        Some(source.to_path_buf()),
    )
}

fn eligible(metadata: &crate::ImportMetadata, generation: u64) -> bool {
    crate::get_attempt_facts(metadata).is_some_and(|attempt| {
        attempt.generation == generation
            && matches!(
                attempt.state,
                AttemptState::Completed | AttemptState::Unconfirmed
            )
            && attempt.failure_reason.is_none()
            && attempt.input_failures.unwrap_or_default() == 0
    })
}

/// The sync ledger must have checkpointed the exact remote recording before
/// an importer releases any of its downloaded or remuxed audio.
fn plaud_checkpointed(journal: &Path, id: &str, metadata: &crate::ImportMetadata) -> bool {
    let Some(remote_id) = metadata
        .get("plaud_file_id")
        .and_then(serde_json::Value::as_str)
    else {
        return true;
    };
    let crate::SyncStateRead::Loaded(state) =
        crate::read_sync_state(journal, crate::BackendName::Plaud)
    else {
        return false;
    };
    let entry = &state.root()["files"][remote_id];
    entry["status"].as_str() == Some("imported") && entry["import_timestamp"].as_str() == Some(id)
}

enum Readiness {
    Pending,
    Released,
    Ready(ProvenRaw),
}

fn verify_part(journal: &Path, part: &Part) -> Result<Readiness, String> {
    let segment = part.directory(journal);
    let raw = segment.join(&part.name);
    let metadata = match std::fs::symlink_metadata(&raw) {
        Ok(file) if file.is_file() => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(
                match recorded_original_deletions(&segment).get(&part.name) {
                    Some(RawReleaseClass::Import | RawReleaseClass::OwnerVoice) => {
                        Readiness::Released
                    }
                    _ => Readiness::Pending,
                },
            );
        }
        Ok(_) => return Err("imported recording no longer names a regular file".to_owned()),
        Err(error) => return Err(error.to_string()),
    };
    let Some(name) = ContentName::new(&part.name) else {
        return Err("imported recording has an invalid name".to_owned());
    };
    let facts = read_sidecar(
        &raw.with_extension("jsonl"),
        Some(vocab::AUDIO_TRANSCRIPT_ROW_KEY),
    );
    let decision = resolve_file(
        &ClosedHandlerSet,
        &part.day,
        &part.stream,
        &part.segment,
        &name,
        metadata.len(),
        &facts,
    );
    let FileVerdict::Releasable(proof) = decision else {
        return Ok(Readiness::Pending);
    };
    if proof.evidence() != Evidence::Record {
        return Ok(Readiness::Pending);
    }
    let hash = crate::hash_source(&raw).map_err(|error| error.to_string())?;
    if hash.as_str() != part.sha256 {
        return Err("imported recording changed after it was saved".to_owned());
    }
    Ok(Readiness::Ready(proof))
}

/// Called after a recording is filed and again by maintenance if processing was
/// pending. Any failure to prove the whole recording leaves retryable source bytes.
pub fn finish(journal: &Path, id: &str) -> Result<(), String> {
    let _target = solstone_core_journal_io::whole_journal_restore::begin_target_use(journal)
        .map_err(|error| error.to_string())?;
    if crate::validate_timestamp(id).is_err() {
        return Ok(());
    }
    let _lock = crate::hold_import_lock(journal, id).map_err(|error| error.to_string())?;
    let metadata = crate::read_import_metadata(journal, id).map_err(|error| error.to_string())?;
    let Some(value) = metadata.get(PLAN) else {
        return Ok(());
    };
    let plan: ReleasePlan =
        serde_json::from_value(value.clone()).map_err(|error| error.to_string())?;
    if !eligible(&metadata, plan.generation) || !plaud_checkpointed(journal, id, &metadata) {
        return Ok(());
    }
    if plan.archive_root.is_some() {
        let status = crate::audio_archive::ArchiveStore::open(journal, id, false)
            .and_then(|store| store.status())
            .map_err(|error| error.to_string())?;
        if !status.save_complete
            || status.pending_processing > 0
            || status.unavailable_parts > 0
            || status.rejected_parts > 0
            || status.processed_parts != status.total_parts
        {
            return Ok(());
        }
    } else if crate::project_import_result(journal, id).status != ProjectionStatus::Success {
        return Ok(());
    }
    for part in &plan.parts {
        if matches!(verify_part(journal, part)?, Readiness::Pending) {
            return Ok(());
        }
    }
    if let Some(attempt) = crate::get_attempt_facts(&metadata)
        && attempt.state == AttemptState::Unconfirmed
    {
        let files: Vec<_> = plan
            .parts
            .iter()
            .map(|part| {
                part.directory(journal)
                    .join(&part.name)
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        let mut days: Vec<_> = plan.parts.iter().map(|part| part.day.clone()).collect();
        days.sort();
        days.dedup();
        let source_hash = metadata
            .get("source_hash")
            .and_then(serde_json::Value::as_str)
            .ok_or("the import has no source hash")?;
        crate::write_manifest(&crate::ManifestWriteRequest {
            journal_root: journal,
            import_id: id,
            source_type: "audio",
            source_hash: &crate::SourceHash::new(source_hash.to_owned()),
            entry_count: plan.parts.len() as u64,
            days_affected: &days,
            files_created: &files,
            imported_via: "native",
            link_id: None,
            observer_handle: None,
            raw_retention: None,
            span: None,
        })
        .map_err(|error| error.to_string())?;
        // Only the publication and all of the saved transcripts together can
        // confirm the producer that stopped waiting. Save that conclusion before
        // unlinking the files the pending projection uses to prove completion.
        let mut confirmed = metadata.clone();
        let mut attempt = attempt;
        attempt.state = AttemptState::Completed;
        confirmed.insert(
            "attempt".to_owned(),
            serde_json::to_value(attempt).map_err(|error| error.to_string())?,
        );
        crate::metadata::write_import_metadata_unlocked(journal, id, &confirmed)
            .map_err(|error| error.to_string())?;
    }
    let mut preserved = BTreeMap::new();
    if plan.keep_audio {
        for original in plan.parts.iter().filter_map(|part| part.original.as_ref()) {
            if let std::collections::btree_map::Entry::Vacant(entry) =
                preserved.entry(&original.relative)
            {
                let kept = originals::preserve_original(
                    journal,
                    id,
                    &original.source,
                    &original.relative,
                    &original.sha256,
                )?;
                entry.insert(kept);
            }
        }
    }
    for part in &plan.parts {
        let segment = part.directory(journal);
        let jsonl = segment.join(&part.name).with_extension("jsonl");
        let claims =
            OutputClaims::acquire([jsonl], solstone_core_journal_io::LockOptions::default())
                .map_err(|error| error.to_string())?;
        let _commit = claims.commit(&segment).map_err(|error| error.to_string())?;
        let proof = match verify_part(journal, part)? {
            Readiness::Ready(proof) => proof,
            Readiness::Released => continue,
            Readiness::Pending => return Ok(()),
        };
        if let Some(original) = &part.original
            && preserved.contains_key(&original.relative)
        {
            let reference = OriginalPlayback {
                import_id: id.to_owned(),
                relative: original.relative.clone(),
                offset_ms: original.offset_ms,
            };
            let path =
                playback_reference(&segment, &part.name).ok_or("invalid recording filename")?;
            solstone_core_journal_io::atomic_replace(
                &path,
                &serde_json::to_vec(&reference).map_err(|error| error.to_string())?,
                solstone_core_journal_io::AtomicWriteOptions { mode: Some(0o600) },
            )
            .map_err(|error| error.to_string())?;
        }
        let (outcome, _) = release_raw(
            journal,
            &[proof],
            RawReleaseClass::Import,
            jiff::Timestamp::now(),
        );
        if outcome.halted.is_some()
            || outcome.targets.iter().any(|target| {
                !target.not_removed.is_empty() || target.post_commit_failure.is_some()
            })
        {
            return Err("finished imported audio is still owed cleanup".to_owned());
        }
    }
    if let Some(source) = &plan.archive_root {
        originals::discard_archive_folder(journal, id, source)?;
    }
    let staged = metadata
        .get("file_path")
        .and_then(serde_json::Value::as_str)
        .map(PathBuf::from);
    drop(_lock);
    if let Some(staged) = staged {
        originals::discard_finished_upload(journal, id, &staged, plan.generation, true)?;
    }
    let _lock = crate::hold_import_lock(journal, id).map_err(|error| error.to_string())?;
    let mut metadata =
        crate::read_import_metadata(journal, id).map_err(|error| error.to_string())?;
    if eligible(&metadata, plan.generation) {
        metadata.remove(PLAN);
        metadata.insert(RELEASED.to_owned(), serde_json::json!(plan.generation));
        crate::metadata::write_import_metadata_unlocked(journal, id, &metadata)
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

pub fn finish_owed(
    journal: &Path,
    checkpoint: &dyn Fn() -> Result<(), String>,
) -> Result<(), String> {
    let entries = match std::fs::read_dir(journal.join("imports")) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.to_string()),
    };
    let mut failure = None;
    for entry in entries {
        checkpoint()?;
        let entry = entry.map_err(|error| error.to_string())?;
        let Some(id) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if crate::validate_timestamp(&id).is_ok()
            && crate::read_import_metadata(journal, &id)
                .is_ok_and(|metadata| metadata.contains_key(PLAN))
            && let Err(error) = finish(journal, &id)
        {
            log::warn!("import audio cleanup {id}: {error}");
            failure = Some(error);
        }
    }
    failure.map_or(Ok(()), Err)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn all_audio_parts_must_finish_and_keep_holds_one_exact_playable_source() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let original = b"unmodified synthetic recording";
        for (id, day, source, keep) in [
            ("20261118_090000", "20261118", "audio", false),
            ("20261119_090000", "20261119", "audio", true),
            ("20261120_090000", "20261120", "plaud", false),
            ("20261121_090000", "20261121", "plaud", true),
        ] {
            let directory = root.join("imports").join(id);
            std::fs::create_dir_all(&directory).unwrap();
            let staged = directory.join("recording.wav");
            std::fs::write(&staged, original).unwrap();
            let hash = crate::hash_source(&staged).unwrap();
            let mut metadata = json!({
                "import_id": id, "source_timestamp": id, "file_path": staged,
                "source_hash": hash.as_str(), "original_filename": "recording.wav",
            })
            .as_object()
            .unwrap()
            .clone();
            originals::record_staged_upload(&mut metadata, &staged);
            if source == "plaud" {
                metadata.insert("plaud_file_id".to_owned(), json!(id));
            }
            crate::write_import_metadata(&root, id, &metadata).unwrap();
            originals::set_preference(&root, source, keep).unwrap();
            originals::capture_choice(&root, id, source).unwrap();
            // A change to the preference only reaches the next admitted recording.
            originals::set_preference(&root, source, !keep).unwrap();
            let attempt = crate::admit_running_attempt(&root, id, 1, Some("audio")).unwrap();
            crate::record_completed_attempt(&root, id, attempt.generation, 2, None, None).unwrap();
            let parts = ["090000_300", "090500_300"]
                .into_iter()
                .enumerate()
                .map(|(index, segment)| {
                    let directory = root
                        .join("chronicle")
                        .join(day)
                        .join("import.audio")
                        .join(segment);
                    std::fs::create_dir_all(&directory).unwrap();
                    let audio = directory.join("imported_audio.wav");
                    std::fs::write(&audio, b"separate part").unwrap();
                    (audio, index as i64 * 300_000)
                })
                .collect::<Vec<_>>();
            plan(&root, id, attempt.generation, &staged, &parts).unwrap();
            let write = |index: usize, state: &str| {
                let (path, _) = &parts[index];
                let header = json!({"raw":"imported_audio.wav","_solstone_processing":{
                    "schema": vocab::SCHEMA, "handler":vocab::HANDLER_TRANSCRIBE,
                    "state":state, "input_size":13,
                }});
                let row = json!({"start": "09:00:00", "text": "saved words"});
                std::fs::write(path.with_extension("jsonl"), format!("{header}\n{row}\n")).unwrap();
            };
            write(0, vocab::STATE_ANALYZED);
            write(1, vocab::STATE_FAILED);
            finish(&root, id).unwrap();
            assert!(staged.is_file());
            assert!(parts.iter().all(|(part, _)| part.is_file()));

            write(1, vocab::STATE_ANALYZED);
            if source == "plaud" {
                finish(&root, id).unwrap();
                assert!(staged.is_file());
                assert!(parts.iter().all(|(part, _)| part.is_file()));
                let mut checkpoint = crate::SyncState::empty(crate::BackendName::Plaud);
                checkpoint.files_mut().insert(
                    id.to_owned(),
                    json!({
                        "status": "imported", "import_timestamp": id,
                    }),
                );
                crate::sync_state::write_sync_state(&root, &checkpoint).unwrap();
            }
            finish(&root, id).unwrap();
            finish(&root, id).unwrap();
            assert!(!staged.exists());
            for (part, offset) in parts {
                assert!(!part.exists());
                assert!(part.with_extension("jsonl").is_file());
                let segment = part.parent().unwrap();
                assert_eq!(
                    recorded_original_deletions(segment)["imported_audio.wav"],
                    RawReleaseClass::Import
                );
                let playback = retained_playback(&root, segment, "imported_audio.wav");
                if keep {
                    let (file, actual_offset) = playback.unwrap();
                    assert_eq!(file, directory.join("originals/recording.wav"));
                    assert_eq!(actual_offset, offset);
                    assert_eq!(std::fs::read(file).unwrap(), original);
                } else {
                    assert!(playback.is_none());
                }
            }
        }
    }

    #[test]
    fn transcribed_audio_requires_a_terminal_record_and_matching_bytes() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let mut part = Part {
            day: "20261118".to_owned(),
            stream: "import.audio".to_owned(),
            segment: "085000_300".to_owned(),
            name: "imported_audio.wav".to_owned(),
            sha256: String::new(),
            original: None,
        };
        let directory = part.directory(root);
        std::fs::create_dir_all(&directory).unwrap();
        let audio = directory.join(&part.name);
        std::fs::write(&audio, b"synthetic audio").unwrap();
        part.sha256 = crate::hash_source(&audio).unwrap().into_inner();
        let sidecar = audio.with_extension("jsonl");
        let words = json!({"start":"08:50:00","text":"a synthetic recording"});
        assert!(matches!(
            verify_part(root, &part).unwrap(),
            Readiness::Pending
        ));

        let write_state = |state: &str| {
            let record = json!({"_solstone_processing":{
                "schema":vocab::SCHEMA,"handler":vocab::HANDLER_TRANSCRIBE,
                "state":state,"input_size":15,"reason":"ok"
            }});
            std::fs::write(&sidecar, format!("{record}\n{words}\n")).unwrap();
        };
        write_state(vocab::STATE_FAILED);
        assert!(matches!(
            verify_part(root, &part).unwrap(),
            Readiness::Pending
        ));
        write_state(vocab::STATE_ANALYZED);
        assert!(matches!(
            verify_part(root, &part).unwrap(),
            Readiness::Ready(_)
        ));
        std::fs::write(&audio, b"changed content").unwrap();
        assert!(verify_part(root, &part).is_err());
    }
}
