// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Complete-tile publication. Consuming the private stage is the saved-once witness.

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use chrono::Utc;
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use solstone_core_format::audio_archive::{
    AUDIO_ARCHIVE_PLACEMENT_FILENAME, AUDIO_ARCHIVE_PLACEMENT_VERSION, AudioArchivePlacement,
    AudioArchivePlacementPart,
};
use solstone_core_journal_io::{AtomicWriteOptions, LockOptions, atomic_replace};
use solstone_core_segment::output_attempt::SegmentMutation;
use solstone_core_segment::{ImportSource, Kind, StreamHints, advance_unbound_stream_checked};

use super::claims::{
    DEFAULT_WRITE_BUSY_TIMEOUT, find_held_members, insert_tile_claims, record_archive_run,
    register_pending_tile,
};
use super::plan::{
    AudioArchiveMemberInput, AudioArchivePlan, AudioArchiveSettings, AudioArchiveSize,
    LeftOutReason, PartOutcome, apply_exclusions, apply_held_members, plan_audio_archive,
};
use super::record::{
    ArchiveStore, AudioArchiveRecord, AudioArchiveRecordMember, AudioArchiveRecordPart,
    plan_to_record, read_audio_archive, save_audio_archive_record, validate_resume_settings,
};
use super::{AudioArchiveError, MemberIdentity};
use crate::allocate::allocate_import_id;
use crate::heartbeat::{HeartbeatSpawner, ImportHeartbeat};
use crate::metadata::{
    AttemptState, admit_running_attempt, record_completed_attempt, resume_running_attempt,
};
use crate::staging::ensure_import_private_chain;
use crate::timestamp::{Timestamp, validate_timestamp};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArchiveFault {
    AfterRecordFirstSaved,
    AfterTileBytesStaged,
    AfterClaimAndIntent,
    BeforeTilePublish,
    AfterPublishBeforeSavedMark,
    BetweenMultiTileParts,
    AfterSavedMarkBeforeEmit,
}

pub type EmitObservedFn = Box<dyn Fn(&str, &str, &str) + Send + Sync>;
pub type ArchiveProgressFn =
    Arc<dyn Fn(&AudioArchiveProgress) -> Result<(), AudioArchiveError> + Send + Sync>;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ArchiveProgressPhase {
    Admitted,
    Saved,
}

#[derive(Clone, Debug, Serialize)]
pub struct AudioArchiveProgress {
    pub import_id: String,
    pub phase: ArchiveProgressPhase,
    pub status: super::AudioArchiveStatus,
}

#[derive(Default)]
pub struct AudioArchivePublishSeams {
    pub fault: Option<ArchiveFault>,
    pub emit_observed: Option<EmitObservedFn>,
    pub heartbeat_spawner: Option<HeartbeatSpawner>,
    pub progress: Option<ArchiveProgressFn>,
}

/// The host stages only this part, verifying its original source against `member`.
/// Whole members retain their original bytes; sliced parts have their own digest.
/// Return `SourceRejected` for an unreadable, unsupported or changed source;
/// other errors abort the writer because journal storage may be unavailable.
pub trait AudioArchiveMaterializer {
    fn materialize(
        &self,
        member: &AudioArchiveRecordMember,
        part: &AudioArchiveRecordPart,
        target: &Path,
    ) -> Result<(), AudioArchiveError>;
}

