// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The owner-voice setup lifecycle.
//!
//! The setup state lives in the `voiceprint` member of
//! `awareness/current.json`. A saved voice is `owner_centroid.npz`; a voice
//! waiting for review is `awareness/owner_candidate.npz`. This module saves,
//! rejects and undoes a voice, runs the scheduled look for one, and follows
//! the update of past speaker labels that saving or undoing a voice starts.
//!
//! The past-label update is the ordinary speaker backfill with reattribution
//! and without voiceprint accumulation: it rewrites speaker labels only, and
//! labels the owner set by hand stay.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::sync::Arc;

use chrono::{DateTime, Duration, Utc};
use serde_json::{Map, Value, json};
use solstone_core_journal_io::{JsonWriteOptions, LockOptions, hold_lock, write_json};

use crate::backfill_coordinator::{
    StartBackfillRequest, backfill_status, run_backfill_blocking_checked, start_backfill,
};
use crate::backfill_operations::BackfillStatusKind;
use crate::owner_admission::{OwnerAdmission, admitted_owner_id};
use crate::owner_candidate::{
    clear_owner_candidate_in_lock, hold_owner_candidate_lock, load_owner_candidate,
};
use crate::owner_centroid::{load_owner_centroid, owner_centroid_file, remove_owner_centroid};
#[cfg(test)]
use crate::owner_detection::detect_owner_candidate;
use crate::owner_detection::detect_owner_candidate_checked;

/// Days a rejected voice waits before the journal looks for another.
pub const COOLDOWN_DAYS: i64 = 14;
/// How many rejected voices are remembered so they are not offered again.
const REJECTED_VOICES_KEPT: usize = 8;

/// The owner-voice setup state recorded in `awareness/current.json`.
pub fn voiceprint_state(root: &Path) -> BTreeMap<String, Value> {
    fs::read(root.join("awareness/current.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .and_then(|current| {
            current
                .get("voiceprint")
                .and_then(Value::as_object)
                .cloned()
        })
        .map(|voiceprint| voiceprint.into_iter().collect())
        .unwrap_or_default()
}

/// Merge `update` into the recorded setup state. A null value removes its key.
pub fn update_voiceprint_state(root: &Path, update: Value) -> Result<(), String> {
    update_voiceprint_state_checked(root, update, &|| Ok(()))
}

pub fn update_voiceprint_state_checked(
    root: &Path,
    update: Value,
    checkpoint: &dyn Fn() -> Result<(), String>,
) -> Result<(), String> {
    let path = root.join("awareness/current.json");
    let _lock = hold_lock(&path, LockOptions::default()).map_err(|error| error.to_string())?;
    let mut state = fs::read(&path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();
    let voiceprint = state
        .entry("voiceprint".to_owned())
        .or_insert_with(|| Value::Object(Map::new()));
    let object = voiceprint
        .as_object_mut()
        .ok_or_else(|| "awareness voiceprint state is not an object".to_owned())?;
    for (key, value) in update.as_object().cloned().unwrap_or_default() {
        if value.is_null() {
            object.remove(&key);
        } else {
            object.insert(key, value);
        }
    }
    checkpoint()?;
    write_json(path, &Value::Object(state), JsonWriteOptions::default())
        .map_err(|error| error.to_string())
}

/// Days left before a rejected voice's cooldown ends, if one is running.
pub fn cooldown_days_remaining(state: &BTreeMap<String, Value>) -> Option<i64> {
    let rejected_at = rejected_at(state)?;
    let elapsed = Utc::now().signed_duration_since(rejected_at).num_days();
    (elapsed < COOLDOWN_DAYS).then_some(COOLDOWN_DAYS - elapsed)
}

/// When the journal looks for the owner's voice again after a rejection.
pub fn next_look_after(state: &BTreeMap<String, Value>) -> Option<DateTime<Utc>> {
    cooldown_days_remaining(state)?;
    Some(rejected_at(state)? + Duration::days(COOLDOWN_DAYS))
}

fn rejected_at(state: &BTreeMap<String, Value>) -> Option<DateTime<Utc>> {
    let value = state.get("rejected_at")?.as_str()?;
    DateTime::parse_from_rfc3339(value)
        .map(|time| time.with_timezone(&Utc))
        .ok()
}

/// Why a voice-setup action could not run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OwnerVoiceError {
    IdentityInvalid,
    NoCandidate,
    StaleCandidate,
    NoSavedVoice,
    Failed(String),
}

