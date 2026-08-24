// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Acoustic speaker-attribution heuristics, the third attribution layer.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::Path;

use solstone_core_entity::{JournalEntity, is_admissible_person, normalize_embedding};

use crate::layer1::Label;
use crate::voiceprint_centroid::{VoiceprintCentroidCache, VoiceprintLoadGap};
use solstone_core_speaker_id::calibration::{
    ACOUSTIC_HIGH, ACOUSTIC_MARGIN_MIN, ACOUSTIC_MEDIUM, CC_CONFIDENCE_GATE, CC_COVERAGE_GATE,
};

/// Inputs loaded once by the attribution orchestrator for acoustic attribution.
pub struct Layer3Inputs<'a> {
    pub labels: BTreeMap<i64, Label>,
    pub non_owner_sids: &'a [i64],
    pub margin_declined_sids: &'a HashSet<i64>,
    pub candidate_entity_ids: &'a BTreeSet<String>,
    pub entities: &'a [JournalEntity],
    pub journal_root: &'a Path,
    pub stream: &'a str,
    pub now_ms: i64,
    pub statements: &'a [(i64, Vec<f32>)],
    pub integer_speakers: &'a HashMap<i64, i64>,
}

/// Acoustic-layer state returned to the attribution orchestrator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layer3Result {
    pub labels: BTreeMap<i64, Label>,
    pub voiceprint_versions: HashMap<String, usize>,
}

/// Apply hybrid-cluster and per-sentence acoustic speaker attribution.
pub fn apply_acoustic_matching(
    inputs: Layer3Inputs<'_>,
    cache: &mut VoiceprintCentroidCache,
    voiceprint_gaps: &mut Vec<VoiceprintLoadGap>,
) -> Layer3Result {
    let mut labels = inputs.labels;
    let unresolved = inputs
        .non_owner_sids
        .iter()
        .copied()
        .filter(|sentence_id| {
            labels
                .get(sentence_id)
                .is_some_and(|label| label.speaker.is_none())
        })
        .collect::<Vec<_>>();
    if unresolved.is_empty() {
        return Layer3Result {
            labels,
            voiceprint_versions: HashMap::new(),
        };
    }

    let vp_entity_ids = if inputs.candidate_entity_ids.is_empty() {
        inputs
            .entities
            .iter()
            .filter(|entity| !entity.is_principal() && is_admissible_person(entity))
            .map(|entity| entity.id.clone())
            .collect::<BTreeSet<_>>()
    } else {
        inputs.candidate_entity_ids.clone()
    };
    let mut voiceprint_versions = HashMap::new();
    let mut matching_centroids = BTreeMap::new();
    for entity_id in vp_entity_ids {
        let entry = cache.entry_for(
            inputs.journal_root,
            &entity_id,
            inputs.stream,
            inputs.now_ms,
            voiceprint_gaps,
        );
        if entry.embedding_count == 0 {
            continue;
        }
        voiceprint_versions.insert(entity_id.clone(), entry.embedding_count);
        if let Some(centroid) = entry.centroid.filter(|_| entry.usable) {
            matching_centroids.insert(entity_id, centroid);
        }
    }

    let sid_to_idx = inputs
        .statements
        .iter()
        .enumerate()
        .map(|(index, (sentence_id, _))| (*sentence_id, index))
        .collect::<HashMap<_, _>>();
    apply_cluster_matches(
        &mut labels,
        &unresolved,
        &sid_to_idx,
        inputs.statements,
        inputs.integer_speakers,
        &matching_centroids,
        inputs.margin_declined_sids,
    );
    apply_sentence_matches(
        &mut labels,
        &unresolved,
        &sid_to_idx,
        inputs.statements,
        &matching_centroids,
        inputs.margin_declined_sids,
    );

    Layer3Result {
        labels,
        voiceprint_versions,
    }
}

