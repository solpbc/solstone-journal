// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Reusable same-stream-preferred voiceprint centroid construction.

use std::collections::HashMap;
use std::path::Path;

use serde_json::Value;
use solstone_core_entity::{
    normalize_embedding, read_identity_map, try_load_entity_voiceprints_file,
    try_load_entity_voiceprints_in_dir,
};

use solstone_core_speaker_id::calibration::VP_DECAY_LAMBDA;

const SAME_STREAM_MIN_ROWS: usize = 5;
const MILLIS_PER_DAY: f64 = 86_400_000.0;

/// Cached, one-entity voiceprint centroid state.
#[derive(Debug, Clone, PartialEq)]
pub struct VoiceprintCentroidEntry {
    pub centroid: Option<Vec<f32>>,
    pub embedding_count: usize,
    pub usable: bool,
}

/// One unreadable voiceprint artifact observed during attribution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VoiceprintLoadGap {
    pub source: String,
    pub reason: String,
    pub entity_id: String,
}

/// Per-attribution-call memoization for voiceprint centroid reads.
#[derive(Debug, Default)]
pub struct VoiceprintCentroidCache {
    entries: HashMap<String, VoiceprintCentroidEntry>,
    /// Effective entity id to on-disk entity directory, read once per call.
    ///
    /// Resolving an id through the entity store rebuilds this map from every
    /// `entities/*/entity.json` on disk. Attribution resolves one id per
    /// admissible person, so doing that per lookup is quadratic in the size of
    /// the journal; holding the map here keeps it to a single scan.
    directories: Option<HashMap<String, String>>,
}

impl VoiceprintCentroidCache {
    /// Load and cache the centroid for one entity.
    pub fn entry_for(
        &mut self,
        journal_root: &Path,
        entity_id: &str,
        stream: &str,
        now_ms: i64,
        gaps: &mut Vec<VoiceprintLoadGap>,
    ) -> VoiceprintCentroidEntry {
        if let Some(entry) = self.entries.get(entity_id) {
            return entry.clone();
        }
        let loaded = {
            let directories = self.directories.get_or_insert_with(|| {
                read_identity_map(journal_root)
                    .map(|map| map.resolved)
                    .unwrap_or_default()
            });
            match directories.get(entity_id) {
                Some(entity_dir) => try_load_entity_voiceprints_in_dir(journal_root, entity_dir),
                // Absent from the snapshot: fall back to a fresh resolve, which
                // still sees an entity written after the snapshot was taken.
                None => try_load_entity_voiceprints_file(journal_root, entity_id),
            }
        };
        let entry = match loaded {
            Ok(None) => empty_entry(),
            Ok(Some(archive)) if archive.rows == 0 => empty_entry(),
            Ok(Some(archive)) => {
                let rows = archive
                    .embeddings
                    .chunks_exact(256)
                    .zip(archive.metadata.iter())
                    .map(|(embedding, metadata)| {
                        let metadata = serde_json::from_str(metadata).unwrap_or(Value::Null);
                        (embedding.to_vec(), metadata)
                    })
                    .collect::<Vec<_>>();
                let centroid = decay_weighted_centroid(&rows, stream, now_ms);
                VoiceprintCentroidEntry {
                    usable: centroid.is_some(),
                    centroid,
                    embedding_count: archive.rows,
                }
            }
            Err(_) => {
                gaps.push(VoiceprintLoadGap {
                    source: "voiceprint".to_owned(),
                    reason: "unreadable".to_owned(),
                    entity_id: entity_id.to_owned(),
                });
                empty_entry()
            }
        };
        self.entries.insert(entity_id.to_owned(), entry.clone());
        entry
    }
}

/// Build a same-stream-preferred, decay-weighted normalized centroid.
pub fn decay_weighted_centroid(
    rows: &[(Vec<f32>, Value)],
    stream: &str,
    now_ms: i64,
) -> Option<Vec<f32>> {
    let normalized = rows
        .iter()
        .filter_map(|(embedding, metadata)| {
            normalize_embedding(embedding).map(|embedding| (embedding, metadata))
        })
        .collect::<Vec<_>>();
    let same_stream = normalized
        .iter()
        .filter(|(_, metadata)| metadata.get("stream").and_then(Value::as_str) == Some(stream))
        .collect::<Vec<_>>();
    let basis = if same_stream.len() >= SAME_STREAM_MIN_ROWS {
        same_stream
    } else {
        normalized.iter().collect()
    };
    let (first, _) = basis.first()?;
    let mut weighted_sum = vec![0.0_f32; first.len()];
    let mut total_weight = 0.0_f64;
    for (embedding, metadata) in basis {
        let added_at_ms = added_at_ms(metadata).unwrap_or(now_ms as f64);
        let age_days = (((now_ms as f64) - added_at_ms) / MILLIS_PER_DAY).max(0.0);
        let weight = (-VP_DECAY_LAMBDA * age_days).exp();
        for (sum, value) in weighted_sum.iter_mut().zip(embedding) {
            *sum += *value * weight as f32;
        }
        total_weight += weight;
    }
    if total_weight <= 0.0 {
        return None;
    }
    for value in &mut weighted_sum {
        *value /= total_weight as f32;
    }
    normalize_embedding(&weighted_sum)
}

fn empty_entry() -> VoiceprintCentroidEntry {
    VoiceprintCentroidEntry {
        centroid: None,
        embedding_count: 0,
        usable: false,
    }
}

fn added_at_ms(metadata: &Value) -> Option<f64> {
    match metadata.get("added_at")? {
        Value::Number(value) => value.as_f64(),
        Value::String(value) => value.parse().ok(),
        Value::Bool(value) => Some(f64::from(*value)),
        _ => None,
    }
}
