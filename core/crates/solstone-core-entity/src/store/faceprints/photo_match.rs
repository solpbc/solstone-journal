// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Side-effect-free matching of still-photo face detections against the
//! owner's existing meeting faceprints.
//!
//! Thresholds were calibrated on an owner-consented evaluation comparing real
//! library photos against real meeting faceprint panels (at cosine >= 0.55 with
//! margin >= 0.08: 100% precision and 0 wrong-person false positives across
//! principal and non-principal meeting identities on both 256 px and 1,063-photo
//! 512 px evaluations, rejecting a sharp lookalike false positive at 0.5324).

use std::collections::BTreeMap;
use std::path::Path;

use super::{Encoder, Result, cross, load_in_dir, panel, valid_embedding};

/// Minimum SFace cosine similarity to link a still-photo face to a meeting person.
pub const PHOTO_MATCH_COSINE_MIN: f32 = 0.55;
/// Minimum YuNet face detector confidence score.
pub const PHOTO_MATCH_DET_SCORE_MIN: f32 = 0.80;
/// Minimum detected face bounding-box width in image pixels.
pub const PHOTO_MATCH_FACE_W_MIN: f32 = 20.0;
/// Minimum cosine gap between top-1 and top-2 eligible meeting identities.
pub const PHOTO_MATCH_MARGIN_MIN: f32 = 0.08;

/// One face detected in a still photo before matching.
#[derive(Clone, Debug, PartialEq)]
pub struct PhotoFaceCandidate {
    pub face_idx: usize,
    pub bbox: [f32; 4],
    pub det_score: f32,
    pub sharpness: f32,
    pub quality: f32,
    pub embedding: Vec<f32>,
    pub crop_jpeg: Option<Vec<u8>>,
}

/// One matched face linking a still-photo detection to an eligible person entity.
#[derive(Clone, Debug, PartialEq)]
pub struct PhotoFaceMatch {
    pub entity_id: String,
    pub is_principal: bool,
    pub face_idx: usize,
    pub bbox: [f32; 4],
    pub det_score: f32,
    pub sharpness: f32,
    pub quality: f32,
    pub similarity: f32,
    pub margin: f32,
    pub embedding: Vec<f32>,
    pub crop_jpeg: Option<Vec<u8>>,
}

/// One eligible meeting faceprint identity target.
#[derive(Clone, Debug, PartialEq)]
pub struct MeetingFaceTarget {
    pub entity_id: String,
    pub is_principal: bool,
    pub centroid: Vec<f32>,
}

/// Read-only index of eligible meeting faceprint targets loaded from the journal.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MeetingFaceIndex {
    pub encoder: Option<Encoder>,
    pub owner_centroid: Option<Vec<f32>>,
    pub targets: Vec<MeetingFaceTarget>,
}

impl MeetingFaceIndex {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.targets.is_empty()
    }
}

/// Load all eligible meeting faceprint targets from the journal with zero
/// side effects (never creates directories, files, or entity rows).
///
/// Only non-contested, non-exclusivity-dropped meeting panels qualify.
/// Centroids are computed strictly from meeting evidence (`Source::Anchored`
/// and `Source::Box`), never from `Source::Photo` rows.
pub fn load_meeting_face_index(journal: &Path) -> Result<MeetingFaceIndex> {
    if !journal.join("entities").is_dir() {
        return Ok(MeetingFaceIndex::default());
    }
    let roster = {
        let _trust = crate::hold_entity_trust_lock(journal).map_err(|e| e.to_string())?;
        crate::live_journal_entities(journal).map_err(|e| e.to_string())?
    };
    let derived = cross::load(journal)?;
    let mut pools = BTreeMap::new();
    let mut encoder: Option<Encoder> = None;

    for (folder, entity) in roster {
        if !crate::is_admissible_person(&entity) {
            continue;
        }
        let (archive, state) = load_in_dir(journal, &folder)?;
        let Some(archive) = archive else {
            continue;
        };
        if encoder
            .as_ref()
            .is_some_and(|e| e != &archive.envelope.encoder)
        {
            return Err("face encoder mismatch across people".into());
        }
        encoder = Some(archive.envelope.encoder);
        pools.insert(entity.id.clone(), (entity, archive.faces, state));
    }

    let owner_centroid = pools
        .values()
        .find(|(e, _, _)| e.is_principal())
        .and_then(|(_, faces, _)| panel::owner_centroid(faces));

    let mut targets = Vec::new();
    for (id, (entity, faces, state)) in &pools {
        let cross_flags = derived.rules.entities.get(id).copied().unwrap_or_default();
        if cross_flags.contested || cross_flags.exclusivity_drop {
            continue;
        }
        if entity.is_principal() {
            if let Some(centroid) = owner_centroid.as_ref() {
                targets.push(MeetingFaceTarget {
                    entity_id: id.clone(),
                    is_principal: true,
                    centroid: centroid.clone(),
                });
            }
            continue;
        }
        let kept = panel::kept_evidence(faces, state, false, owner_centroid.as_deref());
        let distinct = panel::looks(&kept);
        if distinct.len() < panel::MIN_LOOKS {
            continue;
        }
        if let Some(centroid) = panel::centroid(&kept) {
            targets.push(MeetingFaceTarget {
                entity_id: id.clone(),
                is_principal: false,
                centroid,
            });
        }
    }

    Ok(MeetingFaceIndex {
        encoder,
        owner_centroid,
        targets,
    })
}

