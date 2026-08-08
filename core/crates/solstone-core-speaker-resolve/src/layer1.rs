// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Owner-centroid separation, the first attribution layer.

use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use solstone_core_entity::normalize_embedding;

use crate::owner_centroid::OwnerCentroid;
use crate::voiceprint_centroid::{VoiceprintCentroidCache, VoiceprintLoadGap};

/// In-progress label state shared with the later attribution layers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Label {
    pub sentence_id: i64,
    pub speaker: Option<String>,
    pub confidence: Option<String>,
    pub method: Option<String>,
    pub owner_margin_declined: Option<bool>,
    pub acoustic_margin_declined: Option<bool>,
}

/// Layer 1 output required to continue through Layers 2 and 3.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layer1Result {
    pub labels: BTreeMap<i64, Label>,
    pub non_owner_sids: Vec<i64>,
    pub margin_declined_sids: HashSet<i64>,
}

/// Dependencies and per-call inputs for owner-centroid separation.
pub struct OwnerSeparationContext<'a> {
    pub owner: &'a OwnerCentroid,
    pub owner_entity_id: &'a str,
    pub margin_non_principal_entity_ids: &'a [String],
    pub journal_root: &'a Path,
    pub stream: &'a str,
    pub now_ms: i64,
}

/// Separate owner statements from non-owner statements using the persisted centroid.
pub fn separate_owner_statements(
    statements: &[(i64, Vec<f32>)],
    context: OwnerSeparationContext<'_>,
    cache: &mut VoiceprintCentroidCache,
    voiceprint_gaps: &mut Vec<VoiceprintLoadGap>,
) -> Layer1Result {
    let mut labels = statements
        .iter()
        .map(|(sentence_id, _)| {
            (
                *sentence_id,
                Label {
                    sentence_id: *sentence_id,
                    speaker: None,
                    confidence: None,
                    method: None,
                    owner_margin_declined: None,
                    acoustic_margin_declined: None,
                },
            )
        })
        .collect::<BTreeMap<_, _>>();
    let mut non_owner_sids = Vec::new();
    let mut margin_declined_sids = HashSet::new();

    for (sentence_id, embedding) in statements {
        let Some(normalized) = normalize_embedding(embedding) else {
            continue;
        };
        let score = dot(&normalized, &context.owner.centroid);
        let mut owner_claimed = score >= context.owner.threshold;
        if owner_claimed && let Some(margin) = context.owner.margin {
            let mut best_non_owner_cos = f32::NEG_INFINITY;
            for entity_id in context.margin_non_principal_entity_ids {
                let entry = cache.entry_for(
                    context.journal_root,
                    entity_id,
                    context.stream,
                    context.now_ms,
                    voiceprint_gaps,
                );
                if let Some(centroid) = entry.centroid {
                    best_non_owner_cos = best_non_owner_cos.max(dot(&normalized, &centroid));
                }
            }
            owner_claimed = score - best_non_owner_cos >= margin;
        }
        if owner_claimed {
            labels.insert(
                *sentence_id,
                Label {
                    sentence_id: *sentence_id,
                    speaker: Some(context.owner_entity_id.to_owned()),
                    confidence: Some("high".to_owned()),
                    method: Some("owner_centroid".to_owned()),
                    owner_margin_declined: None,
                    acoustic_margin_declined: None,
                },
            );
        } else {
            if score >= context.owner.threshold && context.owner.margin.is_some() {
                labels
                    .get_mut(sentence_id)
                    .expect("label initialized for every statement")
                    .owner_margin_declined = Some(true);
                margin_declined_sids.insert(*sentence_id);
            }
            non_owner_sids.push(*sentence_id);
        }
    }
    Layer1Result {
        labels,
        non_owner_sids,
        margin_declined_sids,
    }
}

fn dot(left: &[f32], right: &[f32]) -> f32 {
    left.iter()
        .zip(right)
        .map(|(left, right)| left * right)
        .sum()
}
