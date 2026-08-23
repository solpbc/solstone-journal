// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Structural candidate evidence readers shared by attribution and discovery.

use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::fmt;
use std::fs;
use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;
use serde_json::{Value, json};
use solstone_core_entity::{
    EntityResolutionOutcome, EntityStoreError, load_all_journal_entities,
    record_entity_resolution_from_name_evidence,
};
use solstone_core_journal_config::{ConfigLoadError, materialized_defaults, read_journal_config};
use solstone_core_journal_io::{PathError, segment_path};

use crate::admission::{
    admissible_person_pool, admissible_resolution_entities, saved_choice_excluded_by_admission,
};
use solstone_core_speaker_id::calibration::RESOLUTION_FUZZY_THRESHOLD;

const CHANNEL_ORDER: [&str; 4] = ["screen", "meeting_day", "setting", "speakers"];
static LEADING_SETTING_CONTEXT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)^(meeting|call|lunch|coffee|dinner|chat|conversation|zoom|hangout)\s+(with\s+)?",
    )
    .expect("valid leading setting context regex")
});
static TRAILING_SETTING_CONTEXT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\s+(at|in|about|re|regarding|on|over)\s+.*$")
        .expect("valid trailing setting context regex")
});
static SETTING_CONNECTOR: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r",\s*(?:and\s+)?|\s+and\s+|&\s*").expect("valid setting connector regex")
});
static MEETING_PARTICIPANTS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\*\*Participants?\s*[:–—\-]\*\*\s*(.*)|\*\*Participants?\*\*\s*[:–—\-]\s*(.*)")
        .expect("valid meeting participants regex")
});
static MEETING_SEPARATOR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[,;]").expect("valid meeting separator regex"));

/// One source-health gap observed while assembling candidate evidence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvidenceGap {
    pub source: String,
    pub reason: String,
}

/// Provenance channels supporting one resolved candidate entity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CandidateEvidence {
    pub entity_id: String,
    pub sources: Vec<String>,
}

/// Failure outside the Python-compatible per-source gap model.
#[derive(Debug)]
pub enum EvidenceError {
    Path(PathError),
    Entity(EntityStoreError),
    Config(ConfigLoadError),
}

impl fmt::Display for EvidenceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Path(error) => error.fmt(formatter),
            Self::Entity(error) => error.fmt(formatter),
            Self::Config(error) => error.fmt(formatter),
        }
    }
}

impl Error for EvidenceError {}

impl From<PathError> for EvidenceError {
    fn from(error: PathError) -> Self {
        Self::Path(error)
    }
}

impl From<EntityStoreError> for EvidenceError {
    fn from(error: EntityStoreError) -> Self {
        Self::Entity(error)
    }
}

impl From<ConfigLoadError> for EvidenceError {
    fn from(error: ConfigLoadError) -> Self {
        Self::Config(error)
    }
}

/// Recompute per-segment candidate evidence without mutation.
pub fn compute_segment_candidate_evidence_readonly(
    journal_root: &Path,
    day: &str,
    stream: &str,
    segment_key: &str,
) -> Result<(Vec<CandidateEvidence>, Vec<EvidenceGap>), EvidenceError> {
    let segment_dir = segment_path(journal_root, day, segment_key, stream, false)?;
    if !segment_dir.is_dir() {
        return Ok((Vec::new(), Vec::new()));
    }

    let mut gaps = Vec::new();
    let (speakers, source_gaps) = load_segment_speakers_with_gaps(&segment_dir);
    gaps.extend(source_gaps);
    let (setting, source_gaps) = load_setting_field_with_gaps(&segment_dir);
    gaps.extend(source_gaps);
    let setting_names = match setting {
        Some(setting) => parse_setting_names(journal_root, &setting)?,
        None => Vec::new(),
    };
    let (screen_names, source_gaps) = extract_screen_participants_with_gaps(&segment_dir);
    gaps.extend(source_gaps);
    let (meeting_names, source_gaps) = extract_meeting_participants_with_gaps(journal_root, day);
    gaps.extend(source_gaps);

    let name_channels =
        candidate_name_channels(&speakers, &setting_names, &screen_names, &meeting_names);
    let candidate_names = ordered_dedup(
        speakers
            .iter()
            .chain(&setting_names)
            .chain(&screen_names)
            .chain(&meeting_names),
    );
    let entities = load_all_journal_entities(journal_root)?;
    let all_entities = entities.iter().collect::<Vec<_>>();
    let unblocked = entities
        .iter()
        .filter(|entity| !entity.is_blocked())
        .collect::<Vec<_>>();
    let pool = admissible_person_pool(&unblocked);
    let resolution_entities = admissible_resolution_entities(&pool);
    let scope = json!({"kind": "journal"});
    let mut name_entity_ids = HashMap::new();
    for name in candidate_names {
        match saved_choice_excluded_by_admission(journal_root, &scope, &name, &all_entities) {
            Ok(true) => continue,
            Ok(false) => {}
            Err(_) => {
                gaps.push(gap("resolution", "stale_resolution"));
                continue;
            }
        }
        match record_entity_resolution_from_name_evidence(
            journal_root,
            &name,
            &resolution_entities,
            scope.clone(),
            json!({
                "lane": "apps.speakers.aggregation",
                "day": day,
                "segment_id": segment_key,
                "field": "candidate_name",
            }),
            RESOLUTION_FUZZY_THRESHOLD,
            true,
        ) {
            Ok(resolution)
                if resolution.outcome == EntityResolutionOutcome::Resolved
                    && let Some(index) = resolution.entity_index =>
            {
                if let Some(entity) = pool.get(index) {
                    name_entity_ids.insert(name, entity.id.clone());
                }
            }
            Ok(_) => {}
            Err(_) => gaps.push(gap("resolution", "stale_resolution")),
        }
    }
    Ok((
        assemble_candidate_evidence(&name_channels, &name_entity_ids),
        gaps,
    ))
}