impl std::fmt::Display for OwnerVoiceError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::IdentityInvalid => {
                formatter.write_str("configured owner identity is not admitted")
            }
            Self::NoCandidate => formatter.write_str("No candidate available"),
            Self::StaleCandidate => {
                formatter.write_str("Candidate version mismatch or stale generation")
            }
            Self::NoSavedVoice => formatter.write_str("No owner voice is saved"),
            Self::Failed(detail) => formatter.write_str(detail),
        }
    }
}

impl std::error::Error for OwnerVoiceError {}

fn failed(error: impl std::fmt::Display) -> OwnerVoiceError {
    OwnerVoiceError::Failed(error.to_string())
}

fn admitted_principal(root: &Path) -> Result<String, OwnerVoiceError> {
    match admitted_owner_id(root) {
        OwnerAdmission::Admitted(id) => Ok(id),
        OwnerAdmission::Invalid => Err(OwnerVoiceError::IdentityInvalid),
    }
}

/// Record a newly saved owner voice and start updating past speaker labels.
///
/// `kind` says how the voice was saved: `confirm` (a reviewed candidate),
/// `teach` (statements the owner marked as theirs) or `redo` (a rebuilt voice).
pub fn record_owner_voice_saved(
    root: &Path,
    kind: &str,
    cluster_size: usize,
    evidence_tier: &str,
) -> Result<Value, String> {
    update_voiceprint_state(
        root,
        json!({"status":"confirmed","cluster_size":cluster_size,"confirmed_at":Utc::now().to_rfc3339(),"evidence_tier":evidence_tier,"source":kind}),
    )?;
    Ok(start_relabel(root, kind))
}

/// Reject the candidate the owner reviewed.
///
/// The voice is remembered so it is not offered again, and the journal waits
/// out the cooldown before it looks for another.
pub fn reject_owner_candidate(root: &Path, version: &str) -> Result<Value, OwnerVoiceError> {
    admitted_principal(root)?;
    let _lock = hold_owner_candidate_lock(root).map_err(failed)?;
    let candidate = load_owner_candidate(root)
        .map_err(failed)?
        .ok_or(OwnerVoiceError::NoCandidate)?;
    let state = voiceprint_state(root);
    if version != candidate.version
        || state.get("detected_at").and_then(Value::as_str) != Some(candidate.version.as_str())
    {
        return Err(OwnerVoiceError::StaleCandidate);
    }
    let now = Utc::now();
    let rejected = remember_rejected_voice(
        &state,
        state.get("candidate_cluster_id").and_then(Value::as_i64),
        &candidate.centroid,
        now,
    );
    clear_owner_candidate_in_lock(root).map_err(failed)?;
    update_voiceprint_state(
        root,
        json!({"status":"rejected","rejected_at":now.to_rfc3339(),"detected_at":null,"samples":null,"candidate_cluster_id":null,"rejected_candidates":rejected}),
    )
    .map_err(OwnerVoiceError::Failed)?;
    Ok(
        json!({"status":"rejected","cooldown_days":COOLDOWN_DAYS,"next_look_after":(now + Duration::days(COOLDOWN_DAYS)).to_rfc3339()}),
    )
}