pub struct AudioArchivePublishOptions<'a> {
    pub import_id: Option<&'a str>,
    pub members: Vec<AudioArchiveMemberInput>,
    pub materializer: &'a dyn AudioArchiveMaterializer,
    pub owner_zone: Tz,
    pub settings: AudioArchiveSettings,
    pub source_timestamp: Option<Timestamp>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioArchivePublishResult {
    pub import_id: String,
    pub plan_size: AudioArchiveSize,
    pub tiles_saved: usize,
    pub parts_saved: usize,
    pub record: AudioArchiveRecord,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct TileIntentPart {
    stored_name: String,
    sha256: String,
    byte_len: u64,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct TileIntent {
    day: String,
    segment_key: String,
    stream: String,
    placement: AudioArchivePlacement,
    parts: Vec<TileIntentPart>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PublicationAction {
    Stage,
    PublishStaged,
    RecoverSaved,
    KeepSaved,
}

fn publication_action(saved: bool, intent: bool, stage: bool) -> PublicationAction {
    if saved {
        PublicationAction::KeepSaved
    } else if !intent {
        PublicationAction::Stage
    } else if stage {
        PublicationAction::PublishStaged
    } else {
        PublicationAction::RecoverSaved
    }
}

/// Read-only preview, including held identities and frozen tiles in this archive stream.
pub fn prepare_audio_archive_plan(
    journal: &Path,
    members: &[AudioArchiveMemberInput],
    owner_zone: Tz,
    settings: AudioArchiveSettings,
) -> Result<AudioArchivePlan, AudioArchiveError> {
    let mut plan = plan_audio_archive(members, owner_zone, settings)?;
    let held = find_held_members(journal, None, &plan.members, DEFAULT_WRITE_BUSY_TIMEOUT)?;
    apply_held_members(&mut plan, &held)?;
    let mut occupied = HashMap::new();
    let mut checked = HashMap::new();
    // Include members previously excluded by another input's interval. Once that
    // input is left out, they must not bypass a frozen tile in their own span.
    for member in &plan.members {
        if held.contains(&member.identity) {
            continue;
        }
        let instant = match &member.resolved_occurrence {
            Some(super::ResolvedOccurrence::UniqueInstant { instant, .. }) => *instant,
            Some(super::ResolvedOccurrence::SettledFold {
                candidate_instants, ..
            }) => candidate_instants[0],
            None => continue,
        };
        let Ok(parts) = super::plan::simulate_parts(instant, owner_zone, member.duration_ms) else {
            continue;
        };
        for part in parts {
            let coordinate = (part.day.clone(), part.segment_key.clone());
            if !checked.contains_key(&coordinate) {
                let target = journal
                    .join("chronicle")
                    .join(&part.day)
                    .join(&plan.stream)
                    .join(&part.segment_key);
                let exists = match fs::symlink_metadata(target) {
                    Ok(_) => true,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
                    Err(error) => return Err(error.into()),
                };
                checked.insert(coordinate.clone(), exists);
            }
            if checked[&coordinate] {
                occupied.insert(
                    member.identity.clone(),
                    LeftOutReason::PublishedTile {
                        day: part.day,
                        segment_key: part.segment_key,
                    },
                );
                break;
            }
        }
    }
    if !occupied.is_empty() {
        apply_exclusions(&mut plan, &occupied)?;
    }
    Ok(plan)
}

struct AttemptRelease<'a> {
    journal: &'a Path,
    import_id: String,
}
impl Drop for AttemptRelease<'_> {
    fn drop(&mut self) {
        crate::metadata::release_attempt(self.journal, &self.import_id);
    }
}

pub fn publish_audio_archive(
    journal: &Path,
    options: AudioArchivePublishOptions<'_>,
    seams: Option<AudioArchivePublishSeams>,
) -> Result<AudioArchivePublishResult, AudioArchiveError> {
    // Check claims before allocating: a removed run's timestamp may be reused.
    let new_plan = if options.import_id.is_none() {
        Some(prepare_audio_archive_plan(
            journal,
            &options.members,
            options.owner_zone,
            options.settings.clone(),
        )?)
    } else {
        None
    };
    let import_id = if let Some(id) = options.import_id {
        validate_timestamp(id).map_err(|error| AudioArchiveError::Other(error.to_string()))?;
        if !journal.join("imports").join(id).is_dir() {
            return Err(AudioArchiveError::MissingRunDirectory(
                journal.join("imports").join(id),
            ));
        }
        id.to_owned()
    } else {
        let stamp = match options.source_timestamp {
            Some(stamp) => stamp,
            None => validate_timestamp(&Utc::now().format("%Y%m%d_%H%M%S").to_string())
                .map_err(|error| AudioArchiveError::Other(error.to_string()))?,
        };
        allocate_import_id(journal, &stamp)
            .map_err(|error| AudioArchiveError::Other(error.to_string()))?
            .import_id
            .as_str()
            .to_owned()
    };
    let run = ensure_import_private_chain(journal, &import_id)
        .map_err(|error| AudioArchiveError::Other(error.to_string()))?;
    let _writer = solstone_core_journal_io::hold_lock(
        run.join(".audio-archive-writer"),
        LockOptions::default(),
    )
    .map_err(|error| AudioArchiveError::Other(error.to_string()))?;
    let _release = AttemptRelease {
        journal,
        import_id: import_id.clone(),
    };
    let generation;
    let attempt_id;
    if options.import_id.is_none() {
        let plan = new_plan.expect("new run was planned before allocation");
        let facts = admit_running_attempt(
            journal,
            &import_id,
            now_ms(),
            Some(&options.settings.archive_identity),
        )
        .map_err(|error| AudioArchiveError::Other(error.to_string()))?;
        generation = facts.generation;
        attempt_id = facts.attempt_id;
        save_audio_archive_record(journal, &import_id, &plan_to_record(&plan))?;
    } else {
        let record = read_audio_archive(journal, &import_id)?
            .ok_or_else(|| AudioArchiveError::MissingRunDirectory(run.clone()))?;
        let stream = crate::stream_name::canonicalize_stream_name(
            "import",
            Some(&options.settings.qualifier),
        )
        .map_err(|error| AudioArchiveError::Other(error.to_string()))?;
        validate_resume_settings(&record, options.owner_zone, &options.settings, &stream)?;
        // Resume uses the frozen plan. Supplied facts may rename a source but cannot change its identity, time or length.
        if !options.members.is_empty() {
            let fresh = plan_audio_archive(
                &options.members,
                options.owner_zone,
                options.settings.clone(),
            )?;
            let mut fresh_facts: Vec<_> = fresh
                .members
                .iter()
                .map(|member| {
                    (
                        &member.identity.sha256,
                        &member.identity.occurrence,
                        member.identity.byte_len,
                        member.duration_ms,
                    )
                })
                .collect();
            let mut saved_facts: Vec<_> = record
                .members
                .iter()
                .map(|member| {
                    (
                        &member.sha256,
                        &member.canonical_occurrence,
                        member.byte_len,
                        member.duration_ms,
                    )
                })
                .collect();
            fresh_facts.sort_unstable();
            saved_facts.sort_unstable();
            if fresh_facts != saved_facts {
                return Err(AudioArchiveError::SettingsMismatch(
                    "source identities or timing differ from the saved archive plan".to_owned(),
                ));
            }
        }
        let previous = crate::metadata::read_provenance(journal, &import_id)
            .map_err(|error| AudioArchiveError::Other(error.to_string()))?
            .and_then(|metadata| crate::metadata::get_attempt_facts(&metadata));
        if let Some(previous) = previous.filter(|facts| facts.state == AttemptState::Running) {
            resume_running_attempt(journal, &import_id)
                .map_err(|error| AudioArchiveError::Other(error.to_string()))?;
            generation = previous.generation;
            attempt_id = previous.attempt_id;
        } else {
            let facts = admit_running_attempt(journal, &import_id, now_ms(), None)
                .map_err(|error| AudioArchiveError::Other(error.to_string()))?;
            generation = facts.generation;
            attempt_id = facts.attempt_id;
        }
    }
    record_archive_run(journal, &import_id, &options.settings.archive_identity)?;
    let _heartbeat = start_run_heartbeat(journal, &import_id, generation, &attempt_id, &seams);
    let mut store = ArchiveStore::open(journal, &import_id, true)?;
    let settings = store.settings()?;
    let initial_status = store.status()?;
    let plan_size = initial_status.size.clone();
    report_progress(&seams, &store, &import_id, ArchiveProgressPhase::Admitted)?;
    check_fault(&seams, ArchiveFault::AfterRecordFirstSaved)?;
    let mut tiles_saved = 0;
    let coordinates = store.tile_coordinates()?;
    if options.import_id.is_some() {
        // Complete consumed-stage bookkeeping before any retry can reject other parts.
        for (day, key) in &coordinates {
            if store.tile_phase(day, key)? == 1 {
                recover_published_tile(journal, &import_id, &mut store, day, key)?;
            }
        }
        if store.status()?.saved_parts > initial_status.saved_parts {
            report_progress(&seams, &store, &import_id, ArchiveProgressPhase::Saved)?;
        }
    }
    for (day, key) in coordinates {
        let stage = stage_path(journal, &import_id, &day, &key);
        let intent_path = intent_path(journal, &import_id, &day, &key);
        if options.import_id.is_some()
            && store.tile_phase(&day, &key)? != 2
            && (!intent_path.try_exists()? || stage.try_exists()?)
        {
            let rejected: Vec<_> = store
                .tile_parts(&day, &key)?
                .into_iter()
                .filter(|part| part.outcome == PartOutcome::Rejected)
                .collect();
            if !rejected.is_empty() {
                let candidates: Vec<_> = rejected
                    .iter()
                    .map(|part| {
                        let member = store.member_for_part(part)?;
                        Ok(super::PlannedMember {
                            identity: MemberIdentity::new(
                                member.byte_len,
                                member.sha256,
                                member.canonical_occurrence,
                            ),
                            original_relative_name: member.original_relative_name,
                            duration_ms: member.duration_ms,
                            source_start: member.source_start,
                            resolved_occurrence: None,
                            disposition: super::MemberDisposition::Included,
                            run_note: member.run_note,
                        })
                    })
                    .collect::<Result<_, AudioArchiveError>>()?;
                let held = find_held_members(
                    journal,
                    Some(&import_id),
                    &candidates,
                    DEFAULT_WRITE_BUSY_TIMEOUT,
                )?;
                let mut eligible = Vec::new();
                for (part, member) in rejected.iter().zip(candidates) {
                    if held.contains(&member.identity) {
                        store.reject_member_remaining(
                            part,
                            super::SourceRejection::HeldByExistingRun,
                        )?;
                    } else {
                        eligible.push(part.stored_name.clone());
                    }
                }
                if !eligible.is_empty() {
                    if intent_path.try_exists()? {
                        // Retire the durable intent before discarding its intact unpublished stage.
                        // Cleanup must never look like a consumed-stage publication to discovery.
                        fs::remove_file(&intent_path)?;
                        sync_directory(journal, &format!("imports/{import_id}/tiles"))?;
                    }
                    store.update_retryable_parts(&day, &key, &eligible)?;
                }
            }
        }
        if store.tile_phase(&day, &key)? == 3 {
            continue;
        }
        let mut parts = store.publishable_tile_parts(&day, &key)?;
        let action = publication_action(
            store.tile_phase(&day, &key)? == 2,
            intent_path.try_exists()?,
            stage.try_exists()?,
        );
        // Claim and intent, or a saved tile waiting to be recovered, already
        // won. A stop does not abandon that tile. Anything earlier publishes
        // nothing new and starts no later tile.
        let admitted = matches!(
            action,
            PublicationAction::PublishStaged | PublicationAction::RecoverSaved
        );
        if !admitted && !crate::work_continues() {
            return Err(AudioArchiveError::Other("import stopped".to_owned()));
        }
        if action == PublicationAction::KeepSaved {
            if !crate::work_continues() {
                return Err(AudioArchiveError::Other("import stopped".to_owned()));
            }
            refresh_saved_availability(
                journal,
                &import_id,
                &mut store,
                &day,
                &key,
                &settings.stream,
            )?;
            continue;
        } else {
            if action == PublicationAction::Stage {
                // Nothing with this stage has been published. A partial pre-intent stage is disposable.
                if stage.try_exists()? {
                    fs::remove_dir_all(&stage)?;
                }
                fs::create_dir_all(&stage)?;
                let mut staged = Vec::with_capacity(parts.len());
                for part in &parts {
                    if !crate::work_continues() {
                        return Err(AudioArchiveError::Other("import stopped".to_owned()));
                    }
                    let member = store.member_for_part(part)?;
                    if store.part_is_rejected(part)? {
                        continue;
                    }
                    let target = stage.join(&part.stored_name);
                    match options.materializer.materialize(&member, part, &target) {
                        Ok(()) => {}
                        Err(AudioArchiveError::SourceRejected(cause)) => {
                            store.reject_member_remaining(part, cause)?;
                            if target.try_exists()? {
                                fs::remove_file(&target)?;
                            }
                            continue;
                        }
                        Err(error) => return Err(error),
                    }
                    if !fs::symlink_metadata(&target)?.file_type().is_file() {
                        return Err(AudioArchiveError::Other(
                            "materializer did not stage a plain file".to_owned(),
                        ));
                    }
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        fs::set_permissions(&target, fs::Permissions::from_mode(0o600))?;
                    }
                    File::options()
                        .read(true)
                        .write(true)
                        .open(&target)?
                        .sync_all()?;
                    staged.push(TileIntentPart {
                        stored_name: part.stored_name.clone(),
                        sha256: hash_file(&target)?,
                        byte_len: fs::metadata(&target)?.len(),
                    });
                }
                parts = store.publishable_tile_parts(&day, &key)?;
                let surviving: std::collections::HashSet<_> =
                    parts.iter().map(|part| part.stored_name.as_str()).collect();
                for rejected in staged
                    .iter()
                    .filter(|part| !surviving.contains(part.stored_name.as_str()))
                {
                    fs::remove_file(stage.join(&rejected.stored_name))?;
                }
                staged.retain(|part| surviving.contains(part.stored_name.as_str()));
                if parts.is_empty() {
                    fs::remove_dir_all(&stage)?;
                    continue;
                }
                if !crate::work_continues() {
                    return Err(AudioArchiveError::Other("import stopped".to_owned()));
                }
                let placement = placement_for(&parts);
                atomic_replace(
                    stage.join(AUDIO_ARCHIVE_PLACEMENT_FILENAME),
                    &serde_json::to_vec(&placement)?,
                    AtomicWriteOptions::default(),
                )
                .map_err(|error| AudioArchiveError::Other(error.to_string()))?;
                sync_stage_chain(journal, &import_id, &day, &key)?;
                store.set_tile_outcomes(
                    &day,
                    &key,
                    &parts
                        .iter()
                        .map(|part| (part.stored_name.clone(), PartOutcome::Staged))
                        .collect::<Vec<_>>(),
                    1,
                )?;
                check_fault(&seams, ArchiveFault::AfterTileBytesStaged)?;
                let identities: Vec<_> = parts
                    .iter()
                    .map(|part| {
                        let member = store.member_for_part(part)?;
                        Ok(MemberIdentity::new(
                            member.byte_len,
                            member.sha256,
                            member.canonical_occurrence,
                        ))
                    })
                    .collect::<Result<_, AudioArchiveError>>()?;
                insert_tile_claims(journal, &import_id, &identities, DEFAULT_WRITE_BUSY_TIMEOUT)?;
                fs::create_dir_all(intent_path.parent().expect("tile intent parent"))?;
                let intent = TileIntent {
                    day: day.clone(),
                    segment_key: key.clone(),
                    stream: settings.stream.clone(),
                    placement,
                    parts: staged,
                };
                atomic_replace(
                    &intent_path,
                    &serde_json::to_vec(&intent)?,
                    AtomicWriteOptions::default(),
                )
                .map_err(|error| AudioArchiveError::Other(error.to_string()))?;
                sync_directory(journal, &format!("imports/{import_id}"))?;
            }
            register_pending_tile(journal, &import_id, &day, &key)?;
            sync_directory(journal, "imports")?;
            check_fault(&seams, ArchiveFault::AfterClaimAndIntent)?;
            check_fault(&seams, ArchiveFault::BeforeTilePublish)?;
            recover_or_publish_tile(journal, &import_id, &mut store, &day, &key, true, &seams)?;
        }
        tiles_saved += 1;
        report_progress(&seams, &store, &import_id, ArchiveProgressPhase::Saved)?;
        if seams
            .as_ref()
            .is_some_and(|seams| seams.fault == Some(ArchiveFault::BetweenMultiTileParts))
        {
            for part in &parts {
                if store.has_other_unsaved_parts(part)? {
                    check_fault(&seams, ArchiveFault::BetweenMultiTileParts)?;
                }
            }
        }
        check_fault(&seams, ArchiveFault::AfterSavedMarkBeforeEmit)?;
        // A single wake is sufficient: discovery reads the durable, bounded frontier.
        if tiles_saved == 1
            && let Some(emit) = seams
                .as_ref()
                .and_then(|seams| seams.emit_observed.as_ref())
        {
            emit(&day, &key, &settings.stream);
        }
    }
    store.finish_saving()?;
    let final_status = store.status()?;
    record_completed_attempt(journal, &import_id, generation, now_ms(), None, None)
        .map_err(|error| AudioArchiveError::Other(error.to_string()))?;
    let record = read_audio_archive(journal, &import_id)?
        .ok_or_else(|| AudioArchiveError::MissingRunDirectory(run))?;
    Ok(AudioArchivePublishResult {
        import_id,
        plan_size,
        tiles_saved: final_status
            .saved_tiles
            .saturating_sub(initial_status.saved_tiles) as usize,
        parts_saved: final_status
            .saved_parts
            .saturating_sub(initial_status.saved_parts) as usize,
        record,
    })
}