/// Read the imported-audio setting field and its source-health gaps.
pub fn load_setting_field_with_gaps(segment_dir: &Path) -> (Option<String>, Vec<EvidenceGap>) {
    let path = segment_dir.join("imported_audio.jsonl");
    if !path.exists() {
        return (None, Vec::new());
    }
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(_) => return (None, vec![gap("setting", "unreadable")]),
    };
    let text = match String::from_utf8(bytes) {
        Ok(text) => text,
        Err(_) => return (None, vec![gap("setting", "malformed_json")]),
    };
    let first_line = text.lines().next().unwrap_or_default().trim();
    if first_line.is_empty() {
        return (None, Vec::new());
    }
    let value: Value = match serde_json::from_str(first_line) {
        Ok(value) => value,
        Err(_) => return (None, vec![gap("setting", "malformed_json")]),
    };
    let Some(object) = value.as_object() else {
        return (None, vec![gap("setting", "wrong_shape")]);
    };
    match object.get("setting") {
        None | Some(Value::Null) => (None, Vec::new()),
        Some(Value::String(setting)) => (Some(setting.clone()), Vec::new()),
        Some(_) => (None, vec![gap("setting", "wrong_shape")]),
    }
}

/// Read the segment speaker-name output and its source-health gaps.
pub fn load_segment_speakers_with_gaps(segment_dir: &Path) -> (Vec<String>, Vec<EvidenceGap>) {
    let path = segment_dir.join("talents/speakers.json");
    if !path.exists() {
        return (Vec::new(), Vec::new());
    }
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(_) => return (Vec::new(), vec![gap("speakers", "unreadable")]),
    };
    let text = match String::from_utf8(bytes) {
        Ok(text) => text,
        Err(_) => return (Vec::new(), vec![gap("speakers", "malformed_json")]),
    };
    let value: Value = match serde_json::from_str(&text) {
        Ok(value) => value,
        Err(_) => return (Vec::new(), vec![gap("speakers", "malformed_json")]),
    };
    let Some(values) = value.as_array() else {
        return (Vec::new(), vec![gap("speakers", "wrong_shape")]);
    };
    let names = values
        .iter()
        .filter_map(Value::as_str)
        .filter(|name| !name.trim().is_empty())
        .map(ToOwned::to_owned)
        .collect();
    let gaps = if values.iter().any(|value| !value.is_string()) {
        vec![gap("speakers", "wrong_shape")]
    } else {
        Vec::new()
    };
    (names, gaps)
}

/// Read Person attendee names from screen output and its source-health gaps.
pub fn extract_screen_participants_with_gaps(
    segment_dir: &Path,
) -> (Vec<String>, Vec<EvidenceGap>) {
    let path = segment_dir.join("talents/screen.json");
    if !path.exists() {
        return (Vec::new(), Vec::new());
    }
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(_) => return (Vec::new(), vec![gap("screen", "malformed_json")]),
    };
    let text = match String::from_utf8(bytes) {
        Ok(text) => text,
        Err(_) => return (Vec::new(), vec![gap("screen", "malformed_json")]),
    };
    let value: Value = match serde_json::from_str(&text) {
        Ok(value) => value,
        Err(_) => return (Vec::new(), vec![gap("screen", "malformed_json")]),
    };
    let Some(entities) = value.get("entities").and_then(Value::as_array) else {
        return (Vec::new(), vec![gap("screen", "wrong_shape")]);
    };
    let names = entities
        .iter()
        .filter_map(Value::as_object)
        .filter(|entity| entity.get("type").and_then(Value::as_str) == Some("Person"))
        .filter(|entity| entity.get("role").and_then(Value::as_str) == Some("attendee"))
        .filter_map(|entity| entity.get("name").and_then(Value::as_str))
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(ToOwned::to_owned)
        .collect();
    (names, Vec::new())
}