/// Remove the owner's saved voice and return past speaker labels to unnamed.
///
/// Labels the owner set by hand stay, and so do the people they named and
/// those people's voices. The removed voice is remembered so it is not offered
/// again. A saved voice that can't be read is removed as well.
pub fn undo_owner_voice(root: &Path) -> Result<Value, OwnerVoiceError> {
    let principal = admitted_principal(root)?;
    let _lock = hold_owner_candidate_lock(root).map_err(failed)?;
    if !owner_centroid_file(root, &principal)
        .map_err(failed)?
        .exists()
    {
        return Err(OwnerVoiceError::NoSavedVoice);
    }
    // A saved voice that can't be read is removed too: undo is how the owner
    // recovers from it. Only a readable one can be remembered.
    let centroid = load_owner_centroid(root, &principal).ok().flatten();
    remove_owner_centroid(root, &principal).map_err(failed)?;
    let state = voiceprint_state(root);
    let rejected = match &centroid {
        Some(centroid) => remember_rejected_voice(&state, None, &centroid.centroid, Utc::now()),
        None => state
            .get("rejected_candidates")
            .cloned()
            .unwrap_or_else(|| json!([])),
    };
    update_voiceprint_state(
        root,
        json!({"status":"none","undone_at":Utc::now().to_rfc3339(),"confirmed_at":null,"cluster_size":null,"evidence_tier":null,"streams_represented":null,"recommendation":null,"detected_at":null,"samples":null,"source":null,"candidate_cluster_id":null,"rejected_candidates":rejected}),
    )
    .map_err(OwnerVoiceError::Failed)?;
    Ok(json!({"status":"none","relabel":start_relabel(root, "undo")}))
}

fn remember_rejected_voice(
    state: &BTreeMap<String, Value>,
    cluster_id: Option<i64>,
    centroid: &[f32],
    at: DateTime<Utc>,
) -> Value {
    let mut voices = state
        .get("rejected_candidates")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    voices.push(json!({"cluster_id":cluster_id,"centroid":centroid,"rejected_at":at.to_rfc3339()}));
    let excess = voices.len().saturating_sub(REJECTED_VOICES_KEPT);
    Value::Array(voices.split_off(excess))
}

fn relabel_request(operation_id: Option<String>) -> StartBackfillRequest {
    StartBackfillRequest {
        operation_id,
        commit: true,
        reattribute: true,
        accumulation: false,
        now_ms: Utc::now().timestamp_millis(),
    }
}

/// Start the past-label update in the background.
///
/// The request is recorded first, so an update that cannot start now is
/// started by the next scheduled look.
fn start_relabel(root: &Path, kind: &str) -> Value {
    let requested = json!({"kind":kind,"requested_at":Utc::now().to_rfc3339()});
    if let Err(error) = update_voiceprint_state(root, json!({"relabel":requested})) {
        log::error!("owner voice: could not record the past-label update: {error}");
        return json!({"kind":kind,"state":"waiting"});
    }
    match start_backfill(root, &relabel_request(None)) {
        Ok(status) => {
            let mut recorded = requested.clone();
            recorded["operation_id"] = json!(status.operation_id);
            if let Err(error) = update_voiceprint_state(root, json!({"relabel":recorded})) {
                log::error!("owner voice: could not record the past-label update: {error}");
            }
            relabel_progress(root).unwrap_or_else(|| json!({"kind":kind,"state":"waiting"}))
        }
        Err(error) => {
            log::error!("owner voice: the past-label update did not start: {error}");
            json!({"kind":kind,"state":"waiting"})
        }
    }
}

/// How far the past-label update has got, if one was requested.
pub fn relabel_progress(root: &Path) -> Option<Value> {
    let relabel = voiceprint_state(root).get("relabel")?.as_object()?.clone();
    let kind = relabel.get("kind").cloned().unwrap_or(Value::Null);
    let Some(operation_id) = relabel.get("operation_id").and_then(Value::as_str) else {
        return Some(json!({"kind":kind,"state":"waiting","completed":0,"total":0}));
    };
    let status = match backfill_status(root, operation_id) {
        Ok(Some(status)) => status,
        Ok(None) => return Some(json!({"kind":kind,"state":"waiting","completed":0,"total":0})),
        Err(error) => {
            log::error!("owner voice: could not read the past-label update: {error}");
            return None;
        }
    };
    let state = match status.status {
        BackfillStatusKind::ActivePreparing | BackfillStatusKind::ActiveRunning => "running",
        BackfillStatusKind::Done => "done",
        BackfillStatusKind::ResumableMemberErrors | BackfillStatusKind::AttemptFailed => {
            "incomplete"
        }
        BackfillStatusKind::NotFound
        | BackfillStatusKind::InactiveAccepted
        | BackfillStatusKind::InactiveInterrupted => "waiting",
    };
    Some(json!({
        "kind": kind,
        "state": state,
        "completed": status.completed_count,
        "total": status.total_count,
        "not_updated": status.error_count,
    }))
}

