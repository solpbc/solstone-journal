// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The sentences a confirmed pool voice names.
//!
//! These are the sentences the transcript view shows as that voice (the same
//! lowest-voice-id claim rule as `voice_tags`, applied to a locked, strict read
//! of the pool) that nobody has named yet, on the one audio source a segment's
//! speaker labels describe, not close to the owner's own voice, and in a
//! recording that sounds like the voice ([`NAME_EVERYWHERE_MIN_SIMILARITY`]).
//! Naming the voice writes a label for each of them.

use std::collections::{BTreeSet, HashSet};
use std::fs;
use std::path::Path;

use serde_json::{Value, json};
use solstone_core_entity::{is_admissible_person, load_all_journal_entities, normalize_embedding};
use solstone_core_journal_io::{DEFAULT_STREAM, SegmentLayout};
use thiserror::Error;

use crate::candidate_tracker::{CandidateTracker, CandidateTrackerError};
use crate::direct_voiceprints::{
    DirectVoiceprintsError, current_owner_centroid, load_member_embedding, recording_similarity,
};
use crate::identify_forward_phases::load_labels;
use crate::identify_operations::MemberProvenance;
use crate::voice_tags::build_index;

/// The least a recording's voice must match the pool voice's centroid for
/// "everywhere this voice appears" to write the name into it. A
/// blinded listening test (2026-10-05, 88 pairs) judged pool recordings at or
/// above .80 the same person 11 of 12 times, and cross-day ones between .72
/// and .80 only 2 of 4. Below it, a recording keeps its voice tag unnamed.
pub const NAME_EVERYWHERE_MIN_SIMILARITY: f32 = 0.80;

/// Failure computing a voice's members.
#[derive(Debug, Error)]
pub enum VoiceMembersError {
    #[error("speaker pool could not be read: {0}")]
    Pool(#[from] CandidateTrackerError),
    #[error("entity store failed: {0}")]
    Entity(#[from] solstone_core_entity::EntityStoreError),
    #[error("segment lookup failed: {0}")]
    Segment(#[from] crate::segment_catalog::ExactLookupError),
    #[error("voice sample lookup failed: {0}")]
    Embedding(#[from] DirectVoiceprintsError),
}

/// The sentence the owner tapped, so a stale page cannot name another voice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VoiceAnchor {
    pub day: String,
    pub stream: String,
    pub segment_key: String,
    pub source: String,
    pub sentence_id: i64,
}

/// What the pool holds for one voice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VoiceLookup {
    /// No such voice, or the pool rejected it.
    Missing,
    /// The tapped sentence no longer belongs to this voice.
    Changed,
    /// Without the owner's voice nothing can be screened against it.
    OwnerVoiceUnavailable,
    Found {
        confirmed_entity: Option<String>,
        members: Vec<MemberProvenance>,
    },
}

fn stream_layout(source: &Value, stream: &str) -> Option<SegmentLayout> {
    match source.get("stream_layout").and_then(Value::as_str) {
        Some("direct") => Some(SegmentLayout::Direct),
        Some("named") => Some(SegmentLayout::Named),
        Some(_) => None,
        None if stream == DEFAULT_STREAM => Some(SegmentLayout::Direct),
        None => Some(SegmentLayout::Named),
    }
}

/// The audio source a segment's speaker labels describe: the first sorted
/// `audio.npz` / `*_audio.npz`, the same choice attribution makes.
pub(crate) fn labels_source(segment: &Path) -> Option<String> {
    let mut stems = fs::read_dir(segment)
        .ok()?
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            let stem = name.strip_suffix(".npz")?;
            (stem == "audio" || stem.ends_with("_audio")).then(|| stem.to_owned())
        })
        .collect::<Vec<_>>();
    stems.sort();
    stems.into_iter().next()
}

