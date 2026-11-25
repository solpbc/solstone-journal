// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Thresholds calibrated on an owner-consented evaluation of real meeting recordings.
use super::{Face, FaceKey, FacesState, Source};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const NEIGHBOUR: f32 = 0.40;
pub const CONSENSUS: f32 = 0.363;
pub const OWNER: f32 = 0.45;
pub const LOOK: f32 = 0.93;
pub const DIVERSITY: f32 = 0.92;
pub const MIN_ROWS: usize = 3;
pub const MIN_SEGMENTS: usize = 2;
pub const MIN_ANCHORED: usize = 2;
pub const MIN_LOOKS: usize = 3;
pub const SUGGESTED_LOOKS: usize = 5;
pub const PANEL_SIZE: usize = 6;
/// Maximum number of slots in a person's profile-pic panel reserved for matched
/// still-photo faces when meeting evidence has already established the panel.
pub const MAX_PHOTO_PANEL_SLOTS: usize = 2;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CrossEntity {
    pub contested: bool,
    pub exclusivity_drop: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Hidden,
    Possible,
    Suggested,
    Confirmed,
    Dismissed,
}
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Panel {
    pub state: State,
    pub faces: Vec<FaceKey>,
    pub default: Option<FaceKey>,
    pub looks: usize,
}

pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(a, b)| a * b).sum()
}
fn segment(face: &Face) -> (&str, &str, &str, &str) {
    let k = &face.metadata.key;
    (&k.day, &k.stream_layout, &k.stream, &k.segment_key)
}

/// Kept evidence, before look collapse, suitable for cross-entity centroids.
pub fn consensus<'a>(
    faces: &[&'a Face],
    anchor: Option<&[f32]>,
    owner: Option<&[f32]>,
) -> Vec<&'a Face> {
    if faces.len() < MIN_ROWS {
        return vec![];
    }
    let medoid = faces
        .iter()
        .enumerate()
        .max_by_key(|(index, f)| {
            (
                faces
                    .iter()
                    .filter(|g| cosine(&f.embedding, &g.embedding) >= NEIGHBOUR)
                    .count(),
                std::cmp::Reverse(*index),
            )
        })
        .map(|(_, f)| f.embedding.as_slice())
        .expect("nonempty");
    let anchor = anchor.unwrap_or(medoid);
    let kept: Vec<_> = faces
        .iter()
        .copied()
        .filter(|f| cosine(anchor, &f.embedding) >= CONSENSUS)
        .filter(|f| owner.is_none_or(|o| cosine(o, &f.embedding) < OWNER))
        .collect();
    if kept.len() < MIN_ROWS
        || kept
            .iter()
            .map(|f| segment(f))
            .collect::<BTreeSet<_>>()
            .len()
            < MIN_SEGMENTS
    {
        vec![]
    } else {
        kept
    }
}

pub fn looks<'a>(faces: &[&'a Face]) -> Vec<&'a Face> {
    let mut out: Vec<&Face> = vec![];
    for face in faces {
        if !out.iter().any(|other| {
            segment(face) != segment(other) && cosine(&face.embedding, &other.embedding) > LOOK
        }) {
            out.push(face);
        }
    }
    out
}

/// The owner uses anchored meeting evidence only, or box meeting evidence if
/// there are no anchors. Still-photo rows (`Source::Photo`) are excluded so
/// meeting centroids never drift from downstream matches.
pub fn owner_centroid(faces: &[Face]) -> Option<Vec<f32>> {
    let mut pool: Vec<_> = faces
        .iter()
        .filter(|f| !f.metadata.rejected && f.metadata.source == Source::Anchored)
        .collect();
    if pool.is_empty() {
        pool = faces
            .iter()
            .filter(|f| !f.metadata.rejected && f.metadata.source != Source::Photo)
            .collect();
    }
    let kept = consensus(&pool, None, None);
    centroid(&kept)
}
pub fn centroid(faces: &[&Face]) -> Option<Vec<f32>> {
    if faces.is_empty() {
        return None;
    }
    let mut v = vec![0.; super::WIDTH];
    for f in faces {
        for (out, x) in v.iter_mut().zip(&f.embedding) {
            *out += x;
        }
    }
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm <= 0. || !norm.is_finite() {
        return None;
    }
    for x in &mut v {
        *x /= norm;
    }
    Some(v)
}