/// Finish a past-label update that stopped or never started, on this thread.
fn finish_pending_relabel_checked(
    root: &Path,
    checkpoint: &Arc<dyn Fn() -> Result<(), String> + Send + Sync>,
) -> Result<Value, String> {
    let state = voiceprint_state(root);
    let Some(relabel) = state.get("relabel").and_then(Value::as_object) else {
        return Ok(Value::Null);
    };
    let operation_id = relabel
        .get("operation_id")
        .and_then(Value::as_str)
        .map(str::to_owned);
    if let Some(operation_id) = &operation_id
        && let Ok(Some(status)) = backfill_status(root, operation_id)
        && matches!(
            status.status,
            BackfillStatusKind::Done
                | BackfillStatusKind::ActivePreparing
                | BackfillStatusKind::ActiveRunning
        )
    {
        return Ok(Value::Null);
    }
    let status =
        run_backfill_blocking_checked(root, &relabel_request(operation_id.clone()), checkpoint)
            .map_err(|error| error.to_string())?;
    if operation_id.is_none() {
        let mut recorded = relabel.clone();
        recorded.insert("operation_id".to_owned(), json!(status.operation_id));
        update_voiceprint_state_checked(root, json!({"relabel": recorded}), checkpoint.as_ref())?;
    }
    Ok(json!({"operation_id":status.operation_id,"done":status.done}))
}

/// The scheduled look for the owner's voice, run by the daily pool catch-up.
///
/// With a saved voice this changes nothing, unless a past-label update that
/// saving or undoing a voice started has not finished, in which case it
/// finishes it. Without one it finishes any such update and then looks for a
/// candidate the same way the Speakers page does, including after an earlier
/// look found none.
pub fn refresh_owner_voice(root: &Path) -> Result<Value, String> {
    let admit: Arc<dyn Fn() -> Result<(), String> + Send + Sync> = Arc::new(|| Ok(()));
    refresh_owner_voice_checked(root, &admit)
}

pub fn refresh_owner_voice_checked(
    root: &Path,
    checkpoint: &Arc<dyn Fn() -> Result<(), String> + Send + Sync>,
) -> Result<Value, String> {
    let principal = match admitted_owner_id(root) {
        OwnerAdmission::Admitted(id) => id,
        OwnerAdmission::Invalid => {
            return Ok(json!({"status":"skipped","reason":"identity_invalid"}));
        }
    };
    let relabel = finish_pending_relabel_checked(root, checkpoint)?;
    let voice_saved = owner_centroid_file(root, &principal)
        .map_err(|error| error.to_string())?
        .exists();
    if voice_saved {
        return Ok(json!({"status":"skipped","reason":"voice_saved","relabel":relabel}));
    }
    let detected = detect_owner_candidate_checked(root, false, checkpoint.as_ref())?;
    Ok(json!({
        "status": "looked",
        "owner_voice": detected.get("status").cloned().unwrap_or(Value::Null),
        "reason": detected.get("reason").or_else(|| detected.get("low_quality_reason")).cloned().unwrap_or(Value::Null),
        "relabel": relabel,
    }))
}

/// Where owner-voice setup stands, for surfaces that prompt the owner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OwnerVoiceSetup {
    /// No admitted owner identity, so there is no one to set up.
    IdentityInvalid,
    /// A voice is saved.
    Saved,
    /// A voice that may be the owner's is waiting for review.
    CandidateReady,
    /// No voice yet.
    NotYet(NotYetReason),
}

/// Why there is no owner voice yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NotYetReason {
    /// Not enough of the owner's voice has been heard yet.
    NeedsMoreAudio,
    /// What has been heard is too short, or too mixed with other voices.
    TooShortOrMixed,
    /// The owner said no to a voice; the journal looks again after this time.
    Declined { next_look_after: DateTime<Utc> },
}

