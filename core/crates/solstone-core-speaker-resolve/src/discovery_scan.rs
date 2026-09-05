// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Shared native speaker discovery and derived-cache publication.

use crate::segment_catalog::catalog_journal;
use chrono::Utc;
use serde_json::{Value, json};
use solstone_core_journal_io::SegmentLayout;
use solstone_core_system::lifecycle::HostedServiceParentRuntime;
use std::collections::{BTreeSet, HashSet};
use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

const DISCOVERY_UNIT_NORM_TOLERANCE: f32 = 1.0e-3;
const MIN_CLUSTER_SIZE: usize = 5;
pub const MAX_UNMATCHED_EMBEDDINGS: usize = 10_000;
static DISCOVERY_CACHE_TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Outcome of a scan, including owner admission states that preserve the cache.
pub enum DiscoveryRefresh {
    IdentityInvalid,
    NoConfirmedOwner,
    Refreshed {
        clusters: std::collections::BTreeMap<String, Vec<Value>>,
        dropped_invalid: usize,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum DiscoveryRefreshError {
    #[error("speaker discovery failed: {0}")]
    Input(String),
    #[error(transparent)]
    Helper(#[from] crate::discovery_helper::DiscoveryHelperError),
}

/// Refresh the discovery cache through the same bounded native clustering helper.
pub fn refresh_discovery_cache(
    root: &Path,
    hosted_parent: Option<Arc<HostedServiceParentRuntime>>,
) -> Result<DiscoveryRefresh, DiscoveryRefreshError> {
    refresh_with_cluster(root, |embeddings| {
        crate::discovery_helper::discovery_cluster(embeddings, hosted_parent)
    })
}

fn refresh_with_cluster(
    root: &Path,
    cluster: impl FnOnce(
        Vec<Vec<f32>>,
    ) -> Result<Vec<i64>, crate::discovery_helper::DiscoveryHelperError>,
) -> Result<DiscoveryRefresh, DiscoveryRefreshError> {
    let (rows, dropped_invalid) =
        match discovery_candidates(root).map_err(DiscoveryRefreshError::Input)? {
            DiscoveryCandidates::IdentityInvalid => return Ok(DiscoveryRefresh::IdentityInvalid),
            DiscoveryCandidates::NoConfirmedOwner => return Ok(DiscoveryRefresh::NoConfirmedOwner),
            DiscoveryCandidates::Candidates {
                rows,
                dropped_invalid,
            } => (rows, dropped_invalid),
        };
    let clusters = if rows.len() < MIN_CLUSTER_SIZE {
        std::collections::BTreeMap::new()
    } else {
        let rows = cap_discovery_candidates(rows);
        let labels = cluster(rows.iter().map(|row| row.embedding.clone()).collect())?;
        retain_discovery_clusters(&rows, &labels)
    };
    if clusters.is_empty() {
        clear_discovery_cache(root).map_err(DiscoveryRefreshError::Input)?;
    } else {
        write_discovery_cache(root, &clusters).map_err(DiscoveryRefreshError::Input)?;
    }
    Ok(DiscoveryRefresh::Refreshed {
        clusters,
        dropped_invalid,
    })
}

#[derive(Clone, Debug, PartialEq)]
pub struct DiscoveryCandidate {
    pub day: String,
    pub stream_layout: SegmentLayout,
    pub stream: String,
    pub segment_key: String,
    pub source: String,
    pub sentence_id: i64,
    pub embedding: Vec<f32>,
}

pub enum DiscoveryCandidates {
    IdentityInvalid,
    NoConfirmedOwner,
    Candidates {
        rows: Vec<DiscoveryCandidate>,
        dropped_invalid: usize,
    },
}

pub fn discovery_candidates(root: &std::path::Path) -> Result<DiscoveryCandidates, String> {
    let principal = match crate::owner_admission::admitted_owner_id(root) {
        crate::owner_admission::OwnerAdmission::Admitted(id) => id,
        crate::owner_admission::OwnerAdmission::Invalid => {
            return Ok(DiscoveryCandidates::IdentityInvalid);
        }
    };
    let owner = match crate::owner_centroid::load_owner_centroid(root, &principal) {
        Ok(Some(owner)) => owner,
        Ok(None) => return Ok(DiscoveryCandidates::NoConfirmedOwner),
        Err(
            crate::owner_centroid::OwnerCentroidError::IdentityInvalid
            | crate::owner_centroid::OwnerCentroidError::TargetMismatch { .. },
        ) => return Ok(DiscoveryCandidates::IdentityInvalid),
        Err(error) => return Err(error.to_string()),
    };
    let mut rows = Vec::new();
    let mut dropped = 0;
    for segment in catalog_journal(root).map_err(|error| error.to_string())? {
        let labels_path = segment.path.join("talents/speaker_labels.json");
        let attributed = match fs::read(&labels_path) {
            Ok(bytes) => {
                let value = serde_json::from_slice::<Value>(&bytes).map_err(|error| {
                    format!("failed to parse {}: {error}", labels_path.display())
                })?;
                let labels = value
                    .get("labels")
                    .and_then(Value::as_array)
                    .ok_or_else(|| {
                        format!("invalid speaker labels at {}", labels_path.display())
                    })?;
                let mut attributed = BTreeSet::new();
                for row in labels {
                    if row.get("speaker").is_some_and(|value| !value.is_null()) {
                        let sentence_id = row
                            .get("sentence_id")
                            .and_then(Value::as_i64)
                            .ok_or_else(|| {
                                format!(
                                    "invalid attributed sentence id at {}",
                                    labels_path.display()
                                )
                            })?;
                        attributed.insert(sentence_id);
                    }
                }
                attributed
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => BTreeSet::new(),
            Err(error) => {
                return Err(format!("failed to read {}: {error}", labels_path.display()));
            }
        };
        let entries = fs::read_dir(&segment.path)
            .map_err(|error| format!("failed to read {}: {error}", segment.path.display()))?;
        for entry in entries {
            let entry = entry.map_err(|error| {
                format!(
                    "failed to read entry in {}: {error}",
                    segment.path.display()
                )
            })?;
            let path = entry.path();
            let Some(source) = path.file_stem().and_then(|name| name.to_str()) else {
                continue;
            };
            if path.extension().and_then(|ext| ext.to_str()) != Some("npz") {
                continue;
            }
            let Some(file) = solstone_core_speaker_id::embeddings::load_embeddings_file(&path)
                .map_err(|error| format!("failed to load {}: {error}", path.display()))?
            else {
                continue;
            };
            for (sentence_id, embedding) in file.statements {
                if attributed.contains(&sentence_id) {
                    continue;
                }
                let norm = embedding
                    .iter()
                    .map(|value| value * value)
                    .sum::<f32>()
                    .sqrt();
                if !norm.is_finite()
                    || embedding.iter().any(|value| !value.is_finite())
                    || (norm - 1.0).abs() > DISCOVERY_UNIT_NORM_TOLERANCE
                {
                    dropped += 1;
                    continue;
                }
                let score: f32 = embedding
                    .iter()
                    .zip(&owner.centroid)
                    .map(|(left, right)| left * right)
                    .sum();
                if score >= owner.threshold {
                    continue;
                }
                rows.push(DiscoveryCandidate {
                    day: segment.day.clone(),
                    stream_layout: segment.layout,
                    stream: segment.stream.clone(),
                    segment_key: segment.name.clone(),
                    source: source.to_owned(),
                    sentence_id,
                    embedding,
                });
            }
        }
    }
    Ok(DiscoveryCandidates::Candidates {
        rows,
        dropped_invalid: dropped,
    })
}

pub fn retain_discovery_clusters(
    rows: &[DiscoveryCandidate],
    labels: &[i64],
) -> std::collections::BTreeMap<String, Vec<Value>> {
    let mut output = std::collections::BTreeMap::new();
    for label in labels
        .iter()
        .copied()
        .filter(|label| *label != -1)
        .collect::<std::collections::BTreeSet<_>>()
    {
        let selected = rows
            .iter()
            .zip(labels)
            .filter(|(_, current)| **current == label)
            .map(|(row, _)| row)
            .collect::<Vec<_>>();
        if selected
            .iter()
            .map(|row| {
                (
                    &row.day,
                    layout_name(row.stream_layout),
                    &row.stream,
                    &row.segment_key,
                )
            })
            .collect::<std::collections::BTreeSet<_>>()
            .len()
            < 3
        {
            continue;
        }
        let mut mean = vec![0.0; selected[0].embedding.len()];
        for row in &selected {
            for (value, input) in mean.iter_mut().zip(&row.embedding) {
                *value += *input
            }
        }
        let norm = mean.iter().map(|value| value * value).sum::<f32>().sqrt();
        if norm == 0.0 {
            continue;
        }
        for value in &mut mean {
            *value /= norm
        }
        let mut selected = selected;
        selected.sort_by(|left, right| {
            let score = |row: &DiscoveryCandidate| {
                row.embedding
                    .iter()
                    .zip(&mean)
                    .map(|(a, b)| a * b)
                    .sum::<f32>()
            };
            score(right).total_cmp(&score(left))
        });
        output.insert(label.to_string(),selected.into_iter().map(|row|json!({"day":row.day,"stream_layout":layout_name(row.stream_layout),"stream":row.stream,"segment_key":row.segment_key,"source":row.source,"sentence_id":row.sentence_id})).collect());
    }
    output
}

/// Match Python's `default_rng(42).choice(..., replace=False)` admission cap.
///
/// Python sorts the sampled indexes before carrying provenance forward, so the
/// result is stable and independent of the helper's clustering order.
fn cap_discovery_candidates(rows: Vec<DiscoveryCandidate>) -> Vec<DiscoveryCandidate> {
    if rows.len() <= MAX_UNMATCHED_EMBEDDINGS {
        return rows;
    }
    let indexes = numpy_choice_indexes(rows.len(), MAX_UNMATCHED_EMBEDDINGS);
    indexes
        .into_iter()
        .map(|index| rows[index].clone())
        .collect()
}

/// Exact subset selection used by NumPy 2.x's `default_rng(42).choice` here.
///
/// This ports its seeded PCG64 stream, Lemire-bounded draws, and its tail
/// shuffle/Floyd choice split. Callers sort no further: this function returns
/// the same ascending indexes Python produces after `indices.sort()`.
pub fn numpy_choice_indexes(population: usize, size: usize) -> Vec<usize> {
    debug_assert!(size <= population);
    let mut rng = NumpyPcg64::seed_42();
    let mut indexes = if population > 10_000 && size > population / 50 {
        let mut values = (0..population).collect::<Vec<_>>();
        for index in (population - size..population).rev() {
            let selected = rng.bounded_usize(index);
            values.swap(selected, index);
        }
        values.split_off(population - size)
    } else {
        let mut values = Vec::with_capacity(size);
        let mut seen = HashSet::with_capacity(size);
        for value in population - size..population {
            let selected = rng.bounded_usize(value);
            if !seen.insert(selected) {
                values.push(value);
                seen.insert(value);
            } else {
                values.push(selected);
            }
        }
        values
    };
    indexes.sort_unstable();
    indexes
}

struct NumpyPcg64 {
    state: u128,
    increment: u128,
    cached_u32: Option<u32>,
}

impl NumpyPcg64 {
    // State emitted by NumPy's `default_rng(42).bit_generator.state`.
    const SEED_42_STATE: u128 = 274_674_114_334_540_486_603_088_602_300_644_985_544;
    const SEED_42_INCREMENT: u128 = 332_724_090_758_049_132_448_979_897_138_935_081_983;
    const MULTIPLIER: u128 = 0x2360_ed05_1fc6_5da4_4385_df64_9fcc_f645;

    fn seed_42() -> Self {
        Self {
            state: Self::SEED_42_STATE,
            increment: Self::SEED_42_INCREMENT,
            cached_u32: None,
        }
    }

    fn next_u64(&mut self) -> u64 {
        self.state = self
            .state
            .wrapping_mul(Self::MULTIPLIER)
            .wrapping_add(self.increment);
        let high = (self.state >> 64) as u64;
        let low = self.state as u64;
        (high ^ low).rotate_right((high >> 58) as u32)
    }

    fn next_u32(&mut self) -> u32 {
        if let Some(value) = self.cached_u32.take() {
            return value;
        }
        let value = self.next_u64();
        self.cached_u32 = Some((value >> 32) as u32);
        value as u32
    }

    fn bounded_usize(&mut self, inclusive_max: usize) -> usize {
        debug_assert!(inclusive_max <= u32::MAX as usize);
        let range = inclusive_max as u32;
        let range_exclusive = range.wrapping_add(1);
        loop {
            let product = self.next_u32() as u64 * range_exclusive as u64;
            let leftover = product as u32;
            if leftover >= range_exclusive || leftover >= (u32::MAX - range) % range_exclusive {
                return (product >> 32) as usize;
            }
        }
    }
}

fn clear_discovery_cache(root: &Path) -> Result<(), String> {
    for name in [
        "discovery_clusters.json",
        "discovery_clusters.resolved.json",
    ] {
        let path = root.join("awareness").join(name);
        match fs::remove_file(path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.to_string()),
        }
    }
    Ok(())
}

fn write_discovery_cache(
    root: &Path,
    clusters: &std::collections::BTreeMap<String, Vec<Value>>,
) -> Result<(), String> {
    let awareness = root.join("awareness");
    fs::create_dir_all(&awareness).map_err(|error| error.to_string())?;
    let cache = awareness.join("discovery_clusters.json");
    let temp = awareness.join(format!(
        "discovery_clusters.{}.{}.tmp",
        std::process::id(),
        DISCOVERY_CACHE_TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed),
    ));
    let payload = json!({
        "version": Utc::now().to_rfc3339(),
        "clusters": clusters,
    });
    let bytes = serde_json::to_vec_pretty(&payload).map_err(|error| error.to_string())?;
    let result = fs::write(&temp, bytes).and_then(|()| fs::rename(&temp, &cache));
    if let Err(error) = result {
        let _ = fs::remove_file(&temp);
        return Err(error.to_string());
    }
    Ok(())
}

fn layout_name(layout: SegmentLayout) -> &'static str {
    match layout {
        SegmentLayout::Direct => "direct",
        SegmentLayout::Named => "named",
    }
}