fn report_progress(
    seams: &Option<AudioArchivePublishSeams>,
    store: &ArchiveStore,
    import_id: &str,
    phase: ArchiveProgressPhase,
) -> Result<(), AudioArchiveError> {
    if let Some(progress) = seams.as_ref().and_then(|seams| seams.progress.as_ref()) {
        progress(&AudioArchiveProgress {
            import_id: import_id.to_owned(),
            phase,
            status: store.status()?,
        })?;
    }
    Ok(())
}

fn stage_path(journal: &Path, id: &str, day: &str, key: &str) -> PathBuf {
    journal
        .join("imports")
        .join(id)
        .join("staged")
        .join(day)
        .join(key)
}
fn intent_path(journal: &Path, id: &str, day: &str, key: &str) -> PathBuf {
    journal
        .join("imports")
        .join(id)
        .join("tiles")
        .join(format!("{day}_{key}.intent.json"))
}
fn placement_for(parts: &[AudioArchiveRecordPart]) -> AudioArchivePlacement {
    AudioArchivePlacement {
        version: AUDIO_ARCHIVE_PLACEMENT_VERSION,
        parts: parts
            .iter()
            .map(|part| AudioArchivePlacementPart {
                stored_name: part.stored_name.clone(),
                original_occurrence: part.member_occurrence.clone(),
                original_time_ms: part
                    .member_occurrence
                    .strip_prefix("utc|")
                    .and_then(|value| value.parse().ok()),
                origin_ms: part.origin_ms,
                offset_ms: part.offset_ms,
                length_ms: part.length_ms,
            })
            .collect(),
    }
}