fn apply_cluster_matches(
    labels: &mut BTreeMap<i64, Label>,
    unresolved: &[i64],
    sid_to_idx: &HashMap<i64, usize>,
    statements: &[(i64, Vec<f32>)],
    integer_speakers: &HashMap<i64, i64>,
    matching_centroids: &BTreeMap<String, Vec<f32>>,
    margin_declined_sids: &HashSet<i64>,
) {
    let labeled = unresolved
        .iter()
        .copied()
        .filter(|sentence_id| integer_speakers.contains_key(sentence_id))
        .collect::<Vec<_>>();
    if labeled.is_empty()
        || (labeled.len() as f32 / unresolved.len() as f32) < CC_COVERAGE_GATE
        || matching_centroids.is_empty()
    {
        return;
    }

    let mut cluster_members = BTreeMap::<i64, Vec<i64>>::new();
    let mut cluster_embeddings = BTreeMap::<i64, Vec<Vec<f32>>>::new();
    for sentence_id in labeled {
        let Some(index) = sid_to_idx.get(&sentence_id) else {
            continue;
        };
        let Some(embedding) = normalize_embedding(&statements[*index].1) else {
            continue;
        };
        let cluster_id = integer_speakers[&sentence_id];
        cluster_members
            .entry(cluster_id)
            .or_default()
            .push(sentence_id);
        cluster_embeddings
            .entry(cluster_id)
            .or_default()
            .push(embedding);
    }
    let cluster_centroids = cluster_embeddings
        .into_iter()
        .filter_map(|(cluster_id, embeddings)| {
            mean_embedding(&embeddings)
                .and_then(|mean| normalize_embedding(&mean).map(|centroid| (cluster_id, centroid)))
        })
        .collect::<BTreeMap<_, _>>();
    let mut pairs = cluster_centroids
        .iter()
        .flat_map(|(cluster_id, cluster_centroid)| {
            matching_centroids
                .iter()
                .map(move |(entity_id, entity_centroid)| {
                    (
                        dot(cluster_centroid, entity_centroid),
                        *cluster_id,
                        entity_id.clone(),
                    )
                })
        })
        .collect::<Vec<_>>();
    pairs.sort_by(|left, right| {
        right
            .0
            .total_cmp(&left.0)
            .then_with(|| right.1.cmp(&left.1))
            .then_with(|| right.2.cmp(&left.2))
    });

    let mut assigned = BTreeMap::new();
    let mut used_clusters = HashSet::new();
    let mut used_entities = HashSet::new();
    for (score, cluster_id, entity_id) in &pairs {
        if used_clusters.contains(cluster_id) || used_entities.contains(entity_id) {
            continue;
        }
        assigned.insert(*cluster_id, (entity_id.clone(), *score));
        used_clusters.insert(*cluster_id);
        used_entities.insert(entity_id.clone());
    }
    if assigned.is_empty() {
        return;
    }
    let mean_confidence =
        assigned.values().map(|(_, score)| score).sum::<f32>() / assigned.len() as f32;
    if mean_confidence < CC_CONFIDENCE_GATE {
        return;
    }
    for (cluster_id, (entity_id, score)) in assigned {
        if score < ACOUSTIC_MEDIUM {
            continue;
        }
        let mut confidence = if score >= ACOUSTIC_HIGH {
            "high"
        } else {
            "medium"
        };
        let mut acoustic_margin_declined = false;
        if confidence == "high" {
            let other_scores = pairs
                .iter()
                .filter(|(_, pair_cluster_id, pair_entity_id)| {
                    *pair_cluster_id == cluster_id && pair_entity_id != &entity_id
                })
                .map(|(pair_score, _, _)| *pair_score)
                .collect::<Vec<_>>();
            acoustic_margin_declined = !passes_acoustic_margin(score, &other_scores);
            if acoustic_margin_declined {
                confidence = "medium";
            }
        }
        for sentence_id in &cluster_members[&cluster_id] {
            let sentence_confidence =
                if margin_declined_sids.contains(sentence_id) && confidence == "high" {
                    "medium"
                } else {
                    confidence
                };
            replace_label(
                labels,
                *sentence_id,
                &entity_id,
                sentence_confidence,
                "acoustic_cluster",
                margin_declined_sids.contains(sentence_id),
                acoustic_margin_declined,
            );
        }
    }
}