/// Read where owner-voice setup stands. Never writes.
pub fn owner_voice_setup(root: &Path) -> OwnerVoiceSetup {
    let principal = match admitted_owner_id(root) {
        OwnerAdmission::Admitted(id) => id,
        OwnerAdmission::Invalid => return OwnerVoiceSetup::IdentityInvalid,
    };
    if owner_centroid_file(root, &principal).is_ok_and(|path| path.exists()) {
        return OwnerVoiceSetup::Saved;
    }
    let state = voiceprint_state(root);
    let status = state.get("status").and_then(Value::as_str);
    if status == Some("candidate")
        && let Ok(Some(candidate)) = load_owner_candidate(root)
        && state.get("detected_at").and_then(Value::as_str) == Some(candidate.version.as_str())
    {
        return OwnerVoiceSetup::CandidateReady;
    }
    if let Some(next_look_after) = next_look_after(&state) {
        return OwnerVoiceSetup::NotYet(NotYetReason::Declined { next_look_after });
    }
    let reason = state.get("low_quality_reason").and_then(Value::as_str);
    if status == Some("low_quality")
        && matches!(
            reason,
            Some("median_duration_too_short" | "cluster_too_diffuse")
        )
    {
        return OwnerVoiceSetup::NotYet(NotYetReason::TooShortOrMixed);
    }
    OwnerVoiceSetup::NotYet(NotYetReason::NeedsMoreAudio)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::candidate_tracker::CandidateTracker;
    use crate::owner_centroid::{OwnerCentroidWriteInput, write_owner_centroid};
    use crate::transcribed_clusters::{TranscribedClusterLoad, load_transcribed_cluster_inputs};

    fn journal() -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join("entities/owner")).unwrap();
        fs::write(
            root.path().join("entities/owner/entity.json"),
            r#"{"id":"owner","name":"Owner","type":"Person","is_principal":true}"#,
        )
        .unwrap();
        root
    }

    fn voice(axis: usize) -> Vec<f32> {
        let mut values = vec![0.0_f32; 256];
        values[axis] = 1.0;
        values
    }

    /// A transcribed segment whose `statements` all carry `embedding`, two
    /// seconds apart.
    fn segment(root: &Path, day: &str, key: &str, embedding: &[f32], statements: usize) -> PathBuf {
        let dir = root.join("chronicle").join(day).join(key);
        fs::create_dir_all(&dir).unwrap();
        let payload_path = dir.join("payload.f32");
        let mut raw = Vec::new();
        for _ in 0..statements {
            for value in embedding {
                raw.extend_from_slice(&value.to_le_bytes());
            }
        }
        fs::write(&payload_path, raw).unwrap();
        let ids = (1..=statements as i64).collect::<Vec<_>>();
        let rows = (1..=statements as i64 + 1)
            .map(|id| json!({"id":id,"start_offset_us":(id - 1) * 2_000_000,"text":format!("statement {id}")}))
            .collect::<Vec<_>>();
        let request = json!({
            "schema": "solstone-speaker-transcript-write-request-v1",
            "output": {
                "jsonl_path": dir.join("audio.jsonl").display().to_string(),
                "npz_path": dir.join("audio.npz").display().to_string(),
                "redo": true,
            },
            "base_time_us_of_day": 100_000_u64,
            "source": "audio",
            "statements": rows,
            "header": {
                "raw": "audio.wav",
                "model": "model",
                "device": "cpu",
                "compute_type": "int8",
                "speaker_evidence": "single",
                "speaker_evidence_version": "windowed-slots-v1",
                "speaker_evidence_multi_fraction": 0.0,
            },
            "embeddings": {
                "payload_path": payload_path,
                "payload_format": "raw-f32le-row-major-v1",
                "dtype": "float32-le",
                "shape": [statements, 256],
                "byte_count": statements * 256 * 4,
                "statement_ids": ids,
                "durations_s": vec![2.0; statements],
                "encoder": "test",
            }
        });
        solstone_core_speaker_id::writer::write_request(
            serde_json::to_vec(&request).unwrap().as_slice(),
        )
        .unwrap();
        dir
    }

    fn feed_pool(root: &Path, segment: &Path) {
        let TranscribedClusterLoad::Clusters(inputs) = load_transcribed_cluster_inputs(
            root,
            &segment.join("audio.jsonl"),
            &segment.join("audio.npz"),
        )
        .unwrap() else {
            panic!("segment yields clusters");
        };
        CandidateTracker::new(root)
            .add_transcribed_clusters(&inputs)
            .unwrap();
    }

    /// Three segments of one voice: enough for a standard candidate.
    fn heard_voice(root: &Path, axis: usize) -> Vec<PathBuf> {
        ["093000_300", "103000_300", "113000_300"]
            .into_iter()
            .map(|key| {
                let segment = segment(root, "20260101", key, &voice(axis), 12);
                feed_pool(root, &segment);
                segment
            })
            .collect()
    }

    fn labels(segment: &Path) -> Value {
        serde_json::from_slice(&fs::read(segment.join("talents/speaker_labels.json")).unwrap())
            .unwrap()
    }

    fn save_voice(root: &Path, axis: usize) {
        write_owner_centroid(
            root,
            "owner",
            &OwnerCentroidWriteInput {
                centroid: voice(axis),
                cluster_size: 36,
                timestamp: Utc::now().to_rfc3339(),
                evidence_tier: "standard".to_owned(),
            },
        )
        .unwrap();
    }

    #[test]
    fn the_scheduled_look_tries_again_after_finding_no_voice() {
        let _backfill = crate::backfill_coordinator::COORDINATOR_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let root = journal();
        let first = refresh_owner_voice(root.path()).unwrap();
        assert_eq!(first["owner_voice"], "no_cluster", "{first}");
        assert_eq!(
            owner_voice_setup(root.path()),
            OwnerVoiceSetup::NotYet(NotYetReason::NeedsMoreAudio)
        );

        heard_voice(root.path(), 0);
        let second = refresh_owner_voice(root.path()).unwrap();
        assert_eq!(second["owner_voice"], "candidate", "{second}");
        assert_eq!(
            owner_voice_setup(root.path()),
            OwnerVoiceSetup::CandidateReady
        );
    }

    #[test]
    fn a_rejected_voice_waits_out_the_cooldown_and_is_not_offered_again() {
        let _backfill = crate::backfill_coordinator::COORDINATOR_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let root = journal();
        heard_voice(root.path(), 0);
        refresh_owner_voice(root.path()).unwrap();
        let version = voiceprint_state(root.path())["detected_at"]
            .as_str()
            .unwrap()
            .to_owned();

        assert_eq!(
            reject_owner_candidate(root.path(), "an older look"),
            Err(OwnerVoiceError::StaleCandidate)
        );
        let rejected = reject_owner_candidate(root.path(), &version).unwrap();
        assert_eq!(rejected["status"], "rejected");
        assert_eq!(rejected["cooldown_days"], COOLDOWN_DAYS);
        assert!(rejected["next_look_after"].is_string());

        let state = voiceprint_state(root.path());
        assert_eq!(state["status"], "rejected");
        assert!(!state.contains_key("detected_at"));
        assert_eq!(state["rejected_candidates"].as_array().unwrap().len(), 1);
        assert!(!root.path().join("awareness/owner_candidate.npz").exists());
        assert!(matches!(
            owner_voice_setup(root.path()),
            OwnerVoiceSetup::NotYet(NotYetReason::Declined { .. })
        ));

        // During the cooldown the scheduled look waits.
        let waiting = refresh_owner_voice(root.path()).unwrap();
        assert_eq!(waiting["reason"], "cooldown", "{waiting}");
        // Even when asked to look now, the same voice is not offered again.
        let forced = detect_owner_candidate(root.path(), true).unwrap();
        assert_eq!(forced["status"], "no_cluster", "{forced}");
        assert_eq!(forced["reason"], "no_eligible_candidate", "{forced}");
    }

    #[test]
    fn saving_a_voice_updates_past_speaker_labels() {
        let _backfill = crate::backfill_coordinator::COORDINATOR_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let root = journal();
        let segments = heard_voice(root.path(), 0);
        let past = &segments[0];
        solstone_core_speaker_id::labels::write_stub_labels(past, "no_owner_centroid").unwrap();
        assert!(labels(past)["labels"].as_array().unwrap().is_empty());

        save_voice(root.path(), 0);
        // The update that saving starts, finished by the scheduled look the way
        // it is after a restart.
        update_voiceprint_state(
            root.path(),
            json!({"status":"confirmed","relabel":{"kind":"confirm"}}),
        )
        .unwrap();
        let report = refresh_owner_voice(root.path()).unwrap();
        assert_eq!(report["status"], "skipped", "{report}");
        assert_eq!(report["relabel"]["done"], true, "{report}");

        let after = labels(past);
        assert!(
            after["labels"]
                .as_array()
                .unwrap()
                .iter()
                .any(|label| label["speaker"] == "owner"),
            "{after}"
        );
        assert_eq!(relabel_progress(root.path()).unwrap()["state"], "done");
    }

    #[test]
    fn with_no_saved_voice_the_past_label_update_keeps_only_labels_set_by_hand() {
        let _backfill = crate::backfill_coordinator::COORDINATOR_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let root = journal();
        let past = segment(root.path(), "20260101", "093000_300", &voice(0), 12);
        fs::create_dir_all(past.join("talents")).unwrap();
        fs::write(
            past.join("talents/speaker_labels.json"),
            json!({"labels":[
                {"sentence_id":1,"speaker":"owner","confidence":"high","method":"owner_centroid"},
                {"sentence_id":2,"speaker":"owner","confidence":"high","method":"user_assigned"}
            ]})
            .to_string(),
        )
        .unwrap();

        update_voiceprint_state(
            root.path(),
            json!({"status":"none","relabel":{"kind":"undo"}}),
        )
        .unwrap();
        refresh_owner_voice(root.path()).unwrap();

        let after = labels(&past);
        assert_eq!(after["skipped"], true, "{after}");
        assert_eq!(
            after["labels"],
            json!([{"sentence_id":2,"speaker":"owner","confidence":"high","method":"user_assigned"}])
        );
    }

    #[test]
    fn undoing_a_voice_returns_to_no_voice_and_does_not_offer_it_again() {
        let _backfill = crate::backfill_coordinator::COORDINATOR_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let root = journal();
        heard_voice(root.path(), 0);
        save_voice(root.path(), 0);
        fs::create_dir_all(root.path().join("entities/ada")).unwrap();
        fs::write(
            root.path().join("entities/ada/entity.json"),
            r#"{"id":"ada","name":"Ada","type":"Person"}"#,
        )
        .unwrap();

        assert_eq!(
            undo_owner_voice(&root.path().join("missing")),
            Err(OwnerVoiceError::IdentityInvalid)
        );
        let undone = undo_owner_voice(root.path()).unwrap();
        assert_eq!(undone["status"], "none");
        assert!(!owner_centroid_file(root.path(), "owner").unwrap().exists());
        let state = voiceprint_state(root.path());
        assert_eq!(state["status"], "none");
        assert_eq!(state["relabel"]["kind"], "undo");
        assert_eq!(state["rejected_candidates"].as_array().unwrap().len(), 1);
        assert!(root.path().join("entities/ada/entity.json").exists());
        assert!(matches!(
            owner_voice_setup(root.path()),
            OwnerVoiceSetup::NotYet(_)
        ));
        assert_eq!(
            undo_owner_voice(root.path()),
            Err(OwnerVoiceError::NoSavedVoice)
        );

        // The removed voice is not offered back as if it were new.
        let looked = detect_owner_candidate(root.path(), false).unwrap();
        assert_eq!(looked["reason"], "no_eligible_candidate", "{looked}");
    }

    #[test]
    fn a_saved_voice_that_cannot_be_read_can_still_be_removed() {
        let _backfill = crate::backfill_coordinator::COORDINATOR_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let root = journal();
        save_voice(root.path(), 0);
        let path = owner_centroid_file(root.path(), "owner").unwrap();
        fs::write(&path, b"not a voice").unwrap();
        let undone = undo_owner_voice(root.path()).unwrap();
        assert_eq!(undone["status"], "none");
        assert!(!path.exists());
    }
}
