// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Unified speaker filtering for spoken transcript reads, listings, and search.
//!
//! Accepts:
//! - `"owner"` (case-insensitive): matches lines attributed to the journal owner
//!   by the voice gate or owner assignment (`Transcripts` permission only; never
//!   requires `Entities`).
//! - An entity reference (sealed token or `entity_id@facet`): requires `Entities`
//!   permission and facet scope parity, matching the principal as owner or other
//!   entities by voice-attributed entity id / display name.
//! - A display name or label string (e.g., `"Dorcas Ansell"`, `"(the templar)"`,
//!   `"Speaker 1"`, `"Someone else"`): matches exact folded speaker display labels
//!   or contiguous alphanumeric word subsequences, while keeping unnamed/numbered
//!   speakers distinct (e.g., `"Speaker 1"` never matches `"Speaker 10"`).

use std::collections::BTreeSet;
use std::path::Path;

use solstone_core_indexer_query::AdmittedCategory;
use solstone_core_transcripts::{AgentSpeakers, ResolvedSpeechLine};

use crate::dispatch::{
    DispatchError, DispatchPrincipal, available_facets, codec, entity_name, read_scoped_entities,
};
use crate::permissions::ConnectionReadSnapshot;
use crate::references::{ReferenceKind, ReferenceTarget, is_framed_entity_token};
use crate::tools::ToolError;

#[derive(Clone, Debug)]
pub(crate) enum SpeakerFilter {
    Owner,
    Entity {
        entity_id: String,
        is_principal: bool,
        folded_name: String,
    },
    Literal {
        raw_folded: String,
        words: Vec<String>,
        matches_owner_name: bool,
    },
}

impl SpeakerFilter {
    pub(crate) fn matches(&self, line: &ResolvedSpeechLine) -> bool {
        match self {
            Self::Owner => line.is_owner,
            Self::Entity {
                entity_id,
                is_principal,
                folded_name,
            } => {
                if *is_principal && line.is_owner {
                    return true;
                }
                if line.speaker_entity_id.as_deref() == Some(entity_id.as_str()) {
                    return true;
                }
                let Some(speaker) = line.speaker.as_deref() else {
                    return false;
                };
                let folded = fold_literal(speaker);
                if folded == *folded_name {
                    return true;
                }
                let bare = speaker.strip_suffix(" (owner)").map(fold_literal);
                bare.as_deref() == Some(folded_name.as_str())
            }
            Self::Literal {
                raw_folded,
                words,
                matches_owner_name,
            } => {
                if *matches_owner_name && line.is_owner {
                    return true;
                }
                let Some(speaker) = line.speaker.as_deref() else {
                    return false;
                };
                let speaker_folded = fold_literal(speaker);
                if speaker_folded == *raw_folded {
                    return true;
                }
                if words.is_empty() {
                    return false;
                }
                let speaker_words = split_alnum_words(&speaker_folded);
                contains_word_subsequence(&speaker_words, words)
            }
        }
    }
}

pub(crate) fn resolve_speaker_filter(
    journal_root: &Path,
    principal: DispatchPrincipal<'_>,
    snapshot: &ConnectionReadSnapshot,
    scoped_facet: Option<&str>,
    speakers: Option<&AgentSpeakers>,
    raw: &str,
) -> Result<SpeakerFilter, DispatchError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(DispatchError::InvalidInput);
    }
    if trimmed.eq_ignore_ascii_case("owner") {
        return Ok(SpeakerFilter::Owner);
    }
    if is_framed_entity_token(trimmed) {
        if !snapshot.categories.contains(&AdmittedCategory::Entities) {
            return Err(DispatchError::Tool(ToolError::ReferenceNotFound));
        }
        let ReferenceTarget::Entity(entity_ref) = codec()
            .resolve(
                trimmed,
                ReferenceKind::Entity,
                principal.connection,
                snapshot.generation,
            )
            .map_err(|_| DispatchError::Tool(ToolError::ReferenceNotFound))?
        else {
            return Err(DispatchError::Tool(ToolError::ReferenceNotFound));
        };
        if let Some(facet) = scoped_facet
            && entity_ref.facet_id != facet
        {
            return Err(DispatchError::Tool(ToolError::ReferenceNotFound));
        }
        let facets = available_facets(journal_root, snapshot)?;
        let Some((_, facet_dir, _)) = facets.iter().find(|(id, _, _)| id == &entity_ref.facet_id)
        else {
            return Err(DispatchError::Tool(ToolError::ReferenceNotFound));
        };
        let item = read_scoped_entities(journal_root, facet_dir)?
            .into_iter()
            .find(|item| item.entity_id == entity_ref.entity_id)
            .ok_or(DispatchError::Tool(ToolError::ReferenceNotFound))?;
        let is_principal = speakers.is_some_and(|s| s.voices.principal == entity_ref.entity_id);
        return Ok(SpeakerFilter::Entity {
            entity_id: entity_ref.entity_id,
            is_principal,
            folded_name: fold_literal(&entity_name(&item.identity)),
        });
    }
    if let Some((entity_id, facet_token)) = crate::names::parse_entity(trimmed) {
        let facets = available_facets(journal_root, snapshot)?;
        let matched_facet = facets
            .iter()
            .find(|(id, _, _)| id == &facet_token)
            .or_else(|| facets.iter().find(|(_, dir, _)| dir == &facet_token));
        if let Some((facet_id, facet_dir, _)) = matched_facet {
            if let Some(scoped) = scoped_facet
                && facet_id != scoped
            {
                return Err(DispatchError::Tool(ToolError::ReferenceNotFound));
            }
            if !snapshot.categories.contains(&AdmittedCategory::Entities) {
                return Err(DispatchError::Tool(ToolError::ReferenceNotFound));
            }
            let item = read_scoped_entities(journal_root, facet_dir)?
                .into_iter()
                .find(|item| item.entity_id == entity_id)
                .ok_or(DispatchError::Tool(ToolError::ReferenceNotFound))?;
            let is_principal = speakers.is_some_and(|s| s.voices.principal == entity_id);
            return Ok(SpeakerFilter::Entity {
                entity_id,
                is_principal,
                folded_name: fold_literal(&entity_name(&item.identity)),
            });
        }
    }
    let raw_folded = fold_literal(trimmed);
    let words = split_alnum_words(&raw_folded);
    let matches_owner_name = speakers.is_some_and(|s| {
        let owner_folded = fold_literal(&s.owner);
        if raw_folded == owner_folded {
            return true;
        }
        s.owner
            .strip_suffix(" (owner)")
            .map(fold_literal)
            .is_some_and(|bare| !bare.is_empty() && raw_folded == bare)
    });
    Ok(SpeakerFilter::Literal {
        raw_folded,
        words,
        matches_owner_name,
    })
}