/// The unnamed sentences of one pool voice, ready to be named.
pub fn voice_members(
    journal: &Path,
    voice_id: i64,
    anchor: Option<&VoiceAnchor>,
) -> Result<VoiceLookup, VoiceMembersError> {
    let candidates = CandidateTracker::new(journal).snapshot_candidates_locked()?;
    let Some(voice) = candidates
        .iter()
        .find(|candidate| candidate.cand_id == voice_id)
    else {
        return Ok(VoiceLookup::Missing);
    };
    if voice.status == "rejected" {
        return Ok(VoiceLookup::Missing);
    }
    let index = build_index(
        &serde_json::to_vec(&json!({
            "candidates": candidates.iter().map(|candidate| candidate.to_json()).collect::<Vec<_>>()
        }))
        .expect("pool snapshot serializes"),
    );
    let claimed_by =
        |day: &str, stream: &str, segment_key: &str, source: &str, sentence_id: i64| {
            index
                .get(&(
                    day.to_owned(),
                    stream.to_owned(),
                    segment_key.to_owned(),
                    source.to_owned(),
                ))
                .and_then(|sentences| sentences.get(&sentence_id))
                .map(|tag| tag.voice_id)
        };
    if let Some(anchor) = anchor
        && claimed_by(
            &anchor.day,
            &anchor.stream,
            &anchor.segment_key,
            &anchor.source,
            anchor.sentence_id,
        ) != Some(voice_id)
    {
        return Ok(VoiceLookup::Changed);
    }
    let owner = match current_owner_centroid(journal) {
        Ok(Some(owner)) => owner,
        Ok(None) | Err(DirectVoiceprintsError::OwnerIdentityInvalid) => {
            return Ok(VoiceLookup::OwnerVoiceUnavailable);
        }
        Err(error) => return Err(error.into()),
    };

    let Some(centroid) = normalize_embedding(&voice.centroid) else {
        return Ok(VoiceLookup::Missing);
    };

    let entities = load_all_journal_entities(journal)?;
    let admissible = entities
        .iter()
        .filter(|entity| is_admissible_person(entity))
        .map(|entity| entity.id.clone())
        .collect::<HashSet<_>>();

    let mut members = BTreeSet::new();
    let mut seen = HashSet::new();
    for source in &voice.source_segments {
        let (Some(day), Some(stream), Some(segment_key), Some(source_name)) = (
            source.get("day").and_then(Value::as_str),
            source.get("stream").and_then(Value::as_str),
            source.get("segment_key").and_then(Value::as_str),
            source.get("source").and_then(Value::as_str),
        ) else {
            continue;
        };
        let Some(sentence_ids) = source.get("sentence_ids").and_then(Value::as_array) else {
            continue;
        };
        let Some(layout) = stream_layout(source, stream) else {
            continue;
        };
        let Some(segment) =
            crate::segment_catalog::resolve_exact(journal, day, stream, segment_key, layout)?
        else {
            continue;
        };
        if labels_source(&segment).as_deref() != Some(source_name) {
            continue;
        }
        let ids = sentence_ids
            .iter()
            .filter_map(Value::as_i64)
            .collect::<BTreeSet<_>>();
        // The tapped recording is the owner's own call; every other one must
        // sound like the voice.
        let tapped = anchor.is_some_and(|anchor| {
            anchor.day == day
                && anchor.stream == stream
                && anchor.segment_key == segment_key
                && anchor.source == source_name
                && ids.contains(&anchor.sentence_id)
        });
        if !tapped
            && recording_similarity(&segment, source_name, &ids, &centroid)
                .is_none_or(|similarity| similarity < NAME_EVERYWHERE_MIN_SIMILARITY)
        {
            continue;
        }
        let labels = load_labels(&segment);
        for sentence_id in sentence_ids.iter().filter_map(Value::as_i64) {
            if !seen.insert((day, stream, segment_key, source_name, sentence_id)) {
                continue;
            }
            if claimed_by(day, stream, segment_key, source_name, sentence_id) != Some(voice_id) {
                continue;
            }
            let named = labels
                .get(&sentence_id)
                .and_then(|label| label.get("speaker"))
                .and_then(Value::as_str)
                .is_some_and(|speaker| admissible.contains(speaker));
            if named {
                continue;
            }
            let member = MemberProvenance {
                day: day.to_owned(),
                stream_layout: layout,
                stream: stream.to_owned(),
                segment_key: segment_key.to_owned(),
                source: source_name.to_owned(),
                sentence_id,
            };
            if load_member_embedding(journal, &member, Some(&owner))?.is_none() {
                continue;
            }
            members.insert(member);
        }
    }
    Ok(VoiceLookup::Found {
        confirmed_entity: voice.confirmed_entity.clone(),
        members: members.into_iter().collect(),
    })
}
