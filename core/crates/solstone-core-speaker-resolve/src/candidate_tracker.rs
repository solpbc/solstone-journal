// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Durable cross-segment speaker candidate pool.

use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use chrono::{SecondsFormat, Utc};
use serde_json::{Value, json};
use solstone_core_journal_io::{
    AtomicWriteOptions, LockError, LockOptions, SegmentLayout, atomic_replace, hold_lock,
};
use thiserror::Error;

use crate::voiceprint_metadata::VoiceprintMetadata;

// Preserved from the retired Python speaker encoder configuration.
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
    #[error("candidate pool centroid width mismatch")]
    CentroidWidth,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ClusterInput {
    pub source_segment: Value,
    pub embeddings: Vec<Vec<f32>>,
    pub durations_s: Vec<f64>,
}
/// Result of compare-restoring a candidate confirmed by identify.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CandidateRestoreReport {
    pub restored_count: usize,
    pub skipped_count: usize,
    pub missing_count: usize,
    pub already_restored_count: usize,
    pub concurrent_change_count: usize,
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

pub fn trim_solo_cluster_indices(rows: &[Vec<f32>]) -> (Vec<usize>, Option<Vec<f32>>) {
    if rows.is_empty() {
        return (vec![], None);
    }
    let Some(first) = centroid(rows) else {
        return (vec![], None);
    };
    let mut kept_indices = Vec::new();
    let mut kept_rows = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        if dot(row, &first) >= SOLO_CLUSTER_MIN_COSINE {
            kept_indices.push(index);
            kept_rows.push(row.clone());
        }
    }
    let center = centroid(&kept_rows);
    (kept_indices, center)
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
        self.load_strict()?;
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
        self.load_strict()?;
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
            } else {
                // SPLIT_THRESHOLD is an inert Python parity calibration; both branches create.
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
        best_matching_candidate(&self.candidates(), center)
            .map(|(candidate, score)| (candidate.cand_id, score))
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
    fn load_strict(&mut self) -> Result<bool, CandidateTrackerError> {
        // Every writer reloads here under the lock: it must not reuse the
        // constructor's snapshot after a failed reload or silently drop
        // malformed candidates before writing.
        let bytes = match fs::read(&self.store_path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                self.candidates.clear();
                self.next_id = 1;
                self.consolidation_summary = json!({"merge_count_total":0,"last_merge":null});
                return Ok(false);
            }
            Err(error) => return Err(error.into()),
        };
        let invalid = || {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "invalid speaker candidate pool",
            )
        };
        let data: Value = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
        let object = data.as_object().ok_or_else(invalid)?;
        let rows = object
            .get("candidates")
            .and_then(Value::as_array)
            .ok_or_else(invalid)?;
        let mut candidates = BTreeMap::new();
        for row in rows {
            let candidate = CandidateProfile::from_json(row).ok_or_else(invalid)?;
            if candidate.centroid.is_empty()
                || candidate.centroid.iter().any(|value| !value.is_finite())
            {
                return Err(invalid().into());
            }
            if candidates.insert(candidate.cand_id, candidate).is_some() {
                return Err(invalid().into());
            }
        }
        if let Some(first) = candidates.values().next()
            && candidates
                .values()
                .any(|candidate| candidate.centroid.len() != first.centroid.len())
        {
            return Err(invalid().into());
        }
        let next_id = object
            .get("next_id")
            .and_then(Value::as_i64)
            .ok_or_else(invalid)?;
        if candidates.keys().any(|id| *id >= next_id) {
            return Err(invalid().into());
        }
        let summary = object
            .get("consolidation_summary")
            .cloned()
            .unwrap_or_else(|| json!({"merge_count_total":0,"last_merge":null}));
        summary
            .get("merge_count_total")
            .and_then(Value::as_u64)
            .ok_or_else(invalid)?;
        self.candidates = candidates;
        self.next_id = next_id;
        self.consolidation_summary = summary;
        Ok(true)
    }
    /// Consolidate dense pending candidates under the existing pool lock.
    pub fn consolidate_dense_candidates(&mut self) -> Result<Value, CandidateTrackerError> {
        self.consolidate_dense_candidates_with_lock_options(LockOptions::default())
    }

    fn consolidate_dense_candidates_with_lock_options(
        &mut self,
        options: LockOptions,
    ) -> Result<Value, CandidateTrackerError> {
        let _lock = hold_lock(&self.store_path, options)?;
        if !self.load_strict()? {
            return Ok(json!({"status":"ok","merged":0,"merges":[]}));
        }
        let merge_count = self.consolidation_summary["merge_count_total"]
            .as_u64()
            .expect("strict pool reader validated count");
        let mut merges = Vec::new();
        while let Some((survivor_id, absorbed_id, score)) = self.best_consolidation_pair() {
            let event = self.merge_candidate_profile(survivor_id, absorbed_id, score);
            let survivor = self
                .candidates
                .get_mut(&survivor_id)
                .expect("merged survivor exists");
            survivor.status = "pending".to_owned();
            survivor.confirmed_entity = None;
            merges.push(event);
        }
        if let Some(last_merge) = merges.last() {
            let total = merge_count
                .checked_add(merges.len() as u64)
                .ok_or_else(|| {
                    std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "speaker merge count overflow",
                    )
                })?;
            self.consolidation_summary = json!({"merge_count_total":total,"last_merge":last_merge});
            self.write()?;
        }
        Ok(json!({"status":"ok","merged":merges.len(),"merges":merges}))
    }

    /// Add clusters from transcribed audio to the candidate pool.
    pub fn add_transcribed_clusters(
        &mut self,
        inputs: &[ClusterInput],
    ) -> Result<Option<Value>, CandidateTrackerError> {
        self.add_transcribed_clusters_with_lock_options(
            inputs,
            LockOptions::default(),
            LockOptions::default(),
        )
    }

    pub fn add_transcribed_clusters_with_lock_options(
        &mut self,
        inputs: &[ClusterInput],
        feed_lock: LockOptions,
        consolidate_lock: LockOptions,
    ) -> Result<Option<Value>, CandidateTrackerError> {
        if inputs.is_empty() {
            return Ok(None);
        }

        let density_edge_crossed;
        {
            let _guard = hold_lock(&self.store_path, feed_lock)?;
            let pool_exists = self.load_strict()?;

            let mut expected_width = if pool_exists {
                self.candidates.values().next().map(|c| c.centroid.len())
            } else {
                None
            };

            let mut known_identities: HashSet<(String, String, String, String, String, i64)> = self
                .candidates
                .values()
                .flat_map(|c| &c.source_segments)
                .filter_map(feed_source_identity)
                .collect();

            struct Admitted<'a> {
                input: &'a ClusterInput,
                center: Vec<f32>,
            }

            let mut to_admit = Vec::new();
            for input in inputs {
                let Some(identity) = feed_source_identity(&input.source_segment) else {
                    continue;
                };
                if known_identities.contains(&identity) {
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
                if let Some(w) = expected_width {
                    if center.len() != w {
                        return Err(CandidateTrackerError::CentroidWidth);
                    }
                } else {
                    expected_width = Some(center.len());
                }
                known_identities.insert(identity);
                to_admit.push(Admitted { input, center });
            }

            if to_admit.is_empty() {
                return Ok(None);
            }

            let mut density_edge = false;
            for admitted in to_admit {
                let intervals = admitted.input.embeddings.len();
                let duration = admitted.input.durations_s.iter().sum();
                let best = self.best_match(&admitted.center);
                if let Some((id, _score)) = best.filter(|(_, score)| *score >= MERGE_THRESHOLD) {
                    let before_intervals =
                        self.candidates.get(&id).map(|c| c.n_intervals).unwrap_or(0);
                    self.merge_into(
                        id,
                        admitted.center,
                        intervals,
                        duration,
                        admitted.input.source_segment.clone(),
                    );
                    let after_intervals =
                        self.candidates.get(&id).map(|c| c.n_intervals).unwrap_or(0);
                    if before_intervals < CONSOLIDATE_MIN_INTERVALS
                        && after_intervals >= CONSOLIDATE_MIN_INTERVALS
                    {
                        density_edge = true;
                    }
                } else {
                    if intervals >= CONSOLIDATE_MIN_INTERVALS {
                        density_edge = true;
                    }
                    self.create(
                        admitted.center,
                        intervals,
                        duration,
                        admitted.input.source_segment.clone(),
                    );
                }
            }

            self.write()?;
            density_edge_crossed = density_edge;
        }

        #[cfg(test)]
        if density_edge_crossed {
            POST_FEED_HOOK.with(|cell| {
                if let Some(ref mut hook) = *cell.borrow_mut() {
                    hook();
                }
            });
        }

        if density_edge_crossed {
            match self.consolidate_dense_candidates_with_lock_options(consolidate_lock) {
                Ok(report) => Ok(Some(report)),
                Err(_err) => {
                    log::warn!("speaker candidate consolidation failed after the pool update");
                    Ok(None)
                }
            }
        } else {
            Ok(None)
        }
    }
    /// Manually merge two review-approved candidates addressed by source anchors.
    pub fn merge_candidate_pair(
        &mut self,
        anchor_a: &str,
        anchor_b: &str,
    ) -> Result<Value, CandidateTrackerError> {
        let _lock = hold_lock(&self.store_path, LockOptions::default())?;
        self.load_strict()?;
        let Some(left_id) = self.candidate_id_for_anchor(anchor_a) else {
            return Ok(json!({"status":"error","error":"candidate anchor not found"}));
        };
        let Some(right_id) = self.candidate_id_for_anchor(anchor_b) else {
            return Ok(json!({"status":"error","error":"candidate anchor not found"}));
        };
        if left_id == right_id {
            return Ok(json!({"status":"already_merged","survivor_id":left_id}));
        }
        let left = self
            .candidates
            .get(&left_id)
            .expect("anchor candidate exists");
        let right = self
            .candidates
            .get(&right_id)
            .expect("anchor candidate exists");
        if left.status == "rejected" || right.status == "rejected" {
            return Ok(json!({"status":"error","error":"cannot merge rejected candidate"}));
        }
        let confirmed = [left, right]
            .into_iter()
            .filter(|candidate| {
                candidate.status == "confirmed" || candidate.confirmed_entity.is_some()
            })
            .collect::<Vec<_>>();
        if confirmed.len() > 1 {
            return Ok(json!({"status":"error","error":"cannot merge two confirmed candidates"}));
        }
        let score = dot(&left.centroid, &right.centroid);
        if score < CONSOLIDATE_SUGGEST_MIN {
            return Ok(json!({
                "status":"error",
                "error":"candidate pair is below review threshold",
                "score":score,
            }));
        }
        let (survivor_id, absorbed_id) = (left_id.min(right_id), left_id.max(right_id));
        let confirmed_entity = confirmed
            .first()
            .and_then(|candidate| candidate.confirmed_entity.clone());
        let merge_event = self.merge_candidate_profile(survivor_id, absorbed_id, score);
        let survivor = self
            .candidates
            .get_mut(&survivor_id)
            .expect("survivor remains after merge");
        if let Some(confirmed_entity) = &confirmed_entity {
            survivor.status = "confirmed".to_owned();
            survivor.confirmed_entity = Some(confirmed_entity.clone());
        } else {
            survivor.status = "pending".to_owned();
            survivor.confirmed_entity = None;
        }
        self.write()?;
        Ok(json!({
            "status":"merged",
            "survivor_id":survivor_id,
            "absorbed_id":absorbed_id,
            "score":score,
            "merge_event":merge_event,
            "confirmed_entity":confirmed_entity,
        }))
    }
    fn candidate_id_for_anchor(&self, anchor: &str) -> Option<i64> {
        self.candidates.values().find_map(|candidate| {
            candidate
                .source_segments
                .iter()
                .filter_map(source_segment_anchor)
                .any(|candidate_anchor| candidate_anchor == anchor)
                .then_some(candidate.cand_id)
        })
    }
    fn merge_candidate_profile(&mut self, survivor_id: i64, absorbed_id: i64, score: f32) -> Value {
        let absorbed = self
            .candidates
            .remove(&absorbed_id)
            .expect("absorbed candidate exists");
        let survivor = self
            .candidates
            .get_mut(&survivor_id)
            .expect("survivor candidate exists");
        let mut centers = Vec::with_capacity(survivor.n_intervals + absorbed.n_intervals);
        centers.extend(std::iter::repeat_n(
            survivor.centroid.clone(),
            survivor.n_intervals,
        ));
        centers.extend(std::iter::repeat_n(
            absorbed.centroid.clone(),
            absorbed.n_intervals,
        ));
        if let Some(center) = centroid(&centers) {
            survivor.centroid = center;
        }
        let mut existing_source_keys = survivor
            .source_segments
            .iter()
            .map(source_key)
            .collect::<HashSet<_>>();
        for source_segment in absorbed.source_segments {
            if existing_source_keys.insert(source_key(&source_segment)) {
                survivor.source_segments.push(source_segment);
            }
        }
        survivor.n_intervals += absorbed.n_intervals;
        survivor.total_duration_s += absorbed.total_duration_s;
        survivor.n_segments = unique_segment_count(&survivor.source_segments);
        survivor.merge_events.extend(absorbed.merge_events);
        let event = json!({
            "survivor_id": survivor.cand_id,
            "absorbed_id": absorbed_id,
            "score": score,
            "merged_at": Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true),
            "absorbed_n_intervals": absorbed.n_intervals,
            "survivor_n_intervals_after": survivor.n_intervals,
        });
        survivor.merge_events.push(event.clone());
        event
    }
    pub(crate) fn mark_confirmed(
        &mut self,
        cand_id: i64,
        entity_id: &str,
    ) -> Result<(), CandidateTrackerError> {
        let _lock = hold_lock(&self.store_path, LockOptions::default())?;
        self.load_strict()?;
        if let Some(candidate) = self.candidates.get_mut(&cand_id) {
            candidate.status = "confirmed".into();
            candidate.confirmed_entity = Some(entity_id.into());
            self.write()?;
        }
        Ok(())
    }
    /// Restore a candidate only if its current snapshot still equals identify's after snapshot.
    pub(crate) fn restore_confirmed_candidate(
        &mut self,
        cand_id: i64,
        expected_after: &Value,
        candidate_before: &Value,
    ) -> Result<CandidateRestoreReport, CandidateTrackerError> {
        let _lock = hold_lock(&self.store_path, LockOptions::default())?;
        self.load_strict()?;
        let Some(candidate) = self.candidates.get(&cand_id) else {
            return Ok(CandidateRestoreReport {
                skipped_count: 1,
                missing_count: 1,
                ..CandidateRestoreReport::default()
            });
        };
        let current = candidate.to_json();
        if current == *candidate_before {
            return Ok(CandidateRestoreReport {
                skipped_count: 1,
                already_restored_count: 1,
                ..CandidateRestoreReport::default()
            });
        }
        if current != *expected_after {
            return Ok(CandidateRestoreReport {
                skipped_count: 1,
                concurrent_change_count: 1,
                ..CandidateRestoreReport::default()
            });
        }
        let Some(restored) = CandidateProfile::from_json(candidate_before) else {
            return Ok(CandidateRestoreReport {
                skipped_count: 1,
                concurrent_change_count: 1,
                ..CandidateRestoreReport::default()
            });
        };
        self.candidates.insert(cand_id, restored);
        self.write()?;
        Ok(CandidateRestoreReport {
            restored_count: 1,
            ..CandidateRestoreReport::default()
        })
    }
}