fn apply_sentence_matches(
    labels: &mut BTreeMap<i64, Label>,
    unresolved: &[i64],
    sid_to_idx: &HashMap<i64, usize>,
    statements: &[(i64, Vec<f32>)],
    matching_centroids: &BTreeMap<String, Vec<f32>>,
    margin_declined_sids: &HashSet<i64>,
) {
    for sentence_id in unresolved {
        if labels
            .get(sentence_id)
            .is_none_or(|label| label.speaker.is_some())
        {
            continue;
        }
        let Some(index) = sid_to_idx.get(sentence_id) else {
            continue;
        };
        let Some(embedding) = normalize_embedding(&statements[*index].1) else {
            continue;
        };
        let mut best_entity_id = None;
        let mut best_score = 0.0_f32;
        let mut runner_up_scores = Vec::new();
        for (entity_id, centroid) in matching_centroids {
            let score = dot(&embedding, centroid);
            if score > best_score {
                if best_entity_id.is_some() {
                    runner_up_scores.push(best_score);
                }
                best_score = score;
                best_entity_id = Some(entity_id);
            } else {
                runner_up_scores.push(score);
            }
        }
        let Some(entity_id) = best_entity_id else {
            continue;
        };
        if best_score >= ACOUSTIC_HIGH {
            let acoustic_margin_declined = !passes_acoustic_margin(best_score, &runner_up_scores);
            let confidence =
                if margin_declined_sids.contains(sentence_id) || acoustic_margin_declined {
                    "medium"
                } else {
                    "high"
                };
            replace_label(
                labels,
                *sentence_id,
                entity_id,
                confidence,
                "acoustic",
                margin_declined_sids.contains(sentence_id),
                acoustic_margin_declined,
            );
        } else if best_score >= ACOUSTIC_MEDIUM {
            replace_label(
                labels,
                *sentence_id,
                entity_id,
                "medium",
                "acoustic",
                margin_declined_sids.contains(sentence_id),
                false,
            );
        }
    }
}

fn replace_label(
    labels: &mut BTreeMap<i64, Label>,
    sentence_id: i64,
    entity_id: &str,
    confidence: &str,
    method: &str,
    owner_margin_declined: bool,
    acoustic_margin_declined: bool,
) {
    let Some(label) = labels.get_mut(&sentence_id) else {
        return;
    };
    label.speaker = Some(entity_id.to_owned());
    label.confidence = Some(confidence.to_owned());
    label.method = Some(method.to_owned());
    if owner_margin_declined {
        label.owner_margin_declined = Some(true);
    }
    if acoustic_margin_declined {
        label.acoustic_margin_declined = Some(true);
    }
}

fn mean_embedding(embeddings: &[Vec<f32>]) -> Option<Vec<f32>> {
    let first = embeddings.first()?;
    let mut mean = vec![0.0_f32; first.len()];
    for embedding in embeddings {
        for (sum, value) in mean.iter_mut().zip(embedding) {
            *sum += *value;
        }
    }
    for value in &mut mean {
        *value /= embeddings.len() as f32;
    }
    Some(mean)
}

fn dot(left: &[f32], right: &[f32]) -> f32 {
    left.iter()
        .zip(right)
        .map(|(left, right)| left * right)
        .sum()
}

fn passes_acoustic_margin(matched_score: f32, other_scores: &[f32]) -> bool {
    let runner_up = other_scores.iter().copied().fold(0.0_f32, f32::max);
    matched_score - runner_up >= ACOUSTIC_MARGIN_MIN
}

#[cfg(test)]
mod tests {
    use super::passes_acoustic_margin;
    use solstone_core_speaker_id::calibration::ACOUSTIC_MARGIN_MIN;

    #[test]
    fn ac15_acoustic_margin_uses_zero_floor() {
        assert!(passes_acoustic_margin(ACOUSTIC_MARGIN_MIN, &[]));
        assert!(!passes_acoustic_margin(ACOUSTIC_MARGIN_MIN - 0.001, &[]));
    }
}