/// Indices into `lines` matching `filter`, expanded by `context_lines` before
/// and after each matching line, deduplicated in chronological order.
pub(crate) fn select_spoken_line_indices(
    lines: &[ResolvedSpeechLine],
    filter: &SpeakerFilter,
    context_lines: usize,
) -> Vec<usize> {
    if lines.is_empty() {
        return Vec::new();
    }
    let mut selected = BTreeSet::new();
    for (idx, line) in lines.iter().enumerate() {
        if filter.matches(line) {
            let start = idx.saturating_sub(context_lines);
            let end = idx.saturating_add(context_lines).min(lines.len() - 1);
            for keep in start..=end {
                selected.insert(keep);
            }
        }
    }
    selected.into_iter().collect()
}

fn fold_literal(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

fn split_alnum_words(folded: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut current = String::new();
    for ch in folded.chars() {
        if ch.is_alphanumeric() {
            current.push(ch);
        } else if !current.is_empty() {
            words.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        words.push(current);
    }
    words
}

fn contains_word_subsequence(haystack: &[String], needle: &[String]) -> bool {
    if needle.is_empty() || needle.len() > haystack.len() {
        return false;
    }
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use super::{SpeakerFilter, fold_literal, select_spoken_line_indices, split_alnum_words};
    use solstone_core_transcripts::ResolvedSpeechLine;

    fn line(speaker: Option<&str>, is_owner: bool, entity_id: Option<&str>) -> ResolvedSpeechLine {
        ResolvedSpeechLine {
            text: format!("[10:00:00] {}: hi", speaker.unwrap_or("")),
            time: Some("10:00:00".to_owned()),
            speaker: speaker.map(str::to_owned),
            is_owner,
            speaker_entity_id: entity_id.map(str::to_owned),
            untimed: false,
        }
    }

    fn literal(raw: &str) -> SpeakerFilter {
        let raw_folded = fold_literal(raw);
        let words = split_alnum_words(&raw_folded);
        SpeakerFilter::Literal {
            raw_folded,
            words,
            matches_owner_name: false,
        }
    }

    #[test]
    fn literal_matching_respects_word_boundaries_and_unnamed_labels() {
        assert!(literal("Speaker 1").matches(&line(Some("Speaker 1"), false, None)));
        assert!(!literal("Speaker 1").matches(&line(Some("Speaker 10"), false, None)));
        assert!(literal("Dorcas Ansell").matches(&line(
            Some("Mistress Dorcas Ansell"),
            false,
            Some("dorcas")
        )));
        assert!(!literal("Ann").matches(&line(Some("Anne"), false, None)));
        assert!(literal("(the templar)").matches(&line(Some("(the templar)"), false, None)));
        assert!(literal("the templar").matches(&line(Some("(the templar)"), false, None)));
        assert!(literal("Someone else").matches(&line(Some("Someone else"), false, None)));
        assert!(!literal("(").matches(&line(Some("Speaker 1"), false, None)));
    }

    #[test]
    fn context_window_deduplicates_overlaps_and_stays_empty_without_match() {
        let lines = vec![
            line(Some("Other"), false, None),
            line(Some("Alex (owner)"), true, Some("owner")),
            line(Some("Other"), false, None),
            line(Some("Alex (owner)"), true, Some("owner")),
            line(Some("Other"), false, None),
            line(Some("Third"), false, None),
        ];
        assert_eq!(
            select_spoken_line_indices(&lines, &SpeakerFilter::Owner, 0),
            vec![1, 3]
        );
        assert_eq!(
            select_spoken_line_indices(&lines, &SpeakerFilter::Owner, 1),
            vec![0, 1, 2, 3, 4]
        );
        assert!(select_spoken_line_indices(&lines, &literal("Missing"), 2).is_empty());
    }
}