/// Recovery discovery may finish bookkeeping, but never creates chronicle data.
/// A missing intent-bound stage witnesses an earlier consuming publication.
pub fn recover_published_tile(
    journal: &Path,
    id: &str,
    store: &mut ArchiveStore,
    day: &str,
    key: &str,
) -> Result<bool, AudioArchiveError> {
    recover_published_tile_checked(journal, id, store, day, key, &|| Ok(()))
}

pub(crate) fn recover_published_tile_checked(
    journal: &Path,
    id: &str,
    store: &mut ArchiveStore,
    day: &str,
    key: &str,
    checkpoint: &dyn Fn() -> Result<(), String>,
) -> Result<bool, AudioArchiveError> {
    checkpoint().map_err(AudioArchiveError::Other)?;
    super::record::validate_coordinates(day, key)?;
    if store.tile_phase(day, key)? == 2 {
        let stream = store.settings()?.stream;
        refresh_saved_availability_checked(journal, id, store, day, key, &stream, checkpoint)?;
        return Ok(true);
    }
    let stage = stage_path(journal, id, day, key);
    match fs::symlink_metadata(&stage) {
        Ok(metadata) if metadata.file_type().is_dir() => return Ok(false),
        Ok(_) => {
            return Err(AudioArchiveError::Other(
                "archive stage is not a directory".to_owned(),
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    if !intent_path(journal, id, day, key).try_exists()? {
        return Ok(false);
    }
    recover_or_publish_tile_checked(journal, id, store, day, key, false, &None, checkpoint)?;
    Ok(true)
}

fn recover_or_publish_tile(
    journal: &Path,
    id: &str,
    store: &mut ArchiveStore,
    day: &str,
    key: &str,
    allow_publish: bool,
    seams: &Option<AudioArchivePublishSeams>,
) -> Result<(), AudioArchiveError> {
    recover_or_publish_tile_checked(journal, id, store, day, key, allow_publish, seams, &|| {
        Ok(())
    })
}

#[allow(clippy::too_many_arguments)]
fn recover_or_publish_tile_checked(
    journal: &Path,
    id: &str,
    store: &mut ArchiveStore,
    day: &str,
    key: &str,
    allow_publish: bool,
    seams: &Option<AudioArchivePublishSeams>,
    checkpoint: &dyn Fn() -> Result<(), String>,
) -> Result<(), AudioArchiveError> {
    checkpoint().map_err(AudioArchiveError::Other)?;
    let parts = store.publishable_tile_parts(day, key)?;
    let stream = store.settings()?.stream;
    let intent: TileIntent =
        serde_json::from_slice(&fs::read(intent_path(journal, id, day, key))?)?;
    if intent.day != day
        || intent.segment_key != key
        || intent.stream != stream
        || intent.placement != placement_for(&parts)
        || intent.parts.len() != parts.len()
        || !intent
            .parts
            .iter()
            .zip(&parts)
            .all(|(staged, part)| staged.stored_name == part.stored_name)
    {
        return Err(AudioArchiveError::ClaimDatabaseCorrupt(
            "tile intent differs from the saved plan".to_owned(),
        ));
    }
    let stage = stage_path(journal, id, day, key);
    let target = journal.join("chronicle").join(day).join(&stream).join(key);
    let stage_exists = match fs::symlink_metadata(&stage) {
        Ok(metadata) if metadata.file_type().is_dir() => true,
        Ok(_) => {
            return Err(AudioArchiveError::Other(
                "archive stage is not a directory".to_owned(),
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) => return Err(error.into()),
    };
    if stage_exists {
        if !allow_publish {
            return Ok(());
        }
        for part in &intent.parts {
            let path = stage.join(&part.stored_name);
            if !fs::symlink_metadata(&path)?.file_type().is_file()
                || fs::metadata(&path)?.len() != part.byte_len
                || hash_file(&path)? != part.sha256
            {
                return Err(AudioArchiveError::Other(
                    "intent-bound archive stage changed".to_owned(),
                ));
            }
        }
        solstone_core_journal_io::ensure_directory(target.parent().expect("segment parent"))
            .map_err(|error| AudioArchiveError::Other(error.to_string()))?;
        let _exclusion = SegmentMutation::acquire(&target, LockOptions::default())?;
        checkpoint().map_err(AudioArchiveError::Other)?;
        match fs::symlink_metadata(&target) {
            Ok(_) => {
                return Err(AudioArchiveError::ChronicleSegmentRefused(format!(
                    "tile already exists: {day}/{stream}/{key}"
                )));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        checkpoint().map_err(AudioArchiveError::Other)?;
        fs::rename(&stage, &target)?;
        sync_directory(journal, &format!("imports/{id}/staged/{day}"))?;
        for directory in [
            format!("chronicle/{day}/{stream}"),
            format!("chronicle/{day}"),
            "chronicle".to_owned(),
        ] {
            sync_directory(journal, &directory)?;
        }
        #[cfg(unix)]
        solstone_core_journal_io::sync_root(journal)
            .map_err(|error| AudioArchiveError::Other(error.to_string()))?;
        advance_stream(journal, &stream, day, key, checkpoint)?;
        check_fault(seams, ArchiveFault::AfterPublishBeforeSavedMark)?;
    } else if target.try_exists()? {
        // Synchronize with a writer that just consumed its stage before recording Saved.
        let _exclusion = SegmentMutation::acquire(&target, LockOptions::default())?;
        checkpoint().map_err(AudioArchiveError::Other)?;
        let placement: AudioArchivePlacement =
            serde_json::from_slice(&fs::read(target.join(AUDIO_ARCHIVE_PLACEMENT_FILENAME))?)?;
        if placement != intent.placement {
            return Err(AudioArchiveError::ClaimDatabaseCorrupt(
                "published tile differs from its intent".to_owned(),
            ));
        }
        sync_directory(journal, &format!("imports/{id}/staged/{day}"))?;
        sync_directory(journal, &format!("chronicle/{day}/{stream}"))?;
        advance_stream(journal, &stream, day, key, checkpoint)?;
    }
    refresh_saved_availability_checked(journal, id, store, day, key, &stream, checkpoint)
}

fn advance_stream(
    journal: &Path,
    stream: &str,
    day: &str,
    key: &str,
    checkpoint: &dyn Fn() -> Result<(), String>,
) -> Result<(), AudioArchiveError> {
    let qualifier = stream.strip_prefix("import.").unwrap_or(stream).to_owned();
    advance_unbound_stream_checked(
        journal,
        stream,
        day,
        key,
        StreamHints {
            kind: Some(Kind::Imported(ImportSource::Named(qualifier))),
            host: None,
            platform: None,
        },
        checkpoint,
    )
    .map_err(|error| AudioArchiveError::Other(error.to_string()))?;
    Ok(())
}
fn refresh_saved_availability(
    journal: &Path,
    _id: &str,
    store: &mut ArchiveStore,
    day: &str,
    key: &str,
    stream: &str,
) -> Result<(), AudioArchiveError> {
    refresh_saved_availability_checked(journal, _id, store, day, key, stream, &|| Ok(()))
}

fn refresh_saved_availability_checked(
    journal: &Path,
    _id: &str,
    store: &mut ArchiveStore,
    day: &str,
    key: &str,
    stream: &str,
    checkpoint: &dyn Fn() -> Result<(), String>,
) -> Result<(), AudioArchiveError> {
    checkpoint().map_err(AudioArchiveError::Other)?;
    let planned = store.publishable_tile_parts(day, key)?;
    let placement_path = journal
        .join("chronicle")
        .join(day)
        .join(stream)
        .join(key)
        .join(AUDIO_ARCHIVE_PLACEMENT_FILENAME);
    let placement_available = match fs::symlink_metadata(&placement_path) {
        Ok(metadata) if metadata.file_type().is_file() => {
            let placement: AudioArchivePlacement =
                serde_json::from_slice(&fs::read(&placement_path)?).map_err(|error| {
                    AudioArchiveError::ChronicleSegmentRefused(error.to_string())
                })?;
            if placement != placement_for(&planned) {
                return Err(AudioArchiveError::ChronicleSegmentRefused(
                    "saved placement differs from the archive plan".to_owned(),
                ));
            }
            true
        }
        Ok(_) => false,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) => return Err(error.into()),
    };
    let mut outcomes = Vec::new();
    for part in planned {
        let raw = journal
            .join("chronicle")
            .join(day)
            .join(stream)
            .join(key)
            .join(&part.stored_name);
        let available = match fs::symlink_metadata(raw) {
            Ok(metadata) => metadata.file_type().is_file(),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
            Err(error) => return Err(error.into()),
        };
        outcomes.push((
            part.stored_name,
            if placement_available && available {
                PartOutcome::Saved
            } else {
                PartOutcome::SavedButUnavailable
            },
        ));
    }
    store.set_tile_outcomes_checked(day, key, &outcomes, 2, checkpoint)?;
    for (name, outcome) in outcomes {
        if outcome == PartOutcome::SavedButUnavailable {
            store.finish_processing_checked(day, key, &name, true, checkpoint)?;
        }
    }
    Ok(())
}
fn sync_directory(journal: &Path, relative: &str) -> Result<(), AudioArchiveError> {
    solstone_core_journal_io::sync_dir(journal, relative)
        .map_err(|error| AudioArchiveError::Other(error.to_string()))
}
fn sync_stage_chain(
    journal: &Path,
    id: &str,
    day: &str,
    key: &str,
) -> Result<(), AudioArchiveError> {
    for relative in [
        format!("imports/{id}/staged/{day}/{key}"),
        format!("imports/{id}/staged/{day}"),
        format!("imports/{id}/staged"),
        format!("imports/{id}"),
        "imports".to_owned(),
    ] {
        sync_directory(journal, &relative)?;
    }
    Ok(())
}
fn check_fault(
    seams: &Option<AudioArchivePublishSeams>,
    expected: ArchiveFault,
) -> Result<(), AudioArchiveError> {
    if seams
        .as_ref()
        .is_some_and(|seams| seams.fault == Some(expected))
    {
        Err(AudioArchiveError::Fault(expected))
    } else {
        Ok(())
    }
}
fn start_run_heartbeat(
    journal: &Path,
    id: &str,
    generation: u64,
    attempt_id: &str,
    seams: &Option<AudioArchivePublishSeams>,
) -> ImportHeartbeat {
    if let Some(spawner) = seams.as_ref().and_then(|seams| seams.heartbeat_spawner) {
        return ImportHeartbeat::start_injected(
            None,
            Arc::new(|_| {}),
            spawner,
            Arc::new(|| {}),
            Arc::new(|| {}),
            id.to_owned(),
            generation,
            attempt_id.to_owned(),
            "audio_archive".to_owned(),
            Instant::now(),
        );
    }
    ImportHeartbeat::start(
        Some(Duration::from_secs(1)),
        journal.to_path_buf(),
        id.to_owned(),
        generation,
        attempt_id.to_owned(),
        "audio_archive".to_owned(),
        Instant::now(),
    )
}
fn hash_file(path: &Path) -> Result<String, AudioArchiveError> {
    let mut file = File::open(path)?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 65536];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    Ok(format!("{:x}", digest.finalize()))
}
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
impl AudioArchiveMaterializer for HashMap<String, Vec<u8>> {
    fn materialize(
        &self,
        member: &AudioArchiveRecordMember,
        _part: &AudioArchiveRecordPart,
        target: &Path,
    ) -> Result<(), AudioArchiveError> {
        let bytes = self
            .get(&member.sha256)
            .or_else(|| self.get(&member.original_relative_name))
            .ok_or_else(|| AudioArchiveError::Other("missing fixture member".to_owned()))?;
        let actual = format!("{:x}", Sha256::digest(bytes));
        if bytes.len() as u64 != member.byte_len || actual != member.sha256 {
            return Err(AudioArchiveError::HashMismatch {
                expected: member.sha256.clone(),
                actual,
            });
        }
        fs::write(target, bytes)?;
        Ok(())
    }
}

#[cfg(all(unix, test, feature = "full-tests"))]
mod crash_tests {
    use super::*;
    use crate::audio_archive::claims::claim_db_path;
    use crate::audio_archive::identity::MemberStartEvidence;
    use crate::audio_archive::plan::DurationInput;
    use tempfile::TempDir;

    #[test]
    fn crash_seam_recovery_leaves_one_claim_and_staged_bytes() {
        let temp = TempDir::new().unwrap();
        let journal = temp.path();

        let member_bytes = b"synthetic test audio bytes".to_vec();
        let sha256 = format!("{:x}", Sha256::digest(&member_bytes));
        let member = AudioArchiveMemberInput {
            original_relative_name: "archive/test_crash.ogg".to_owned(),
            byte_len: member_bytes.len() as u64,
            sha256: sha256.clone(),
            duration: DurationInput::Milliseconds(10_000),
            start: MemberStartEvidence {
                instant: Some("2026-01-14T21:00:00.500Z".to_owned()),
                local_civil: None,
                source_zone: None,
                ..Default::default()
            },
            run_note: None,
        };
        let mut map = HashMap::new();
        map.insert(sha256, member_bytes.clone());

        let seams = AudioArchivePublishSeams {
            fault: Some(ArchiveFault::AfterClaimAndIntent),
            emit_observed: None,
            heartbeat_spawner: Some(|_, _| Ok(std::thread::spawn(|| {}))),
            progress: None,
        };
        let fixed_ts = validate_timestamp("20260114_210000").unwrap();
        let res = publish_audio_archive(
            journal,
            AudioArchivePublishOptions {
                import_id: None,
                members: vec![member],
                materializer: &map,
                owner_zone: Tz::America__New_York,
                settings: AudioArchiveSettings {
                    qualifier: "clips".to_owned(),
                    archive_identity: "crash_test_archive".to_owned(),
                },
                source_timestamp: Some(fixed_ts),
            },
            Some(seams),
        );
        assert!(matches!(
            res,
            Err(AudioArchiveError::Fault(ArchiveFault::AfterClaimAndIntent))
        ));

        let claim_path = claim_db_path(journal);
        let conn = rusqlite::Connection::open(&claim_path).unwrap();
        let count: i64 = conn
            .query_row("SELECT count(*) FROM audio_archive_claim", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, 1);

        let record = read_audio_archive(journal, "20260114_210000")
            .unwrap()
            .expect("audio-archive.json should exist");
        assert_eq!(record.members.len(), 1);

        let part = &record.parts[0];
        let staged_path = journal
            .join("imports")
            .join("20260114_210000")
            .join("staged")
            .join(&part.day)
            .join(&part.segment_key)
            .join(&part.stored_name);
        assert!(staged_path.is_file());
        assert_eq!(std::fs::read(&staged_path).unwrap(), member_bytes);
    }
}

#[cfg(test)]
mod publication_policy_tests {
    use super::*;
    #[test]
    fn saved_record_and_consumed_stage_both_forbid_new_publication() {
        assert_eq!(
            publication_action(true, false, false),
            PublicationAction::KeepSaved
        );
        assert_eq!(
            publication_action(false, true, false),
            PublicationAction::RecoverSaved
        );
    }
    #[test]
    fn interrupted_private_staging_is_distinct_from_a_complete_unpublished_stage() {
        assert_eq!(
            publication_action(false, false, true),
            PublicationAction::Stage
        );
        assert_eq!(
            publication_action(false, true, true),
            PublicationAction::PublishStaged
        );
    }
}
