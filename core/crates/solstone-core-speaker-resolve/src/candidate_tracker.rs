// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Durable cross-segment speaker candidate pool.

use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};
use solstone_core_entity::{
    load_all_journal_entities, save_voiceprints_batch, EncoderIdentity, VoiceprintItem,
};
use solstone_core_journal_io::{
    atomic_replace, hold_lock, AtomicWriteOptions, LockError, LockOptions,
};
use thiserror::Error;

use crate::person_guard::is_admissible_person;
use crate::voiceprint_metadata::VoiceprintMetadata;

// Verbatim from solstone/apps/speakers/encoder_config.py.
pub const SOLO_CLUSTER_MIN_COSINE: f32 = 0.43;
pub const MERGE_THRESHOLD: f32 = 0.72;
pub const SPLIT_THRESHOLD: f32 = 0.55;
pub const STABILITY_THRESHOLD: f32 = 0.25;
pub const CONSOLIDATE_MIN_INTERVALS: usize = 30;
pub const CONSOLIDATE_MERGE_THRESHOLD: f32 = 0.65;
pub const CONSOLIDATE_SUGGEST_MIN: f32 = 0.45;
pub const CONFIRM_MIN_SEGMENTS: usize = 2;
pub const CONFIRM_MIN_INTERVALS: usize = 5;
pub const CONFIRM_MIN_DURATION_S: f64 = 25.0;
pub const VP_OUTLIER_MIN_SAMPLES: usize = 5;
pub const VP_OUTLIER_MIN_SIMILARITY: f32 = 0.18;

#[derive(Debug, Clone, PartialEq)]
pub struct CandidateProfile {
    pub cand_id: i64,
    pub centroid: Vec<f32>,
    pub n_segments: usize,
    pub n_intervals: usize,
    pub total_duration_s: f64,
    pub source_segments: Vec<Value>,
    pub confirmed_entity: Option<String>,
    pub status: String,
    pub merge_events: Vec<Value>,
}
impl CandidateProfile {
    #[must_use]
    pub fn to_json(&self) -> Value {
        json!({"cand_id":self.cand_id,"centroid":self.centroid,"n_segments":self.n_segments,"n_intervals":self.n_intervals,"total_duration_s":self.total_duration_s,"source_segments":self.source_segments,"confirmed_entity":self.confirmed_entity,"status":self.status,"merge_events":self.merge_events})
    }
    pub fn from_json(value: &Value) -> Option<Self> {
        let o = value.as_object()?;
        Some(Self {
            cand_id: o.get("cand_id")?.as_i64()?,
            centroid: o
                .get("centroid")?
                .as_array()?
                .iter()
                .map(|v| v.as_f64().map(|x| x as f32))
                .collect::<Option<Vec<_>>>()?,
            n_segments: o.get("n_segments").and_then(Value::as_u64).unwrap_or(0) as usize,
            n_intervals: o.get("n_intervals").and_then(Value::as_u64).unwrap_or(0) as usize,
            total_duration_s: o
                .get("total_duration_s")
                .and_then(Value::as_f64)
                .unwrap_or(0.0),
            source_segments: o
                .get("source_segments")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default(),
            confirmed_entity: o
                .get("confirmed_entity")
                .and_then(Value::as_str)
                .map(str::to_owned),
            status: o
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("pending")
                .to_owned(),
            merge_events: o
                .get("merge_events")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default(),
        })
    }
    #[must_use]
    pub fn ready_for_confirmation(&self) -> bool {
        self.status == "pending"
            && self.n_segments >= CONFIRM_MIN_SEGMENTS
            && self.n_intervals >= CONFIRM_MIN_INTERVALS
            && self.total_duration_s >= CONFIRM_MIN_DURATION_S
    }
}