/// Pure. Inputs retain ingestion order: anchored picks first, then box fallback.
///
/// Meeting evidence (`Source::Anchored` and `Source::Box`) solely governs the
/// `MIN_LOOKS` (>=3 -> `Possible`) and `SUGGESTED_LOOKS` (>=5 -> `Suggested`)
/// gates and unconfirmed auto-default selection so photo matches cannot
/// bootstrap a panel or cause circular centroid drift. Once meeting evidence
/// establishes a visible panel (`Possible`, `Suggested`, or `Confirmed`),
/// non-rejected `Source::Photo` matches are admitted as candidate profile-pic
/// tiles alongside meeting tiles so the owner can confirm them.
pub fn panel(
    faces: &[Face],
    state: &FacesState,
    principal: bool,
    owner: Option<&[f32]>,
    cross: Option<CrossEntity>,
) -> Panel {
    let kept = kept_evidence(faces, state, principal, owner);
    let mut distinct = looks(&kept);
    let look_count = distinct.len();
    let cross = cross.unwrap_or_default();
    let photo = state.decision.photo.as_ref();
    let photo_candidates = eligible_photo_candidates(faces, principal, owner);
    finish_panel(
        state,
        cross,
        photo,
        &mut distinct,
        &photo_candidates,
        look_count,
    )
}

/// Non-rejected `Source::Photo` faces eligible to appear as candidate profile
/// pics on an already-established panel, sorted by quality descending.
fn eligible_photo_candidates<'a>(
    faces: &'a [Face],
    principal: bool,
    owner: Option<&[f32]>,
) -> Vec<&'a Face> {
    let rejected: Vec<_> = faces.iter().filter(|f| f.metadata.rejected).collect();
    let mut photos: Vec<_> = faces
        .iter()
        .filter(|f| !f.metadata.rejected && f.metadata.source == Source::Photo)
        .filter(|f| {
            !rejected
                .iter()
                .any(|r| cosine(&r.embedding, &f.embedding) >= LOOK)
        })
        .filter(|f| principal || owner.is_none_or(|o| cosine(o, &f.embedding) < OWNER))
        .collect();
    photos.sort_by(|a, b| b.metadata.quality.total_cmp(&a.metadata.quality));
    photos
}

/// Shared by the panel and the daily cross-person pass, before look collapse.
/// Strictly meeting evidence (`Source::Anchored` and `Source::Box`); still
/// photo rows (`Source::Photo`) never alter meeting consensus or cross rules.
pub fn kept_evidence<'a>(
    faces: &'a [Face],
    state: &FacesState,
    principal: bool,
    owner: Option<&[f32]>,
) -> Vec<&'a Face> {
    let anchor = state
        .decision
        .photo
        .as_ref()
        .map(|p| p.face.embedding.as_slice());
    let rejected: Vec<_> = faces.iter().filter(|f| f.metadata.rejected).collect();
    let mut pool: Vec<_> = faces
        .iter()
        .filter(|f| !f.metadata.rejected && f.metadata.source != Source::Photo)
        .filter(|f| {
            !rejected
                .iter()
                .any(|r| cosine(&r.embedding, &f.embedding) >= LOOK)
        })
        .collect();
    pool.sort_by_key(|f| {
        if f.metadata.source == Source::Anchored {
            0
        } else {
            1
        }
    });
    let kept = consensus(&pool, anchor, if principal { None } else { owner });
    if kept
        .iter()
        .filter(|f| f.metadata.source == Source::Anchored)
        .count()
        < MIN_ANCHORED
    {
        vec![]
    } else {
        kept
    }
}

