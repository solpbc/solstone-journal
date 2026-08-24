// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Whole-journal voiceprint similarity scanning for speaker name variants.

use std::path::Path;

use serde::Serialize;
use solstone_core_entity::{
    EncoderIdentity, EntityMergeOptions, EntityStoreError, commit_entity_merge,
    is_admissible_person, load_all_journal_entities, load_entity_voiceprints_file,
    normalize_embedding,
};
use solstone_core_entity_matching::is_name_variant_match;

use crate::bootstrap::{NAME_MERGE_THRESHOLD, dot};

const EMBEDDING_WIDTH: usize = 256;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct NameVariantCandidate {
    pub source_id: String,
    pub source_label: String,
    pub target_id: String,
    pub target_label: String,
    pub similarity: f64,
    pub readiness: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct NameVariantMatch {
    pub name_a: String,
    pub name_b: String,
    pub similarity: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct NameVariantAmbiguityCandidate {
    pub name: String,
    pub similarity: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct NameVariantAmbiguity {
    pub name: String,
    pub candidates: Vec<NameVariantAmbiguityCandidate>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct NameVariantScan {
    pub candidates: Vec<NameVariantCandidate>,
    pub entities_with_voiceprints: usize,
    pub pairs_compared: usize,
    pub matches_found: Vec<NameVariantMatch>,
    pub ambiguous: Vec<NameVariantAmbiguity>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct NameVariantAutoMerged {
    pub canonical: String,
    pub alias: String,
    pub similarity: f64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct ResolveNameVariantsStats {
    pub entities_with_voiceprints: usize,
    pub pairs_compared: usize,
    pub matches_found: Vec<NameVariantMatch>,
    pub auto_merged: Vec<NameVariantAutoMerged>,
    pub ambiguous: Vec<NameVariantAmbiguity>,
    pub errors: Vec<String>,
}

#[derive(Debug)]
struct Centroid {
    id: String,
    name: String,
    values: Vec<f32>,
}

/// Find unambiguous speaker name-variant merge candidates without mutating the journal.
pub fn detect_name_variant_candidates(
    journal_root: &Path,
) -> Result<NameVariantScan, EntityStoreError> {
    let mut centroids = Vec::new();
    for entity in load_all_journal_entities(journal_root)? {
        if !is_admissible_person(&entity) || entity.is_principal() {
            continue;
        }
        let name = entity
            .value
            .get("name")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        if name.is_empty() {
            continue;
        }
        let Some(archive) = load_entity_voiceprints_file(journal_root, &entity.id) else {
            continue;
        };
        if archive.rows == 0 {
            continue;
        }
        let Some(values) = plain_mean_centroid(&archive.embeddings) else {
            continue;
        };
        centroids.push(Centroid {
            id: entity.id,
            name: name.to_owned(),
            values,
        });
    }

    let mut scan = NameVariantScan {
        entities_with_voiceprints: centroids.len(),
        ..NameVariantScan::default()
    };
    let mut neighbors = vec![Vec::<(usize, f64)>::new(); centroids.len()];
    for i in 0..centroids.len() {
        for j in (i + 1)..centroids.len() {
            scan.pairs_compared += 1;
            let similarity = f64::from(dot(&centroids[i].values, &centroids[j].values));
            if meets_name_merge_threshold(similarity) {
                let rounded = round_similarity(similarity);
                scan.matches_found.push(NameVariantMatch {
                    name_a: centroids[i].name.clone(),
                    name_b: centroids[j].name.clone(),
                    similarity: rounded,
                });
                neighbors[i].push((j, similarity));
                neighbors[j].push((i, similarity));
            }
        }
    }

    for (i, matches) in neighbors.iter().enumerate() {
        if matches.len() > 1 {
            scan.ambiguous.push(ambiguity_for(&centroids, matches, i));
        }
    }

    for i in 0..centroids.len() {
        if neighbors[i].len() != 1 {
            continue;
        }
        let (j, similarity) = neighbors[i][0];
        if i >= j || neighbors[j].len() != 1 {
            continue;
        }
        let (target, source) =
            if centroids[i].name.chars().count() >= centroids[j].name.chars().count() {
                (i, j)
            } else {
                (j, i)
            };
        let rounded = round_similarity(similarity);
        if is_name_variant_match(&centroids[source].name, &centroids[target].name) {
            scan.candidates.push(NameVariantCandidate {
                source_id: centroids[source].id.clone(),
                source_label: centroids[source].name.clone(),
                target_id: centroids[target].id.clone(),
                target_label: centroids[target].name.clone(),
                similarity: rounded,
                readiness: "ready".to_owned(),
            });
        } else {
            scan.ambiguous.push(NameVariantAmbiguity {
                name: centroids[i].name.clone(),
                candidates: vec![NameVariantAmbiguityCandidate {
                    name: centroids[j].name.clone(),
                    similarity: rounded,
                }],
            });
        }
    }

    Ok(scan)
}

/// Apply a completed scan's ready candidates when requested, retaining per-merge failures.
pub fn resolve_name_variant_candidates(
    scan: NameVariantScan,
    journal_root: &Path,
    commit: bool,
    encoder: &EncoderIdentity,
) -> ResolveNameVariantsStats {
    let mut stats = ResolveNameVariantsStats {
        entities_with_voiceprints: scan.entities_with_voiceprints,
        pairs_compared: scan.pairs_compared,
        matches_found: scan.matches_found,
        ambiguous: scan.ambiguous,
        ..ResolveNameVariantsStats::default()
    };
    for candidate in scan.candidates {
        if commit
            && let Err(error) = commit_entity_merge(
                journal_root,
                &candidate.source_id,
                &candidate.target_id,
                EntityMergeOptions {
                    keep_source_as_aka: true,
                },
                encoder,
            )
        {
            stats.errors.push(format!(
                "Failed to merge {} -> {}: {error}",
                candidate.source_label, candidate.target_label
            ));
            continue;
        }
        stats.auto_merged.push(NameVariantAutoMerged {
            canonical: candidate.target_label,
            alias: candidate.source_label,
            similarity: candidate.similarity,
        });
    }
    stats
}

fn plain_mean_centroid(embeddings: &[f32]) -> Option<Vec<f32>> {
    let mut sum = vec![0.0_f32; EMBEDDING_WIDTH];
    let mut count = 0_usize;
    for row in embeddings.chunks_exact(EMBEDDING_WIDTH) {
        let Some(normalized) = normalize_embedding(row) else {
            continue;
        };
        for (sum, value) in sum.iter_mut().zip(normalized) {
            *sum += value;
        }
        count += 1;
    }
    if count == 0 {
        return None;
    }
    for value in &mut sum {
        *value /= count as f32;
    }
    normalize_embedding(&sum)
}

fn ambiguity_for(
    centroids: &[Centroid],
    neighbors: &[(usize, f64)],
    index: usize,
) -> NameVariantAmbiguity {
    NameVariantAmbiguity {
        name: centroids[index].name.clone(),
        candidates: neighbors
            .iter()
            .map(|(neighbor, similarity)| NameVariantAmbiguityCandidate {
                name: centroids[*neighbor].name.clone(),
                similarity: round_similarity(*similarity),
            })
            .collect(),
    }
}

fn round_similarity(similarity: f64) -> f64 {
    (similarity * 10_000.0).round() / 10_000.0
}

fn meets_name_merge_threshold(similarity: f64) -> bool {
    similarity >= NAME_MERGE_THRESHOLD
}

#[cfg(test)]
mod tests {
    use super::meets_name_merge_threshold;
    use crate::bootstrap::NAME_MERGE_THRESHOLD;

    #[test]
    fn ac4_threshold_is_inclusive() {
        assert!(meets_name_merge_threshold(NAME_MERGE_THRESHOLD));
        assert!(!meets_name_merge_threshold(f64::from(0.899_999_f32)));
    }
}