#[derive(Debug, Error)]
pub enum CandidateTrackerError {
    #[error("candidate pool lock failed: {0}")]
    Lock(#[from] LockError),
    #[error("candidate pool read failed: {0}")]
    Read(#[from] std::io::Error),
    #[error("candidate pool write failed: {0}")]
    Write(#[from] solstone_core_journal_io::AtomicWriteError),
    #[error("entity lookup failed: {0}")]
    Entity(#[from] solstone_core_entity::EntityStoreError),
    #[error("voiceprint write failed: {0}")]
    Voiceprint(#[from] solstone_core_entity::VoiceprintOperationError),
    #[error("target entity is not an admissible Person")]
    NonPersonTarget,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ClusterInput {
    pub source_segment: Value,
    pub embeddings: Vec<Vec<f32>>,
    pub durations_s: Vec<f64>,
}
#[derive(Debug, Clone, PartialEq)]
pub struct RetroactiveConfirmPlan {
    pub matched: bool,
    pub candidate_id: Option<i64>,
    pub entity_id: String,
    pub voiceprint_items_to_add: Vec<VoiceprintItem>,
}

pub fn trim_solo_cluster_rows(rows: &[Vec<f32>]) -> (Vec<Vec<f32>>, Option<Vec<f32>>, usize) {
    if rows.is_empty() {
        return (vec![], None, 0);
    }
    let Some(first) = centroid(rows) else {
        return (vec![], None, 0);
    };
    let kept = rows
        .iter()
        .filter(|row| dot(row, &first) >= SOLO_CLUSTER_MIN_COSINE)
        .cloned()
        .collect::<Vec<_>>();
    let trimmed = rows.len() - kept.len();
    let center = centroid(&kept);
    (kept, center, trimmed)
}

pub struct CandidateTracker {
    store_path: PathBuf,
    candidates: BTreeMap<i64, CandidateProfile>,
    next_id: i64,
    pub consolidation_summary: Value,
}
impl CandidateTracker {
    pub fn new(journal_root: &Path) -> Self {
        let mut tracker = Self {
            store_path: journal_root.join("awareness/speaker_candidates.json"),
            candidates: BTreeMap::new(),
            next_id: 1,
            consolidation_summary: json!({"merge_count_total":0,"last_merge":null}),
        };
        tracker.load_tolerant();
        tracker
    }
    #[must_use]
    pub fn candidates(&self) -> Vec<CandidateProfile> {
        self.candidates.values().cloned().collect()
    }
    fn load_tolerant(&mut self) {
        let Ok(bytes) = fs::read(&self.store_path) else {
            return;
        };
        let Ok(data) = serde_json::from_slice::<Value>(&bytes) else {
            return;
        };
        let Some(o) = data.as_object() else { return };
        self.next_id = o.get("next_id").and_then(Value::as_i64).unwrap_or(1);
        self.candidates = o
            .get("candidates")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(CandidateProfile::from_json)
            .map(|c| (c.cand_id, c))
            .collect();
        self.consolidation_summary = o
            .get("consolidation_summary")
            .cloned()
            .filter(Value::is_object)
            .unwrap_or_else(|| json!({"merge_count_total":0,"last_merge":null}));
    }
    fn write(&self) -> Result<(), CandidateTrackerError> {
        let data = json!({"next_id":self.next_id,"candidates":self.candidates.values().map(CandidateProfile::to_json).collect::<Vec<_>>(),"consolidation_summary":self.consolidation_summary});
        atomic_replace(
            &self.store_path,
            format!(
                "{}\n",
                serde_json::to_string_pretty(&data).expect("JSON value serializes")
            )
            .as_bytes(),
            AtomicWriteOptions::default(),
        )?;
        Ok(())
    }
    pub fn snapshot_candidates_locked(
        &mut self,
    ) -> Result<Vec<CandidateProfile>, CandidateTrackerError> {
        let _lock = hold_lock(&self.store_path, LockOptions::default())?;
        self.load_tolerant();
        Ok(self.candidates())
    }
    /// Process the embedding-derived clusters for one source segment.
    pub fn process_segment(
        &mut self,
        inputs: &[ClusterInput],
    ) -> Result<(), CandidateTrackerError> {
        self.process_segment_with_lock_options(inputs, LockOptions::default())
    }

    fn process_segment_with_lock_options(
        &mut self,
        inputs: &[ClusterInput],
        options: LockOptions,
    ) -> Result<(), CandidateTrackerError> {
        // The guard deliberately precedes the reload: this is a locked RMW.
        let _lock = hold_lock(&self.store_path, options)?;
        self.load_tolerant();
        let mut changed = false;
        let mut known = self.source_keys();
        for input in inputs {
            let key = source_key(&input.source_segment);
            if known.contains(&key) {
                continue;
            }
            let Some(center) = centroid(&input.embeddings) else {
                continue;
            };
            let spread = input
                .embeddings
                .iter()
                .map(|e| 1.0 - dot(e, &center))
                .sum::<f32>()
                / input.embeddings.len() as f32;
            if spread >= STABILITY_THRESHOLD {
                continue;
            }
            let intervals = input.embeddings.len();
            let duration = input.durations_s.iter().sum();
            let best = self.best_match(&center);
            if let Some((id, _score)) = best.filter(|(_, score)| *score >= MERGE_THRESHOLD) {
                self.merge_into(
                    id,
                    center,
                    intervals,
                    duration,
                    input.source_segment.clone(),
                );
            } else if best.is_none() || best.is_some_and(|(_, score)| score < SPLIT_THRESHOLD) {
                self.create(center, intervals, duration, input.source_segment.clone());
            } else {
                self.create(center, intervals, duration, input.source_segment.clone());
            }
            known.insert(key);
            changed = true;
        }
        if changed {
            self.write()?
        }
        Ok(())
    }
    fn source_keys(&self) -> HashSet<String> {
        self.candidates
            .values()
            .flat_map(|c| c.source_segments.iter().map(source_key))
            .collect()
    }
    fn best_match(&self, center: &[f32]) -> Option<(i64, f32)> {
        self.candidates
            .values()
            .filter(|c| c.status != "rejected")
            .map(|c| (c.cand_id, dot(center, &c.centroid)))
            .max_by(|a, b| a.1.total_cmp(&b.1))
    }
    fn create(&mut self, center: Vec<f32>, n: usize, d: f64, source: Value) {
        let id = self.next_id;
        self.next_id += 1;
        self.candidates.insert(
            id,
            CandidateProfile {
                cand_id: id,
                centroid: center,
                n_segments: 1,
                n_intervals: n,
                total_duration_s: d,
                source_segments: vec![source],
                confirmed_entity: None,
                status: "pending".into(),
                merge_events: vec![],
            },
        );
    }
    fn merge_into(&mut self, id: i64, center: Vec<f32>, n: usize, d: f64, source: Value) {
        let c = self.candidates.get_mut(&id).expect("matched candidate");
        let mut both = Vec::new();
        for _ in 0..c.n_intervals {
            both.push(c.centroid.clone())
        }
        for _ in 0..n {
            both.push(center.clone())
        }
        if let Some(v) = centroid(&both) {
            c.centroid = v
        }
        c.n_intervals += n;
        c.total_duration_s += d;
        c.source_segments.push(source);
        c.n_segments = unique_segment_count(&c.source_segments);
    }
    pub fn best_consolidation_pair(&self) -> Option<(i64, i64, f32)> {
        let all = self.candidates();
        let mut best = None;
        for (i, a) in all.iter().enumerate() {
            if !eligible(a) {
                continue;
            }
            for b in &all[i + 1..] {
                if !eligible(b) {
                    continue;
                }
                let score = dot(&a.centroid, &b.centroid);
                if score < CONSOLIDATE_MERGE_THRESHOLD {
                    continue;
                }
                let pair = (a.cand_id.min(b.cand_id), a.cand_id.max(b.cand_id), score);
                if best.as_ref().is_none_or(|old: &(i64, i64, f32)| {
                    score > old.2 || (score == old.2 && (pair.0, pair.1) < (old.0, old.1))
                }) {
                    best = Some(pair)
                }
            }
        }
        best
    }
    pub fn plan_retroactive_confirm(
        &self,
        center: &[f32],
        entity_id: &str,
        items: Vec<VoiceprintItem>,
    ) -> RetroactiveConfirmPlan {
        let matched = self
            .best_match(center)
            .is_some_and(|(_, score)| score >= MERGE_THRESHOLD);
        let candidate_id = matched.then(|| self.best_match(center).expect("matched").0);
        RetroactiveConfirmPlan {
            matched,
            candidate_id,
            entity_id: entity_id.into(),
            voiceprint_items_to_add: items,
        }
    }
    pub(crate) fn mark_confirmed(
        &mut self,
        cand_id: i64,
        entity_id: &str,
    ) -> Result<(), CandidateTrackerError> {
        let _lock = hold_lock(&self.store_path, LockOptions::default())?;
        self.load_tolerant();
        if let Some(candidate) = self.candidates.get_mut(&cand_id) {
            candidate.status = "confirmed".into();
            candidate.confirmed_entity = Some(entity_id.into());
            self.write()?;
        }
        Ok(())
    }
    pub fn apply_retroactive_confirm_plan(
        &mut self,
        journal_root: &Path,
        plan: &RetroactiveConfirmPlan,
        encoder: &EncoderIdentity,
    ) -> Result<usize, CandidateTrackerError> {
        if !plan.matched {
            return Ok(0);
        }
        let entities = load_all_journal_entities(journal_root)?;
        let kind = entities
            .iter()
            .find(|e| e.id == plan.entity_id)
            .and_then(|e| e.entity_type());
        if !is_admissible_person(kind) {
            return Err(CandidateTrackerError::NonPersonTarget);
        }
        let saved = save_voiceprints_batch(
            journal_root,
            &plan.entity_id,
            &plan.voiceprint_items_to_add,
            encoder,
        )?;
        if let Some(id) = plan.candidate_id {
            let _lock = hold_lock(&self.store_path, LockOptions::default())?;
            self.load_tolerant();
            if let Some(candidate) = self.candidates.get_mut(&id) {
                candidate.status = "confirmed".into();
                candidate.confirmed_entity = Some(plan.entity_id.clone());
                self.write()?
            }
        }
        Ok(saved)
    }
}
fn eligible(c: &CandidateProfile) -> bool {
    c.n_intervals >= CONSOLIDATE_MIN_INTERVALS
        && c.status == "pending"
        && c.confirmed_entity.is_none()
}
pub fn eligible_for_pair_suggestion(
    left: &CandidateProfile,
    right: &CandidateProfile,
    score: f32,
) -> bool {
    if left.n_intervals < CONSOLIDATE_MIN_INTERVALS
        || right.n_intervals < CONSOLIDATE_MIN_INTERVALS
        || left.status == "rejected"
        || right.status == "rejected"
    {
        return false;
    }
    let lc = left.status == "confirmed" || left.confirmed_entity.is_some();
    let rc = right.status == "confirmed" || right.confirmed_entity.is_some();
    if lc && rc {
        return false;
    }
    (CONSOLIDATE_SUGGEST_MIN..CONSOLIDATE_MERGE_THRESHOLD).contains(&score)
        || (score >= CONSOLIDATE_MERGE_THRESHOLD && lc != rc)
}
pub fn pool_section(candidates: &[CandidateProfile]) -> Value {
    json!({"total":candidates.len(),"dense_count":candidates.iter().filter(|c|c.n_intervals>=CONSOLIDATE_MIN_INTERVALS).count()})
}
pub fn retroactive_voiceprint_metadata(
    day: &str,
    stream: &str,
    segment_key: &str,
    source: &str,
    sentence_id: i64,
    added_at: i64,
    last_seen_ts: i64,
) -> Value {
    VoiceprintMetadata::new(
        day,
        segment_key,
        source,
        stream,
        sentence_id,
        added_at,
        last_seen_ts,
    )
    .to_json()
}

fn source_key(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_default()
}
fn unique_segment_count(items: &[Value]) -> usize {
    items
        .iter()
        .filter_map(Value::as_object)
        .map(|o| {
            format!(
                "{}:{}:{}:{}",
                o.get("day").and_then(Value::as_str).unwrap_or_default(),
                o.get("segment_key")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
                o.get("stream").and_then(Value::as_str).unwrap_or_default(),
                o.get("source").and_then(Value::as_str).unwrap_or_default()
            )
        })
        .collect::<HashSet<_>>()
        .len()
}
fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(a, b)| a * b).sum()
}
fn centroid(rows: &[Vec<f32>]) -> Option<Vec<f32>> {
    let first = rows.first()?;
    let mut sum = vec![0.0; first.len()];
    for row in rows {
        if row.len() != sum.len() {
            return None;
        }
        for (x, y) in sum.iter_mut().zip(row) {
            *x += y
        }
    }
    let norm = sum.iter().map(|x| x * x).sum::<f32>().sqrt();
    (norm > 0.0).then(|| sum.into_iter().map(|x| x / norm).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::Duration;

    fn temporary_journal(name: &str) -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("solstone-candidate-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir(&path).unwrap();
        path
    }

    fn source_segment() -> ClusterInput {
        ClusterInput {
            source_segment: json!({"day":"20260101","segment_key":"seg-a","stream":"mic","source":"audio","cluster_label":1}),
            embeddings: vec![vec![1.0, 0.0], vec![1.0, 0.0]],
            durations_s: vec![2.0, 3.0],
        }
    }

    #[test]
    fn ac13_trim_solo_cluster_rows_discards_below_threshold_rows() {
        let (rows, centroid, trimmed) =
            trim_solo_cluster_rows(&[vec![1.0, 0.0], vec![1.0, 0.0], vec![-1.0, 0.0]]);
        assert_eq!(rows.len(), 2);
        assert!(centroid.is_some());
        assert_eq!(trimmed, 1);
    }

    #[test]
    fn ac14_confirmation_and_pair_gates_match_boundaries() {
        let candidate = CandidateProfile {
            cand_id: 1,
            centroid: vec![1.0, 0.0],
            n_segments: CONFIRM_MIN_SEGMENTS,
            n_intervals: CONFIRM_MIN_INTERVALS,
            total_duration_s: CONFIRM_MIN_DURATION_S,
            source_segments: vec![],
            confirmed_entity: None,
            status: "pending".into(),
            merge_events: vec![],
        };
        assert!(candidate.ready_for_confirmation());
        let mut dense = candidate.clone();
        dense.n_intervals = CONSOLIDATE_MIN_INTERVALS;
        assert!(eligible_for_pair_suggestion(
            &dense,
            &dense,
            CONSOLIDATE_SUGGEST_MIN
        ));
        assert!(!eligible_for_pair_suggestion(
            &dense,
            &dense,
            CONSOLIDATE_SUGGEST_MIN - f32::EPSILON
        ));
    }

    #[test]
    fn ac11_ac12_merge_and_stability_boundaries_are_inclusive() {
        assert!(MERGE_THRESHOLD >= 0.72);
        assert!(STABILITY_THRESHOLD >= 0.25);
    }

    #[test]
    fn ac15_process_segment_creates_once_and_deduplicates_the_source_key() {
        let journal = temporary_journal("ac15");
        let mut tracker = CandidateTracker::new(&journal);
        let input = source_segment();
        tracker
            .process_segment(std::slice::from_ref(&input))
            .unwrap();
        let first = tracker.snapshot_candidates_locked().unwrap();
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].cand_id, 1);
        assert_eq!(first[0].n_intervals, 2);

        tracker.process_segment(&[input]).unwrap();
        let second = tracker.snapshot_candidates_locked().unwrap();
        assert_eq!(second.len(), 1);
        assert_eq!(second[0].cand_id, 1);
        assert_eq!(second[0].n_intervals, 2);
        let _ = fs::remove_dir_all(journal);
    }

    #[test]
    fn ac16_process_segment_times_out_before_reload_or_write_when_pool_is_locked() {
        let journal = temporary_journal("ac16");
        let mut tracker = CandidateTracker::new(&journal);
        let path = tracker.store_path.clone();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, b"{\"next_id\":9,\"candidates\":[]}").unwrap();
        let before = fs::read(&path).unwrap();
        let _holder = hold_lock(&path, LockOptions::default()).unwrap();
        let error = tracker
            .process_segment_with_lock_options(
                &[source_segment()],
                LockOptions {
                    timeout: Duration::ZERO,
                    ..LockOptions::default()
                },
            )
            .unwrap_err();
        assert!(matches!(
            error,
            CandidateTrackerError::Lock(LockError::Timeout(_))
        ));
        assert_eq!(fs::read(&path).unwrap(), before);
        assert!(tracker.candidates.is_empty());
        let _ = fs::remove_dir_all(journal);
    }
}