/// Return the highest-scoring candidate eligible for a new match.
pub(crate) fn best_matching_candidate<'a>(
    candidates: &'a [CandidateProfile],
    center: &[f32],
) -> Option<(&'a CandidateProfile, f32)> {
    candidates
        .iter()
        .filter(|candidate| candidate.status != "rejected")
        .map(|candidate| (candidate, dot(center, &candidate.centroid)))
        .max_by(|left, right| left.1.total_cmp(&right.1))
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
#[allow(clippy::too_many_arguments)]
pub fn retroactive_voiceprint_metadata(
    day: &str,
    stream_layout: SegmentLayout,
    stream: &str,
    segment_key: &str,
    source: &str,
    sentence_id: i64,
    added_at: i64,
    last_seen_ts: i64,
) -> Value {
    VoiceprintMetadata::new(
        day,
        stream_layout,
        segment_key,
        source,
        stream,
        sentence_id,
        added_at,
        last_seen_ts,
    )
    .to_json()
}

fn feed_source_identity(source: &Value) -> Option<(String, String, String, String, String, i64)> {
    let day = source.get("day")?.as_str()?.to_owned();
    let stream = source.get("stream")?.as_str()?.to_owned();
    let stream_layout = source
        .get("stream_layout")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or_else(|| {
            if stream == "_default" {
                "direct".into()
            } else {
                "named".into()
            }
        });
    let segment_key = source.get("segment_key")?.as_str()?.to_owned();
    let source_str = source.get("source")?.as_str()?.to_owned();
    let cluster_label = source.get("cluster_label")?.as_i64()?;
    Some((
        day,
        stream_layout,
        stream,
        segment_key,
        source_str,
        cluster_label,
    ))
}

#[cfg(test)]
thread_local! {
    static POST_FEED_HOOK: std::cell::RefCell<Option<Box<dyn FnMut()>>> = const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
pub struct PostFeedHookGuard;

#[cfg(test)]
impl Drop for PostFeedHookGuard {
    fn drop(&mut self) {
        POST_FEED_HOOK.with(|cell| {
            *cell.borrow_mut() = None;
        });
    }
}

#[cfg(test)]
pub fn set_post_feed_hook(hook: impl FnMut() + 'static) -> PostFeedHookGuard {
    POST_FEED_HOOK.with(|cell| {
        *cell.borrow_mut() = Some(Box::new(hook));
    });
    PostFeedHookGuard
}

fn source_key(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_default()
}
pub(crate) fn source_segment_anchor(value: &Value) -> Option<String> {
    let object = value.as_object()?;
    let day = object.get("day")?.as_str()?;
    let segment_key = object.get("segment_key")?.as_str()?;
    let stream = object.get("stream")?.as_str()?;
    let source = object.get("source")?.as_str()?;
    let cluster_label = object.get("cluster_label")?.as_i64()?;
    if let Some(layout_val) = object.get("stream_layout") {
        let layout_str = layout_val.as_str()?;
        if layout_str != "direct" && layout_str != "named" {
            return None;
        }
        Some(
            json!([
                2,
                day,
                layout_str,
                stream,
                segment_key,
                source,
                cluster_label
            ])
            .to_string(),
        )
    } else {
        Some(json!([day, segment_key, stream, source, cluster_label]).to_string())
    }
}
fn unique_segment_count(items: &[Value]) -> usize {
    items
        .iter()
        .filter_map(Value::as_object)
        .map(|o| {
            format!(
                "{}:{}:{}:{}:{}",
                o.get("day").and_then(Value::as_str).unwrap_or_default(),
                o.get("stream_layout")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
                o.get("stream").and_then(Value::as_str).unwrap_or_default(),
                o.get("segment_key")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
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
        cluster_input("seg-a", vec![vec![1.0, 0.0], vec![1.0, 0.0]])
    }

    fn cluster_input(segment_key: &str, embeddings: Vec<Vec<f32>>) -> ClusterInput {
        ClusterInput {
            source_segment: json!({"day":"20260101","segment_key":segment_key,"stream":"mic","source":"audio","cluster_label":1}),
            durations_s: vec![1.0; embeddings.len()],
            embeddings,
        }
    }

    fn source(anchor: &str) -> Value {
        json!({
            "day": "20260101",
            "segment_key": anchor,
            "stream": "mic",
            "source": "audio",
            "cluster_label": 1,
        })
    }

    fn candidate(id: i64, anchor: &str, centroid: Vec<f32>) -> CandidateProfile {
        CandidateProfile {
            cand_id: id,
            centroid,
            n_segments: 1,
            n_intervals: 1,
            total_duration_s: 1.0,
            source_segments: vec![source(anchor)],
            confirmed_entity: None,
            status: "pending".to_owned(),
            merge_events: Vec::new(),
        }
    }

    fn anchor(anchor: &str) -> String {
        source_segment_anchor(&source(anchor)).unwrap()
    }

    fn tracker_with_candidates(
        name: &str,
        candidates: Vec<CandidateProfile>,
    ) -> (PathBuf, CandidateTracker) {
        let journal = temporary_journal(name);
        let mut tracker = CandidateTracker::new(&journal);
        fs::create_dir_all(tracker.store_path.parent().unwrap()).unwrap();
        tracker.next_id = candidates
            .iter()
            .map(|candidate| candidate.cand_id)
            .max()
            .unwrap_or(0)
            + 1;
        tracker.candidates = candidates
            .into_iter()
            .map(|candidate| (candidate.cand_id, candidate))
            .collect();
        tracker.write().unwrap();
        (journal, tracker)
    }

    #[test]
    fn consolidation_merges_to_fixpoint_preserves_reviewed_candidates_and_is_idempotent() {
        let mut rows = (1..=5)
            .map(|id| {
                let mut row = candidate(id, &format!("seg-{id}"), vec![1.0, 0.0]);
                row.n_intervals = CONSOLIDATE_MIN_INTERVALS;
                row
            })
            .collect::<Vec<_>>();
        rows[3].status = "confirmed".to_owned();
        rows[3].confirmed_entity = Some("alice".to_owned());
        rows[4].status = "rejected".to_owned();
        let confirmed = rows[3].clone();
        let rejected = rows[4].clone();
        let (root, mut tracker) = tracker_with_candidates("consolidation-fixpoint", rows);
        tracker.consolidation_summary = json!({"merge_count_total":5,"last_merge":null});
        tracker.write().unwrap();
        let report = tracker.consolidate_dense_candidates().unwrap();
        assert_eq!(report["merged"], 2);
        let reloaded = CandidateTracker::new(&root);
        assert_eq!(reloaded.candidates.len(), 3);
        assert_eq!(reloaded.candidates[&4], confirmed);
        assert_eq!(reloaded.candidates[&5], rejected);
        assert_eq!(
            reloaded.candidates[&1].n_intervals,
            3 * CONSOLIDATE_MIN_INTERVALS
        );
        assert_eq!(reloaded.candidates[&1].source_segments.len(), 3);
        assert_eq!(reloaded.candidates[&1].status, "pending");
        assert_eq!(reloaded.candidates[&1].confirmed_entity, None);
        assert_eq!(reloaded.next_id, 6);
        assert_eq!(reloaded.consolidation_summary["merge_count_total"], 7);
        let before = fs::read(&tracker.store_path).unwrap();
        let mtime = fs::metadata(&tracker.store_path)
            .unwrap()
            .modified()
            .unwrap();
        assert_eq!(tracker.consolidate_dense_candidates().unwrap()["merged"], 0);
        assert_eq!(fs::read(&tracker.store_path).unwrap(), before);
        assert_eq!(
            fs::metadata(&tracker.store_path)
                .unwrap()
                .modified()
                .unwrap(),
            mtime
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn consolidation_reloads_current_pool_and_never_resurrects_a_removed_store() {
        let mut left = candidate(1, "a", vec![1.0, 0.0]);
        left.n_intervals = CONSOLIDATE_MIN_INTERVALS;
        let mut right = candidate(2, "b", vec![1.0, 0.0]);
        right.n_intervals = CONSOLIDATE_MIN_INTERVALS;
        let (root, mut stale) = tracker_with_candidates("consolidation-reload", vec![left, right]);
        let mut current = CandidateTracker::new(&root);
        current.candidates.get_mut(&2).unwrap().status = "rejected".to_owned();
        current.write().unwrap();
        let before = fs::read(&stale.store_path).unwrap();
        assert_eq!(stale.consolidate_dense_candidates().unwrap()["merged"], 0);
        assert_eq!(fs::read(&stale.store_path).unwrap(), before);
        fs::remove_file(&stale.store_path).unwrap();
        assert_eq!(stale.consolidate_dense_candidates().unwrap()["merged"], 0);
        assert!(!stale.store_path.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn consolidation_refuses_malformed_pool_without_overwriting_it() {
        let mut left = candidate(1, "a", vec![1.0, 0.0]);
        left.n_intervals = CONSOLIDATE_MIN_INTERVALS;
        let mut right = candidate(2, "b", vec![1.0, 0.0]);
        right.n_intervals = CONSOLIDATE_MIN_INTERVALS;
        let (root, mut tracker) =
            tracker_with_candidates("consolidation-malformed", vec![left.clone(), right.clone()]);
        let mut wrong_width = right.clone();
        wrong_width.centroid.push(0.0);
        for raw in [
            "invalid JSON".to_owned(),
            json!({"next_id":3,"candidates":[left.to_json(),right.to_json(),{}]}).to_string(),
            json!({"next_id":3,"candidates":[left.to_json(),left.to_json()]}).to_string(),
            json!({"next_id":3,"candidates":[left.to_json(),wrong_width.to_json()]}).to_string(),
        ] {
            fs::write(&tracker.store_path, &raw).unwrap();
            assert!(tracker.consolidate_dense_candidates().is_err());
            assert_eq!(fs::read_to_string(&tracker.store_path).unwrap(), raw);
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn every_pool_writer_refuses_a_pool_it_cannot_read_without_overwriting_it() {
        let left = candidate(1, "a", vec![1.0, 0.0]);
        let right = candidate(2, "b", vec![0.8, 0.6]);
        let (root, _) =
            tracker_with_candidates("writers-malformed", vec![left.clone(), right.clone()]);
        // A row with no cand_id is dropped by the tolerant reader, so a writer
        // that reloaded tolerantly would rewrite the pool without it.
        for raw in [
            "invalid JSON".to_owned(),
            json!({"next_id":4,"candidates":[left.to_json(),right.to_json(),{"centroid":[0.0,1.0]}],"consolidation_summary":{"merge_count_total":0}}).to_string(),
        ] {
            let mut tracker = CandidateTracker::new(&root);
            fs::write(&tracker.store_path, &raw).unwrap();
            assert!(tracker.mark_confirmed(1, "alice").is_err());
            assert!(
                tracker
                    .merge_candidate_pair(&anchor("a"), &anchor("b"))
                    .is_err()
            );
            assert!(
                tracker
                    .restore_confirmed_candidate(1, &left.to_json(), &left.to_json())
                    .is_err()
            );
            assert!(
                tracker
                    .process_segment(&[cluster_input("new", vec![vec![1.0, 0.0]])])
                    .is_err()
            );
            assert_eq!(fs::read_to_string(&tracker.store_path).unwrap(), raw);
        }
        fs::remove_dir_all(root).unwrap();
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
    fn ac11_process_segment_merges_at_threshold_and_creates_below_it() {
        let at_threshold = vec![MERGE_THRESHOLD, (1.0 - MERGE_THRESHOLD.powi(2)).sqrt()];
        let below_threshold = vec![
            MERGE_THRESHOLD - f32::EPSILON,
            (1.0 - (MERGE_THRESHOLD - f32::EPSILON).powi(2)).sqrt(),
        ];

        let journal = temporary_journal("ac11-at-threshold");
        let mut tracker = CandidateTracker::new(&journal);
        tracker
            .process_segment(&[cluster_input("seed", vec![vec![1.0, 0.0]])])
            .unwrap();
        tracker
            .process_segment(&[cluster_input("at-threshold", vec![at_threshold])])
            .unwrap();
        let merged = tracker.snapshot_candidates_locked().unwrap();
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].n_intervals, 2);
        let _ = fs::remove_dir_all(&journal);

        let journal = temporary_journal("ac11-below-threshold");
        let mut tracker = CandidateTracker::new(&journal);
        tracker
            .process_segment(&[cluster_input("seed", vec![vec![1.0, 0.0]])])
            .unwrap();
        tracker
            .process_segment(&[cluster_input("below-threshold", vec![below_threshold])])
            .unwrap();
        let created = tracker.snapshot_candidates_locked().unwrap();
        assert_eq!(created.len(), 2);
        assert_eq!(created[0].n_intervals, 1);
        let _ = fs::remove_dir_all(journal);
    }

    #[test]
    fn ac12_process_segment_drops_at_stability_threshold_and_keeps_below_it() {
        let cluster_rows = |spread: f32| {
            let cosine = 1.0 - spread;
            let offset = (1.0 - cosine.powi(2)).sqrt();
            vec![vec![cosine, offset], vec![cosine, -offset]]
        };

        let journal = temporary_journal("ac12-at-threshold");
        let mut tracker = CandidateTracker::new(&journal);
        tracker
            .process_segment(&[cluster_input(
                "at-threshold",
                cluster_rows(STABILITY_THRESHOLD),
            )])
            .unwrap();
        assert!(tracker.snapshot_candidates_locked().unwrap().is_empty());
        let _ = fs::remove_dir_all(&journal);

        let journal = temporary_journal("ac12-below-threshold");
        let mut tracker = CandidateTracker::new(&journal);
        tracker
            .process_segment(&[cluster_input(
                "below-threshold",
                cluster_rows(STABILITY_THRESHOLD - f32::EPSILON),
            )])
            .unwrap();
        assert_eq!(tracker.snapshot_candidates_locked().unwrap().len(), 1);
        let _ = fs::remove_dir_all(journal);
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

    #[test]
    fn merge_candidate_pair_returns_not_found_for_unknown_anchor() {
        let (journal, mut tracker) = tracker_with_candidates("manual-not-found", vec![]);

        assert_eq!(
            tracker
                .merge_candidate_pair("missing-a", "missing-b")
                .unwrap(),
            json!({"status":"error","error":"candidate anchor not found"})
        );
        fs::remove_dir_all(journal).unwrap();
    }

    #[test]
    fn merge_candidate_pair_short_circuits_when_anchors_share_a_candidate() {
        let mut shared = candidate(2, "a", vec![1.0, 0.0]);
        shared.source_segments.push(source("b"));
        let (journal, mut tracker) = tracker_with_candidates("manual-same", vec![shared]);

        assert_eq!(
            tracker
                .merge_candidate_pair(&anchor("a"), &anchor("b"))
                .unwrap(),
            json!({"status":"already_merged","survivor_id":2})
        );
        fs::remove_dir_all(journal).unwrap();
    }

    #[test]
    fn merge_candidate_pair_refuses_rejected_candidates() {
        let mut rejected = candidate(1, "a", vec![1.0, 0.0]);
        rejected.status = "rejected".to_owned();
        let (journal, mut tracker) = tracker_with_candidates(
            "manual-rejected",
            vec![rejected, candidate(2, "b", vec![1.0, 0.0])],
        );

        assert_eq!(
            tracker
                .merge_candidate_pair(&anchor("a"), &anchor("b"))
                .unwrap(),
            json!({"status":"error","error":"cannot merge rejected candidate"})
        );
        fs::remove_dir_all(journal).unwrap();
    }

    #[test]
    fn merge_candidate_pair_refuses_two_confirmed_candidates() {
        let mut left = candidate(1, "a", vec![1.0, 0.0]);
        left.status = "confirmed".to_owned();
        left.confirmed_entity = Some("entity-a".to_owned());
        let mut right = candidate(2, "b", vec![1.0, 0.0]);
        right.status = "confirmed".to_owned();
        right.confirmed_entity = Some("entity-b".to_owned());
        let (journal, mut tracker) = tracker_with_candidates("manual-confirmed", vec![left, right]);

        assert_eq!(
            tracker
                .merge_candidate_pair(&anchor("a"), &anchor("b"))
                .unwrap(),
            json!({"status":"error","error":"cannot merge two confirmed candidates"})
        );
        fs::remove_dir_all(journal).unwrap();
    }

    #[test]
    fn merge_candidate_pair_refuses_scores_below_review_threshold() {
        let (journal, mut tracker) = tracker_with_candidates(
            "manual-low-score",
            vec![
                candidate(1, "a", vec![1.0, 0.0]),
                candidate(2, "b", vec![0.0, 1.0]),
            ],
        );

        assert_eq!(
            tracker
                .merge_candidate_pair(&anchor("a"), &anchor("b"))
                .unwrap(),
            json!({
                "status":"error",
                "error":"candidate pair is below review threshold",
                "score":0.0,
            })
        );
        fs::remove_dir_all(journal).unwrap();
    }

    #[test]
    fn merge_candidate_pair_keeps_lowest_id_and_persists_confirmed_entity() {
        let mut confirmed = candidate(2, "b", vec![1.0, 0.0]);
        confirmed.status = "confirmed".to_owned();
        confirmed.confirmed_entity = Some("entity-b".to_owned());
        let (journal, mut tracker) = tracker_with_candidates(
            "manual-success",
            vec![candidate(1, "a", vec![1.0, 0.0]), confirmed],
        );

        let response = tracker
            .merge_candidate_pair(&anchor("b"), &anchor("a"))
            .unwrap();

        assert_eq!(response["status"], "merged");
        assert_eq!(response["survivor_id"], 1);
        assert_eq!(response["absorbed_id"], 2);
        assert_eq!(response["confirmed_entity"], "entity-b");
        let persisted = CandidateTracker::new(&journal).candidates();
        assert_eq!(persisted.len(), 1);
        assert_eq!(persisted[0].cand_id, 1);
        assert_eq!(persisted[0].status, "confirmed");
        assert_eq!(persisted[0].confirmed_entity.as_deref(), Some("entity-b"));
        assert_eq!(persisted[0].n_intervals, 2);
        assert_eq!(persisted[0].merge_events.len(), 1);
        fs::remove_dir_all(journal).unwrap();
    }

    #[test]
    fn v1_source_objects_preserved_and_v2_anchors_differentiate_coordinates() {
        let v1_src = json!({
            "day": "20260101",
            "segment_key": "seg-v1",
            "stream": "mic",
            "source": "audio",
            "cluster_label": 1
        });
        let v1_cand = CandidateProfile {
            cand_id: 1,
            centroid: vec![1.0, 0.0],
            n_segments: 1,
            n_intervals: 1,
            total_duration_s: 1.0,
            source_segments: vec![v1_src.clone()],
            confirmed_entity: None,
            status: "pending".to_owned(),
            merge_events: Vec::new(),
        };
        let (journal_path, mut tracker) = tracker_with_candidates("v1-stability", vec![v1_cand]);

        // Add a new cluster
        let new_input = ClusterInput {
            source_segment: json!({
                "day": "20260102",
                "stream_layout": "named",
                "stream": "mic",
                "segment_key": "seg-v2",
                "source": "audio",
                "cluster_label": 2
            }),
            durations_s: vec![1.0],
            embeddings: vec![vec![0.0, 1.0]],
        };
        tracker.process_segment(&[new_input]).unwrap();

        let loaded = CandidateTracker::new(&journal_path).candidates();
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].source_segments[0], v1_src);
        assert_eq!(
            source_segment_anchor(&loaded[0].source_segments[0]).unwrap(),
            json!(["20260101", "seg-v1", "mic", "audio", 1]).to_string()
        );

        // Test v2 anchors differ when any coordinate differs
        let a_direct = json!({"day":"20260101","stream_layout":"direct","stream":"_default","segment_key":"080000_300","source":"audio","cluster_label":1});
        let a_named = json!({"day":"20260101","stream_layout":"named","stream":"_default","segment_key":"080000_300","source":"audio","cluster_label":1});
        assert_eq!(
            source_segment_anchor(&a_direct).unwrap(),
            json!([
                2,
                "20260101",
                "direct",
                "_default",
                "080000_300",
                "audio",
                1
            ])
            .to_string()
        );
        assert_eq!(
            source_segment_anchor(&a_named).unwrap(),
            json!([2, "20260101", "named", "_default", "080000_300", "audio", 1]).to_string()
        );
        assert_ne!(
            source_segment_anchor(&a_direct).unwrap(),
            source_segment_anchor(&a_named).unwrap()
        );

        fs::remove_dir_all(journal_path).unwrap();
    }

    #[test]
    fn identity_dedupe_vs_source_key_bytes_and_mtime_unchanged() {
        let journal =
            PathBuf::from("/var/tmp").join(format!("solstone-cand-dedupe-{}", std::process::id()));
        let _ = fs::remove_dir_all(&journal);
        fs::create_dir_all(journal.join("awareness")).unwrap();

        let mut tracker = CandidateTracker::new(&journal);
        let input1 = ClusterInput {
            source_segment: json!({
                "day": "20260101",
                "stream_layout": "direct",
                "stream": "_default",
                "segment_key": "093000_300_summary",
                "source": "audio",
                "cluster_label": 1,
                "sentence_ids": [10, 20]
            }),
            durations_s: vec![1.0],
            embeddings: vec![vec![1.0, 0.0]],
        };
        let res = tracker
            .add_transcribed_clusters(std::slice::from_ref(&input1))
            .unwrap();
        assert!(res.is_none());

        let store_path = journal.join("awareness/speaker_candidates.json");
        let bytes_before = fs::read(&store_path).unwrap();
        let mtime_before = fs::metadata(&store_path).unwrap().modified().unwrap();

        // Feed same identity but different sentence_ids
        let input2 = ClusterInput {
            source_segment: json!({
                "day": "20260101",
                "stream_layout": "direct",
                "stream": "_default",
                "segment_key": "093000_300_summary",
                "source": "audio",
                "cluster_label": 1,
                "sentence_ids": [30, 40]
            }),
            durations_s: vec![1.0],
            embeddings: vec![vec![1.0, 0.0]],
        };
        assert!(source_key(&input1.source_segment) != source_key(&input2.source_segment));
        let res2 = tracker.add_transcribed_clusters(&[input2]).unwrap();
        assert!(res2.is_none());

        let bytes_after = fs::read(&store_path).unwrap();
        let mtime_after = fs::metadata(&store_path).unwrap().modified().unwrap();
        assert_eq!(bytes_before, bytes_after);
        assert_eq!(mtime_before, mtime_after);

        let _ = fs::remove_dir_all(journal);
    }

    #[test]
    fn legacy_default_direct_and_mic_named_anchors() {
        let default_src = json!({
            "day": "20260101",
            "segment_key": "seg1",
            "stream": "_default",
            "source": "audio",
            "cluster_label": 1
        });
        let id_default = feed_source_identity(&default_src).unwrap();
        assert_eq!(id_default.1, "direct");

        let mic_src = json!({
            "day": "20260101",
            "segment_key": "seg2",
            "stream": "mic",
            "source": "audio",
            "cluster_label": 1
        });
        let id_mic = feed_source_identity(&mic_src).unwrap();
        assert_eq!(id_mic.1, "named");

        let mic_anchor = source_segment_anchor(&mic_src).unwrap();
        assert_eq!(
            mic_anchor,
            json!(["20260101", "seg2", "mic", "audio", 1]).to_string()
        );

        // Seed a pool with legacy mic row (no stream_layout)
        let legacy_mic_cand = CandidateProfile {
            cand_id: 1,
            centroid: vec![1.0, 0.0],
            n_segments: 1,
            n_intervals: 1,
            total_duration_s: 1.0,
            source_segments: vec![mic_src.clone()],
            confirmed_entity: None,
            status: "pending".to_owned(),
            merge_events: Vec::new(),
        };
        let (journal_mic, mut tracker_mic) =
            tracker_with_candidates("legacy-mic", vec![legacy_mic_cand]);
        let store_mic = tracker_mic.store_path.clone();
        let bytes_mic_before = fs::read(&store_mic).unwrap();
        let mtime_mic_before = fs::metadata(&store_mic).unwrap().modified().unwrap();

        let feed_mic_input = ClusterInput {
            source_segment: json!({
                "day": "20260101",
                "stream_layout": "named",
                "stream": "mic",
                "segment_key": "seg2",
                "source": "audio",
                "cluster_label": 1,
                "sentence_ids": [1, 2]
            }),
            durations_s: vec![1.0],
            embeddings: vec![vec![1.0, 0.0]],
        };
        let res = tracker_mic
            .add_transcribed_clusters(&[feed_mic_input])
            .unwrap();
        assert!(res.is_none());
        assert_eq!(fs::read(&store_mic).unwrap(), bytes_mic_before);
        assert_eq!(
            fs::metadata(&store_mic).unwrap().modified().unwrap(),
            mtime_mic_before
        );
        let loaded_mic = CandidateTracker::new(&journal_mic).candidates();
        assert_eq!(
            source_segment_anchor(&loaded_mic[0].source_segments[0]).unwrap(),
            mic_anchor
        );
        let _ = fs::remove_dir_all(journal_mic);

        // Seed a pool with legacy default direct row (no stream_layout)
        let legacy_default_cand = CandidateProfile {
            cand_id: 1,
            centroid: vec![1.0, 0.0],
            n_segments: 1,
            n_intervals: 1,
            total_duration_s: 1.0,
            source_segments: vec![default_src.clone()],
            confirmed_entity: None,
            status: "pending".to_owned(),
            merge_events: Vec::new(),
        };
        let (journal_default, mut tracker_default) =
            tracker_with_candidates("legacy-default", vec![legacy_default_cand]);
        let store_default = tracker_default.store_path.clone();
        let bytes_default_before = fs::read(&store_default).unwrap();
        let mtime_default_before = fs::metadata(&store_default).unwrap().modified().unwrap();

        let default_anchor = source_segment_anchor(&default_src).unwrap();
        let feed_default_input = ClusterInput {
            source_segment: json!({
                "day": "20260101",
                "stream_layout": "direct",
                "stream": "_default",
                "segment_key": "seg1",
                "source": "audio",
                "cluster_label": 1,
                "sentence_ids": [1, 2]
            }),
            durations_s: vec![1.0],
            embeddings: vec![vec![1.0, 0.0]],
        };
        let res2 = tracker_default
            .add_transcribed_clusters(&[feed_default_input])
            .unwrap();
        assert!(res2.is_none());
        assert_eq!(fs::read(&store_default).unwrap(), bytes_default_before);
        assert_eq!(
            fs::metadata(&store_default).unwrap().modified().unwrap(),
            mtime_default_before
        );
        let loaded_default = CandidateTracker::new(&journal_default).candidates();
        assert_eq!(
            source_segment_anchor(&loaded_default[0].source_segments[0]).unwrap(),
            default_anchor
        );
        let _ = fs::remove_dir_all(journal_default);
    }

    #[test]
    fn second_cluster_label_and_different_audio_stem() {
        let journal = PathBuf::from("/var/tmp")
            .join(format!("solstone-cand-multicluster-{}", std::process::id()));
        let _ = fs::remove_dir_all(&journal);
        fs::create_dir_all(journal.join("awareness")).unwrap();

        let mut tracker = CandidateTracker::new(&journal);
        let input_c1 = ClusterInput {
            source_segment: json!({
                "day": "20260101",
                "stream_layout": "direct",
                "stream": "_default",
                "segment_key": "seg1",
                "source": "audio",
                "cluster_label": 1
            }),
            durations_s: vec![1.0],
            embeddings: vec![vec![1.0, 0.0]],
        };
        tracker.add_transcribed_clusters(&[input_c1]).unwrap();
        assert_eq!(tracker.candidates().len(), 1);

        // Different cluster_label on same segment
        let input_c2 = ClusterInput {
            source_segment: json!({
                "day": "20260101",
                "stream_layout": "direct",
                "stream": "_default",
                "segment_key": "seg1",
                "source": "audio",
                "cluster_label": 2
            }),
            durations_s: vec![1.0],
            embeddings: vec![vec![0.0, 1.0]],
        };
        tracker.add_transcribed_clusters(&[input_c2]).unwrap();
        assert_eq!(tracker.candidates().len(), 2);

        // Different source audio stem
        let input_c3 = ClusterInput {
            source_segment: json!({
                "day": "20260101",
                "stream_layout": "direct",
                "stream": "_default",
                "segment_key": "seg1",
                "source": "aux",
                "cluster_label": 1
            }),
            durations_s: vec![1.0],
            embeddings: vec![vec![-1.0, 0.0]],
        };
        tracker.add_transcribed_clusters(&[input_c3]).unwrap();
        assert_eq!(tracker.candidates().len(), 3);

        let _ = fs::remove_dir_all(journal);
    }

    #[test]
    fn full_strict_refusal_set() {
        let journal =
            PathBuf::from("/var/tmp").join(format!("solstone-cand-strict-{}", std::process::id()));
        let store_path = journal.join("awareness/speaker_candidates.json");

        let bad_payloads = vec![
            "not valid json".to_owned(),
            "[1, 2, 3]".to_owned(),
            json!({"next_id": 2, "candidates": [{"cand_id": 1}], "consolidation_summary": {"merge_count_total": 0}}).to_string(),
            json!({"next_id": 2, "candidates": [{"cand_id": 1, "centroid": [], "n_segments": 1, "n_intervals": 1, "total_duration_s": 1.0, "source_segments": [], "status": "pending", "merge_events": []}], "consolidation_summary": {"merge_count_total": 0}}).to_string(),
            json!({"next_id": 3, "candidates": [
                {"cand_id": 1, "centroid": [1.0, 0.0], "n_segments": 1, "n_intervals": 1, "total_duration_s": 1.0, "source_segments": [], "status": "pending", "merge_events": []},
                {"cand_id": 1, "centroid": [0.0, 1.0], "n_segments": 1, "n_intervals": 1, "total_duration_s": 1.0, "source_segments": [], "status": "pending", "merge_events": []}
            ], "consolidation_summary": {"merge_count_total": 0}}).to_string(),
            json!({"next_id": 3, "candidates": [
                {"cand_id": 1, "centroid": [1.0, 0.0], "n_segments": 1, "n_intervals": 1, "total_duration_s": 1.0, "source_segments": [], "status": "pending", "merge_events": []},
                {"cand_id": 2, "centroid": [0.0, 1.0, 0.0], "n_segments": 1, "n_intervals": 1, "total_duration_s": 1.0, "source_segments": [], "status": "pending", "merge_events": []}
            ], "consolidation_summary": {"merge_count_total": 0}}).to_string(),
            json!({"candidates": [], "consolidation_summary": {"merge_count_total": 0}}).to_string(),
            json!({"next_id": 1, "candidates": [
                {"cand_id": 1, "centroid": [1.0, 0.0], "n_segments": 1, "n_intervals": 1, "total_duration_s": 1.0, "source_segments": [], "status": "pending", "merge_events": []}
            ], "consolidation_summary": {"merge_count_total": 0}}).to_string(),
            json!({"next_id": 2, "candidates": [], "consolidation_summary": {"merge_count_total": -1}}).to_string(),
        ];

        for bad in &bad_payloads {
            let _ = fs::remove_dir_all(&journal);
            fs::create_dir_all(journal.join("awareness")).unwrap();
            fs::write(&store_path, bad.as_bytes()).unwrap();

            let mut tracker = CandidateTracker::new(&journal);
            let input = ClusterInput {
                source_segment: json!({
                    "day": "20260101",
                    "stream_layout": "direct",
                    "stream": "_default",
                    "segment_key": "seg1",
                    "source": "audio",
                    "cluster_label": 1
                }),
                durations_s: vec![1.0],
                embeddings: vec![vec![1.0, 0.0]],
            };
            let res = tracker.add_transcribed_clusters(&[input]);
            assert!(res.is_err(), "payload should be rejected: {bad}");
            assert_eq!(&fs::read_to_string(&store_path).unwrap(), bad);
        }

        let _ = fs::remove_dir_all(journal);
    }

    #[test]
    fn width_mismatch_in_memory_and_bytes_unchanged() {
        let journal =
            PathBuf::from("/var/tmp").join(format!("solstone-cand-width-{}", std::process::id()));
        let _ = fs::remove_dir_all(&journal);
        fs::create_dir_all(journal.join("awareness")).unwrap();

        let cand = CandidateProfile {
            cand_id: 1,
            centroid: vec![1.0, 0.0],
            n_segments: 1,
            n_intervals: 1,
            total_duration_s: 1.0,
            source_segments: vec![
                json!({"day":"20260101","stream_layout":"direct","stream":"_default","segment_key":"seg1","source":"audio","cluster_label":1}),
            ],
            confirmed_entity: None,
            status: "pending".to_owned(),
            merge_events: vec![],
        };
        let (_, mut tracker) = tracker_with_candidates("width-mismatch", vec![cand]);
        let store_path = tracker.store_path.clone();
        let bytes_before = fs::read(&store_path).unwrap();

        let bad_input = ClusterInput {
            source_segment: json!({
                "day": "20260102",
                "stream_layout": "direct",
                "stream": "_default",
                "segment_key": "seg2",
                "source": "audio",
                "cluster_label": 1
            }),
            durations_s: vec![1.0],
            embeddings: vec![vec![1.0, 0.0, 0.0]],
        };
        let err = tracker.add_transcribed_clusters(&[bad_input]).unwrap_err();
        assert!(matches!(err, CandidateTrackerError::CentroidWidth));
        assert_eq!(tracker.candidates().len(), 1);
        assert_eq!(tracker.candidates()[0].centroid.len(), 2);
        assert_eq!(fs::read(&store_path).unwrap(), bytes_before);

        let _ = fs::remove_dir_all(journal);
    }

    #[test]
    fn missing_pool_created_and_load_strict_accepts() {
        let journal =
            PathBuf::from("/var/tmp").join(format!("solstone-cand-missing-{}", std::process::id()));
        let _ = fs::remove_dir_all(&journal);
        fs::create_dir_all(journal.join("awareness")).unwrap();

        let mut tracker = CandidateTracker::new(&journal);
        let input = ClusterInput {
            source_segment: json!({
                "day": "20260101",
                "stream_layout": "direct",
                "stream": "_default",
                "segment_key": "seg1",
                "source": "audio",
                "cluster_label": 1
            }),
            durations_s: vec![1.0],
            embeddings: vec![vec![1.0, 0.0]],
        };
        tracker.add_transcribed_clusters(&[input]).unwrap();

        let mut strict_tracker = CandidateTracker::new(&journal);
        assert!(strict_tracker.load_strict().unwrap());
        assert_eq!(strict_tracker.candidates().len(), 1);

        let _ = fs::remove_dir_all(journal);
    }

    #[test]
    fn nothing_admitted_spread_above_threshold_bytes_and_mtime_unchanged() {
        let journal =
            PathBuf::from("/var/tmp").join(format!("solstone-cand-spread-{}", std::process::id()));
        let _ = fs::remove_dir_all(&journal);
        fs::create_dir_all(journal.join("awareness")).unwrap();

        let mut tracker = CandidateTracker::new(&journal);
        let store_path = journal.join("awareness/speaker_candidates.json");
        let input = ClusterInput {
            source_segment: json!({
                "day": "20260101",
                "stream_layout": "direct",
                "stream": "_default",
                "segment_key": "seg1",
                "source": "audio",
                "cluster_label": 1
            }),
            durations_s: vec![1.0, 1.0],
            embeddings: vec![vec![1.0, 0.0], vec![0.0, 1.0]],
        };
        let res = tracker.add_transcribed_clusters(&[input]).unwrap();
        assert!(res.is_none());
        assert!(!store_path.exists());

        let _ = fs::remove_dir_all(journal);
    }

    #[test]
    fn density_29_to_30_merge_hook_and_consolidation() {
        let cand = CandidateProfile {
            cand_id: 1,
            centroid: vec![1.0, 0.0],
            n_segments: 1,
            n_intervals: 29,
            total_duration_s: 29.0,
            source_segments: vec![
                json!({"day":"20260101","stream_layout":"direct","stream":"_default","segment_key":"seg0","source":"audio","cluster_label":1}),
            ],
            confirmed_entity: None,
            status: "pending".to_owned(),
            merge_events: vec![],
        };
        let (journal, mut tracker) = tracker_with_candidates("29to30", vec![cand]);
        let store_path = tracker.store_path.clone();

        let hook_runs = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let hook_runs_clone = hook_runs.clone();
        let path_clone = store_path.clone();
        let _guard = set_post_feed_hook(move || {
            let lock_res = hold_lock(
                &path_clone,
                LockOptions {
                    timeout: Duration::ZERO,
                    ..Default::default()
                },
            );
            assert!(lock_res.is_ok(), "feed lock must be released before hook");
            hook_runs_clone.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        });

        let input = ClusterInput {
            source_segment: json!({
                "day": "20260101",
                "stream_layout": "direct",
                "stream": "_default",
                "segment_key": "seg1",
                "source": "audio",
                "cluster_label": 1
            }),
            durations_s: vec![1.0],
            embeddings: vec![vec![1.0, 0.0]],
        };
        let res = tracker
            .add_transcribed_clusters(std::slice::from_ref(&input))
            .unwrap();
        assert!(res.is_some());
        assert_eq!(hook_runs.load(std::sync::atomic::Ordering::SeqCst), 1);

        // Second feed of same identity does not enter hook
        let res2 = tracker.add_transcribed_clusters(&[input]).unwrap();
        assert!(res2.is_none());
        assert_eq!(hook_runs.load(std::sync::atomic::Ordering::SeqCst), 1);

        let _ = fs::remove_dir_all(journal);
    }

    #[test]
    fn feed_that_lands_at_29_hook_does_not_run() {
        let cand = CandidateProfile {
            cand_id: 1,
            centroid: vec![1.0, 0.0],
            n_segments: 1,
            n_intervals: 20,
            total_duration_s: 20.0,
            source_segments: vec![
                json!({"day":"20260101","stream_layout":"direct","stream":"_default","segment_key":"seg0","source":"audio","cluster_label":1}),
            ],
            confirmed_entity: None,
            status: "pending".to_owned(),
            merge_events: vec![],
        };
        let (journal, mut tracker) = tracker_with_candidates("lands29", vec![cand]);

        let hook_runs = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let hook_runs_clone = hook_runs.clone();
        let _guard = set_post_feed_hook(move || {
            hook_runs_clone.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        });

        let input = ClusterInput {
            source_segment: json!({
                "day": "20260101",
                "stream_layout":"direct",
                "stream": "_default",
                "segment_key": "seg1",
                "source": "audio",
                "cluster_label": 1
            }),
            durations_s: vec![1.0; 9],
            embeddings: vec![vec![1.0, 0.0]; 9],
        };
        let res = tracker.add_transcribed_clusters(&[input]).unwrap();
        assert!(res.is_none());
        assert_eq!(hook_runs.load(std::sync::atomic::Ordering::SeqCst), 0);

        let _ = fs::remove_dir_all(journal);
    }

    #[test]
    fn create_at_ge_30_hook_and_consolidation() {
        let cand = CandidateProfile {
            cand_id: 1,
            centroid: vec![1.0, 0.0],
            n_segments: 1,
            n_intervals: 30,
            total_duration_s: 30.0,
            source_segments: vec![
                json!({"day":"20260101","stream_layout":"direct","stream":"_default","segment_key":"seg0","source":"audio","cluster_label":1}),
            ],
            confirmed_entity: None,
            status: "pending".to_owned(),
            merge_events: vec![],
        };
        let (journal, mut tracker) = tracker_with_candidates("create30", vec![cand]);
        let store_path = tracker.store_path.clone();

        let hook_runs = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let hook_runs_clone = hook_runs.clone();
        let path_clone = store_path.clone();
        let _guard = set_post_feed_hook(move || {
            let lock_res = hold_lock(
                &path_clone,
                LockOptions {
                    timeout: Duration::ZERO,
                    ..Default::default()
                },
            );
            assert!(lock_res.is_ok());
            hook_runs_clone.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        });

        // cos(theta) = 0.68 -> dot in [0.65, 0.72)
        let c2_vec = vec![0.68, (1.0f32 - 0.68 * 0.68).sqrt()];
        let input = ClusterInput {
            source_segment: json!({
                "day": "20260101",
                "stream_layout": "direct",
                "stream": "_default",
                "segment_key": "seg1",
                "source": "audio",
                "cluster_label": 1
            }),
            durations_s: vec![1.0; 30],
            embeddings: vec![c2_vec; 30],
        };
        let res = tracker.add_transcribed_clusters(&[input]).unwrap();
        assert!(res.is_some());
        let report = res.unwrap();
        assert_eq!(report["merged"].as_u64().unwrap(), 1);
        assert_eq!(hook_runs.load(std::sync::atomic::Ordering::SeqCst), 1);

        let _ = fs::remove_dir_all(journal);
    }

    #[test]
    fn consolidation_timeout_hook_holds_lock_across_consolidate() {
        struct TestLogger;
        static CAPTURED_LOGS: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
        impl log::Log for TestLogger {
            fn enabled(&self, metadata: &log::Metadata) -> bool {
                metadata.level() <= log::Level::Warn
            }
            fn log(&self, record: &log::Record) {
                if self.enabled(record.metadata())
                    && let Ok(mut logs) = CAPTURED_LOGS.lock()
                {
                    logs.push(record.args().to_string());
                }
            }
            fn flush(&self) {}
        }
        static TEST_LOGGER: TestLogger = TestLogger;
        static LOGGER_INIT: std::sync::Once = std::sync::Once::new();
        LOGGER_INIT.call_once(|| {
            let _ = log::set_logger(&TEST_LOGGER);
            log::set_max_level(log::LevelFilter::Warn);
        });

        if let Ok(mut logs) = CAPTURED_LOGS.lock() {
            logs.clear();
        }

        let cand = CandidateProfile {
            cand_id: 1,
            centroid: vec![1.0, 0.0],
            n_segments: 1,
            n_intervals: 29,
            total_duration_s: 29.0,
            source_segments: vec![
                json!({"day":"20260101","stream_layout":"direct","stream":"_default","segment_key":"seg0","source":"audio","cluster_label":1}),
            ],
            confirmed_entity: None,
            status: "pending".to_owned(),
            merge_events: vec![],
        };
        let (journal, mut tracker) = tracker_with_candidates("timeout", vec![cand]);
        let store_path = tracker.store_path.clone();

        let (tx, rx) = std::sync::mpsc::channel();
        let rx_arc = std::sync::Arc::new(std::sync::Mutex::new(rx));
        let path_clone = store_path.clone();
        let _guard = set_post_feed_hook(move || {
            let path = path_clone.clone();
            let (tx_locked, rx_locked) = std::sync::mpsc::channel();
            let rx_done = rx_arc.clone();
            std::thread::spawn(move || {
                let _lock = hold_lock(&path, LockOptions::default()).unwrap();
                tx_locked.send(()).unwrap();
                let rx_guard = rx_done.lock().unwrap();
                let _ = rx_guard.recv();
            });
            rx_locked.recv().unwrap();
        });

        let input = ClusterInput {
            source_segment: json!({
                "day": "20260101",
                "stream_layout": "direct",
                "stream": "_default",
                "segment_key": "seg1",
                "source": "audio",
                "cluster_label": 1
            }),
            durations_s: vec![1.0],
            embeddings: vec![vec![1.0, 0.0]],
        };
        let res = tracker
            .add_transcribed_clusters_with_lock_options(
                &[input],
                LockOptions::default(),
                LockOptions {
                    timeout: Duration::ZERO,
                    ..Default::default()
                },
            )
            .unwrap();
        assert!(res.is_none());
        let _ = tx.send(());

        let logs = CAPTURED_LOGS.lock().unwrap().clone();
        assert!(
            logs.iter()
                .any(|msg| msg == "speaker candidate consolidation failed after the pool update"),
            "expected warning log not found in {logs:?}"
        );

        // Verify post-feed write is still there
        let mut loaded = CandidateTracker::new(&journal);
        assert!(loaded.load_strict().unwrap());
        assert_eq!(loaded.candidates().len(), 1);
        assert_eq!(loaded.candidates()[0].n_intervals, 30);

        let _ = fs::remove_dir_all(journal);
    }
}