fn finish_panel(
    state: &FacesState,
    cross: CrossEntity,
    photo: Option<&super::ChosenPhoto>,
    distinct: &mut Vec<&Face>,
    photo_candidates: &[&Face],
    look_count: usize,
) -> Panel {
    let result_state = if photo.is_some() {
        State::Confirmed
    } else if state.decision.dismissed_at.is_some()
        && !distinct.iter().any(|f| {
            !state
                .decision
                .dismissed_looks
                .iter()
                .any(|e| cosine(e, &f.embedding) > LOOK)
        })
    {
        State::Dismissed
    } else if cross.contested || cross.exclusivity_drop || look_count < MIN_LOOKS {
        State::Hidden
    } else if look_count < SUGGESTED_LOOKS {
        State::Possible
    } else {
        State::Suggested
    };
    if matches!(result_state, State::Hidden | State::Dismissed) {
        return Panel {
            state: result_state,
            faces: vec![],
            default: None,
            looks: look_count,
        };
    }
    distinct.sort_by(|a, b| b.metadata.quality.total_cmp(&a.metadata.quality));
    // Pick diverse still-photo candidates first (up to MAX_PHOTO_PANEL_SLOTS) so
    // crisp real photos appear in the owner's possible profile pics alongside
    // meeting crops without pushing unconfirmed auto-default off a meeting face.
    let mut picked_photos: Vec<&Face> = vec![];
    let mut photo_segments = BTreeSet::new();
    for f in photo_candidates {
        if picked_photos.len() >= MAX_PHOTO_PANEL_SLOTS {
            break;
        }
        if photo_segments.insert(segment(f))
            && picked_photos
                .iter()
                .all(|g| cosine(&g.embedding, &f.embedding) < DIVERSITY)
        {
            picked_photos.push(f);
        }
    }
    let meeting_budget = PANEL_SIZE.saturating_sub(picked_photos.len());
    let mut selected: Vec<&Face> = vec![];
    let mut segments = BTreeSet::new();
    for f in distinct.iter() {
        if selected.len() < meeting_budget && segments.insert(segment(f)) {
            selected.push(f);
        }
    }
    for f in distinct.iter() {
        if selected.len() >= meeting_budget {
            break;
        }
        if !selected.iter().any(|g| g.metadata.key == f.metadata.key)
            && selected
                .iter()
                .all(|g| cosine(&g.embedding, &f.embedding) < DIVERSITY)
        {
            selected.push(f);
        }
    }
    // Append picked still-photo faces directly: they were already deduplicated
    // across segments and `DIVERSITY` among photo candidates, and a crisp real
    // camera photo should never be suppressed because it closely matches a
    // meeting webcam frame of the same person.
    for f in picked_photos {
        if selected.len() >= PANEL_SIZE {
            break;
        }
        if !selected.iter().any(|g| g.metadata.key == f.metadata.key) {
            selected.push(f);
        }
    }
    for f in photo_candidates {
        if selected.len() >= PANEL_SIZE {
            break;
        }
        if !selected.iter().any(|g| g.metadata.key == f.metadata.key)
            && selected
                .iter()
                .filter(|g| g.metadata.source == Source::Photo)
                .all(|g| cosine(&g.embedding, &f.embedding) < DIVERSITY)
        {
            selected.push(f);
        }
    }
    for f in distinct.iter() {
        if selected.len() >= PANEL_SIZE {
            break;
        }
        if !selected.iter().any(|g| g.metadata.key == f.metadata.key)
            && selected
                .iter()
                .all(|g| cosine(&g.embedding, &f.embedding) < DIVERSITY)
        {
            selected.push(f);
        }
    }
    let default = photo.map(|p| p.face.metadata.key.clone()).or_else(|| {
        (result_state == State::Suggested)
            .then(|| {
                selected
                    .iter()
                    .find(|f| f.metadata.source != Source::Photo)
                    .map(|f| f.metadata.key.clone())
            })
            .flatten()
    });
    Panel {
        state: result_state,
        faces: selected.iter().map(|f| f.metadata.key.clone()).collect(),
        default,
        looks: look_count,
    }
}