/// Match detected still-photo faces against `index` with zero side effects.
///
/// Applies the calibrated precision gate (`cosine >= 0.55`, `det_score >= 0.80`,
/// `bbox_w >= 20.0`, `margin >= 0.08`, and owner-centroid exclusion for
/// non-principal identities). At most one detection per photo is linked to a
/// given entity (highest similarity wins).
#[must_use]
pub fn match_photo_faces(
    index: &MeetingFaceIndex,
    candidates: &[PhotoFaceCandidate],
) -> Vec<PhotoFaceMatch> {
    if index.targets.is_empty() || candidates.is_empty() {
        return Vec::new();
    }
    let mut best_by_entity: BTreeMap<String, PhotoFaceMatch> = BTreeMap::new();

    for cand in candidates {
        if cand.det_score < PHOTO_MATCH_DET_SCORE_MIN
            || cand.bbox[2] < PHOTO_MATCH_FACE_W_MIN
            || !valid_embedding(&cand.embedding)
        {
            continue;
        }
        let mut scored: Vec<(&MeetingFaceTarget, f32)> = index
            .targets
            .iter()
            .map(|target| (target, panel::cosine(&cand.embedding, &target.centroid)))
            .collect();
        scored.sort_by(|a, b| b.1.total_cmp(&a.1));

        let Some(&(best_target, best_sim)) = scored.first() else {
            continue;
        };
        if best_sim < PHOTO_MATCH_COSINE_MIN {
            continue;
        }
        let second_sim = scored.get(1).map_or(0.0, |(_, s)| *s);
        let margin = best_sim - second_sim;
        if scored.len() > 1 && margin < PHOTO_MATCH_MARGIN_MIN {
            continue;
        }
        // Keep the same owner-exclusion rule used by meeting panels so an
        // owner face cannot accidentally attach to another person's panel.
        if !best_target.is_principal
            && let Some(owner) = index.owner_centroid.as_deref()
            && panel::cosine(owner, &cand.embedding) >= panel::OWNER
        {
            continue;
        }

        let matched = PhotoFaceMatch {
            entity_id: best_target.entity_id.clone(),
            is_principal: best_target.is_principal,
            face_idx: cand.face_idx,
            bbox: cand.bbox,
            det_score: cand.det_score,
            sharpness: cand.sharpness,
            quality: cand.quality,
            similarity: best_sim,
            margin,
            embedding: cand.embedding.clone(),
            crop_jpeg: cand.crop_jpeg.clone(),
        };
        let replace = best_by_entity
            .get(&matched.entity_id)
            .is_none_or(|prev| matched.similarity > prev.similarity);
        if replace {
            best_by_entity.insert(matched.entity_id.clone(), matched);
        }
    }

    let mut out: Vec<PhotoFaceMatch> = best_by_entity.into_values().collect();
    out.sort_by_key(|a| a.face_idx);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::faceprints::WIDTH;

    fn unit_vec(dim: usize) -> Vec<f32> {
        let mut v = vec![0.0_f32; WIDTH];
        v[dim] = 1.0;
        v
    }

    fn mix(a: &[f32], b: &[f32], weight_a: f32) -> Vec<f32> {
        let weight_b = (1.0 - weight_a * weight_a).max(0.0).sqrt();
        let mut v: Vec<f32> = a
            .iter()
            .zip(b)
            .map(|(x, y)| x * weight_a + y * weight_b)
            .collect();
        let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        for x in &mut v {
            *x /= norm;
        }
        v
    }

    #[test]
    fn match_photo_faces_enforces_cosine_det_width_margin_and_owner_exclusion() {
        let owner = unit_vec(0);
        let alice = unit_vec(1);
        let bob = unit_vec(2);
        let index = MeetingFaceIndex {
            encoder: None,
            owner_centroid: Some(owner.clone()),
            targets: vec![
                MeetingFaceTarget {
                    entity_id: "ent-owner".into(),
                    is_principal: true,
                    centroid: owner.clone(),
                },
                MeetingFaceTarget {
                    entity_id: "ent-alice".into(),
                    is_principal: false,
                    centroid: alice.clone(),
                },
                MeetingFaceTarget {
                    entity_id: "ent-bob".into(),
                    is_principal: false,
                    centroid: bob.clone(),
                },
            ],
        };

        let clear_alice = mix(&alice, &unit_vec(10), 0.72);
        let low_sim = mix(&alice, &unit_vec(11), 0.53); // below 0.55 cosine gate (rejects lookalikes in [0.50, 0.55))
        let ambiguous = mix(&alice, &bob, 0.72); // ~0.72 alice, ~0.69 bob -> margin < 0.08
        let small_box = mix(&bob, &unit_vec(12), 0.85);
        let low_det = mix(&bob, &unit_vec(13), 0.85);
        let clear_owner = mix(&owner, &unit_vec(14), 0.78);

        let candidates = vec![
            PhotoFaceCandidate {
                face_idx: 0,
                bbox: [10.0, 10.0, 48.0, 48.0],
                det_score: 0.92,
                sharpness: 300.0,
                quality: 0.8,
                embedding: clear_alice,
                crop_jpeg: None,
            },
            PhotoFaceCandidate {
                face_idx: 1,
                bbox: [80.0, 10.0, 48.0, 48.0],
                det_score: 0.92,
                sharpness: 300.0,
                quality: 0.8,
                embedding: low_sim,
                crop_jpeg: None,
            },
            PhotoFaceCandidate {
                face_idx: 2,
                bbox: [140.0, 10.0, 48.0, 48.0],
                det_score: 0.92,
                sharpness: 300.0,
                quality: 0.8,
                embedding: ambiguous,
                crop_jpeg: None,
            },
            PhotoFaceCandidate {
                face_idx: 3,
                bbox: [200.0, 10.0, 18.0, 18.0],
                det_score: 0.95,
                sharpness: 300.0,
                quality: 0.8,
                embedding: small_box,
                crop_jpeg: None,
            },
            PhotoFaceCandidate {
                face_idx: 4,
                bbox: [250.0, 10.0, 48.0, 48.0],
                det_score: 0.75,
                sharpness: 300.0,
                quality: 0.8,
                embedding: low_det,
                crop_jpeg: None,
            },
            PhotoFaceCandidate {
                face_idx: 5,
                bbox: [310.0, 10.0, 48.0, 48.0],
                det_score: 0.91,
                sharpness: 300.0,
                quality: 0.8,
                embedding: clear_owner,
                crop_jpeg: None,
            },
        ];

        let matches = match_photo_faces(&index, &candidates);
        assert_eq!(matches.len(), 2);
        assert_eq!(matches[0].entity_id, "ent-alice");
        assert_eq!(matches[0].face_idx, 0);
        assert!(matches[0].similarity >= PHOTO_MATCH_COSINE_MIN);
        assert!(matches[0].margin >= PHOTO_MATCH_MARGIN_MIN);
        assert_eq!(matches[1].entity_id, "ent-owner");
        assert_eq!(matches[1].face_idx, 5);
        assert!(matches[1].is_principal);
    }
}