/// Read daily meeting participant names and its source-health gaps.
pub fn extract_meeting_participants_with_gaps(
    journal_root: &Path,
    day: &str,
) -> (Vec<String>, Vec<EvidenceGap>) {
    let path = journal_root
        .join("chronicle")
        .join(day)
        .join("talents/meetings.md");
    if !path.exists() {
        return (Vec::new(), Vec::new());
    }
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(_) => return (Vec::new(), vec![gap("meeting_day", "unreadable")]),
    };
    let content = match String::from_utf8(bytes) {
        Ok(content) => content,
        Err(_) => return (Vec::new(), vec![gap("meeting_day", "unreadable")]),
    };
    let mut names = Vec::new();
    for line in content.lines() {
        let Some(captures) = MEETING_PARTICIPANTS.captures(line) else {
            continue;
        };
        let participants = captures
            .get(1)
            .or_else(|| captures.get(2))
            .map_or("", |capture| capture.as_str());
        for name in MEETING_SEPARATOR.split(participants) {
            let name = name.trim().trim_matches('*').trim();
            if name.chars().count() > 1 {
                names.push(name.to_owned());
            }
        }
    }
    (names, Vec::new())
}

/// Parse non-owner participant names from an imported setting field.
pub fn parse_setting_names(
    journal_root: &Path,
    setting: &str,
) -> Result<Vec<String>, EvidenceError> {
    if setting.is_empty() {
        return Ok(Vec::new());
    }
    let text = LEADING_SETTING_CONTEXT.replace(setting, "");
    let text = TRAILING_SETTING_CONTEXT.replace(&text, "");
    let owner_names = derive_owner_name_variants(identity_names(journal_root)?);
    Ok(SETTING_CONNECTOR
        .split(&text)
        .map(str::trim)
        .filter(|name| name.chars().count() > 1)
        .filter(|name| !owner_names.contains(&name.to_lowercase()))
        .map(ToOwned::to_owned)
        .collect())
}

/// Build name-to-source provenance using the canonical channel order.
pub fn candidate_name_channels(
    speakers: &[String],
    setting_names: &[String],
    screen_names: &[String],
    meeting_names: &[String],
) -> HashMap<String, HashSet<String>> {
    let mut channels = HashMap::new();
    for (channel, names) in [
        ("screen", screen_names),
        ("meeting_day", meeting_names),
        ("setting", setting_names),
        ("speakers", speakers),
    ] {
        for name in names {
            channels
                .entry(name.clone())
                .or_insert_with(HashSet::new)
                .insert(channel.to_owned());
        }
    }
    channels
}

/// Assemble deterministic entity evidence from resolved candidate names.
pub fn assemble_candidate_evidence(
    name_channels: &HashMap<String, HashSet<String>>,
    name_entity_ids: &HashMap<String, String>,
) -> Vec<CandidateEvidence> {
    let mut entity_sources: HashMap<String, HashSet<String>> = HashMap::new();
    for (name, entity_id) in name_entity_ids {
        if let Some(sources) = name_channels.get(name) {
            entity_sources
                .entry(entity_id.clone())
                .or_default()
                .extend(sources.iter().cloned());
        }
    }
    let mut evidence = entity_sources
        .into_iter()
        .filter(|(_, sources)| !sources.is_empty())
        .map(|(entity_id, sources)| {
            let mut sources = sources.into_iter().collect::<Vec<_>>();
            sources.sort_by_key(|source| channel_rank(source));
            CandidateEvidence { entity_id, sources }
        })
        .collect::<Vec<_>>();
    evidence.sort_by(|left, right| left.entity_id.cmp(&right.entity_id));
    evidence
}

fn identity_names(journal_root: &Path) -> Result<Vec<String>, EvidenceError> {
    let config = read_journal_config(journal_root)?
        .config
        .unwrap_or_else(materialized_defaults);
    let Some(identity) = config.get("identity").and_then(Value::as_object) else {
        return Ok(Vec::new());
    };
    let mut names = Vec::new();
    push_identity_name(&mut names, identity.get("preferred"));
    push_identity_name(&mut names, identity.get("name"));
    if let Some(aliases) = identity.get("aliases").and_then(Value::as_array) {
        for alias in aliases {
            push_identity_name(&mut names, Some(alias));
        }
    }
    Ok(names)
}

fn push_identity_name(names: &mut Vec<String>, value: Option<&Value>) {
    let Some(name) = value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|name| !name.is_empty())
    else {
        return;
    };
    if !names.iter().any(|existing| existing == name) {
        names.push(name.to_owned());
    }
}

fn derive_owner_name_variants(names: Vec<String>) -> HashSet<String> {
    let mut variants = HashSet::new();
    for name in names {
        let name = name.trim().to_lowercase();
        if name.is_empty() {
            continue;
        }
        variants.insert(name.clone());
        variants.extend(name.split_whitespace().map(ToOwned::to_owned));
    }
    variants
}

pub(crate) fn ordered_dedup<'a>(names: impl Iterator<Item = &'a String>) -> Vec<String> {
    let mut seen = HashSet::new();
    names
        .filter(|name| seen.insert((*name).clone()))
        .cloned()
        .collect()
}

fn gap(source: &str, reason: &str) -> EvidenceGap {
    EvidenceGap {
        source: source.to_owned(),
        reason: reason.to_owned(),
    }
}

fn channel_rank(source: &str) -> usize {
    CHANNEL_ORDER
        .iter()
        .position(|channel| *channel == source)
        .unwrap_or(CHANNEL_ORDER.len())
}
