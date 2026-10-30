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
use serde_json::Value;
use solstone_core_entity::EntityStoreError;
use solstone_core_journal_config::{ConfigLoadError, materialized_defaults, read_journal_config};
use solstone_core_journal_io::PathError;

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

#[derive(Debug)]
pub enum EvidenceError {
    Path(PathError),
    Entity(EntityStoreError),
    Config(ConfigLoadError),
    ExactLookup(crate::segment_catalog::ExactLookupError),
}

impl fmt::Display for EvidenceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Path(error) => error.fmt(formatter),
            Self::Entity(error) => error.fmt(formatter),
            Self::Config(error) => error.fmt(formatter),
            Self::ExactLookup(error) => error.fmt(formatter),
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

impl From<crate::segment_catalog::ExactLookupError> for EvidenceError {
    fn from(error: crate::segment_catalog::ExactLookupError) -> Self {
        Self::ExactLookup(error)
    }
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

/// Read the known people invited to calendar events scheduled during a
/// segment, as the journal names them now, and its source-health gaps. An
/// event sits beside the segment by the rule an activity uses
/// (`ScheduledEvent::scheduled_during`); the people are the ones the calendar
/// listing already matched to known entities, the owner left out.
pub fn extract_meeting_participants_with_gaps(
    journal_root: &Path,
    day: &str,
    segment_key: &str,
) -> (Vec<String>, Vec<EvidenceGap>) {
    let Some(span) = segment_seconds(day, segment_key) else {
        return (Vec::new(), Vec::new());
    };
    let events =
        match solstone_core_indexer_store::calendar_schedule::scheduled_on_day(journal_root, day) {
            Ok(events) => events,
            Err(_) => return (Vec::new(), vec![gap("meeting_day", "unreadable")]),
        };
    let mut names = Vec::new();
    for event in events
        .iter()
        .filter(|event| event.scheduled_during(day, span))
    {
        for person in &event.people {
            if !names.contains(&person.name) {
                names.push(person.name.clone());
            }
        }
    }
    (names, Vec::new())
}

/// A segment's span in seconds from its day's midnight.
fn segment_seconds(day: &str, segment_key: &str) -> Option<(u32, u32)> {
    let (start, end) = solstone_core_format::calendar::segment_span(day, segment_key)?;
    let midnight = start.date().and_hms_opt(0, 0, 0)?;
    let seconds = |at: chrono::NaiveDateTime| u32::try_from((at - midnight).num_seconds()).ok();
    Some((seconds(start)?, seconds(end)?))
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

#[cfg(test)]
#[allow(clippy::disallowed_methods, clippy::disallowed_types)]
mod tests {
    use std::fs;
    use std::path::Path;

    use serde_json::json;
    use sha2::{Digest, Sha256};

    use super::extract_meeting_participants_with_gaps;

    /// A meeting on 2026-10-27, 10:00–11:00 Denver, organized by Alex Rivera
    /// with the owner invited.
    const MEETING: &str = include_str!("../../../fixtures/calendar_notes/golden.jsonl");
    const CALENDAR: &str = "phone_calendar";

    fn person(journal: &Path, id: &str, name: &str, emails: &[&str], principal: bool) {
        let dir = journal.join("entities").join(id);
        fs::create_dir_all(&dir).unwrap();
        let mut identity = json!({"id": id, "name": name, "type": "Person", "emails": emails});
        if principal {
            identity["is_principal"] = json!(true);
        }
        fs::write(dir.join("entity.json"), identity.to_string()).unwrap();
    }

    /// A journal whose calendar holds the meeting, indexed.
    fn journal_with_meeting() -> tempfile::TempDir {
        let journal = tempfile::TempDir::new().unwrap();
        let root = journal.path();
        fs::create_dir_all(root.join("config")).unwrap();
        fs::write(
            root.join("config/journal.json"),
            json!({"identity": {"timezone": "America/Denver", "email_addresses": ["owner@example.com"]}})
                .to_string(),
        )
        .unwrap();
        fs::create_dir_all(root.join("streams")).unwrap();
        fs::write(
            root.join("streams").join(format!("{CALENDAR}.json")),
            json!({
                "name": CALENDAR, "kind": "observer", "host": null, "platform": null,
                "created_at": 0, "last_day": null, "last_segment": null, "seq": 1,
                "cid": "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                "source": "calendar",
            })
            .to_string(),
        )
        .unwrap();
        person(root, "owner", "Jordan Lee", &["owner@example.com"], true);
        person(
            root,
            "alex_rivera",
            "Alex Rivera",
            &["alex@example.com"],
            false,
        );
        let dir = root
            .join("chronicle/20261019")
            .join(CALENDAR)
            .join("094200_300");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("stream.json"),
            json!({"stream": CALENDAR, "prev_day": null, "prev_segment": null, "seq": 1})
                .to_string(),
        )
        .unwrap();
        let bytes = format!("{}\n", MEETING.lines().next().unwrap()).into_bytes();
        let name = format!("calendar_{:x}.jsonl", Sha256::digest(&bytes));
        fs::write(dir.join(&name), bytes).unwrap();
        solstone_core_indexer_store::scan::rescan_file(
            root,
            Path::new(&format!("20261019/{CALENDAR}/094200_300/{name}")),
        )
        .unwrap();
        journal
    }

    #[test]
    fn a_segment_during_a_meeting_names_its_known_invitees() {
        let journal = journal_with_meeting();
        assert_eq!(
            extract_meeting_participants_with_gaps(journal.path(), "20261027", "101500_300"),
            (vec!["Alex Rivera".to_owned()], vec![])
        );
    }

    #[test]
    fn a_segment_outside_every_meeting_names_no_one() {
        let journal = journal_with_meeting();
        assert_eq!(
            extract_meeting_participants_with_gaps(journal.path(), "20261027", "120000_300"),
            (Vec::new(), vec![])
        );
        assert_eq!(
            extract_meeting_participants_with_gaps(journal.path(), "20261028", "101500_300"),
            (Vec::new(), vec![])
        );
    }

    #[test]
    fn an_unreadable_search_index_is_a_meeting_gap() {
        let journal = tempfile::TempDir::new().unwrap();
        let index = solstone_core_indexer_store::db::db_path(journal.path());
        fs::create_dir_all(index.parent().unwrap()).unwrap();
        fs::write(&index, b"not a database").unwrap();
        let (names, gaps) =
            extract_meeting_participants_with_gaps(journal.path(), "20261027", "101500_300");
        assert!(names.is_empty());
        assert_eq!(gaps.len(), 1);
        assert_eq!(
            (gaps[0].source.as_str(), gaps[0].reason.as_str()),
            ("meeting_day", "unreadable")
        );
    }

    #[test]
    fn a_journal_without_a_search_index_has_no_meetings_and_no_gap() {
        let journal = tempfile::TempDir::new().unwrap();
        assert_eq!(
            extract_meeting_participants_with_gaps(journal.path(), "20261027", "101500_300"),
            (Vec::new(), vec![])
        );
    }
}
