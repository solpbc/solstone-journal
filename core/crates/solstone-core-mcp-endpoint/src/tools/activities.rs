// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::Path;

use jiff::Span;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use solstone_core_indexer_query::{AdmittedCategory, ConnectionScope};
use solstone_core_transcripts::{MAX_TRANSCRIPT_PAGE_BYTES, MAX_TRANSCRIPT_PAGE_ITEMS};

use super::{
    MAX_DAY_BYTES, MAX_FACET_BYTES, MAX_OPAQUE_REFERENCE_BYTES, ToolError,
    optional_string_within_limit, search::MAX_LIMIT,
};
use crate::dispatch::{
    DispatchError, DispatchPrincipal, Prepared, available_facets, codec, entity_name, fold_literal,
    read_scoped_entities, read_scoped_entities_resolved,
};
use crate::permissions::ConnectionReadSnapshot;
use crate::references::{
    ActivityAddress, ActivityListCursor, ActivityReference, ReferenceKind, ReferenceTarget,
    is_framed_entity_token,
};
use crate::segment_links::{
    LinkSubject, LiveTranscriptPages, activity_segment_candidates, issue_segment_links,
};

pub(crate) const MAX_ACTIVITY_FILTER_ITEMS: usize = 20;
pub(crate) const MAX_ACTIVITY_TYPE_BYTES: usize = 128;
pub(crate) const MAX_GROUP_BY_GROUPS: usize = 500;

/// Resolved participation roles on an activity, matching the buckets returned by
/// `list_activities` (`attendee` maps to `participants`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ActivityRole {
    Attendee,
    Heard,
    Corresponded,
    InPhoto,
    Mentioned,
    OnScreen,
}

impl ActivityRole {
    pub(crate) const ALL: &'static [Self] = &[
        Self::Attendee,
        Self::Heard,
        Self::Corresponded,
        Self::InPhoto,
        Self::Mentioned,
        Self::OnScreen,
    ];

    /// Spoken or attended presence: the default when grouping by `person`
    /// without an explicit `roles` filter.
    pub(crate) const DEFAULT_PERSON_GROUP: &'static [Self] = &[Self::Attendee, Self::Heard];

    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Attendee => "attendee",
            Self::Heard => "heard",
            Self::Corresponded => "corresponded",
            Self::InPhoto => "in_photo",
            Self::Mentioned => "mentioned",
            Self::OnScreen => "on_screen",
        }
    }

    const fn from_rank(rank: u8) -> Self {
        match rank {
            5 => Self::Attendee,
            4 => Self::Heard,
            3 => Self::Corresponded,
            2 => Self::InPhoto,
            0 => Self::OnScreen,
            _ => Self::Mentioned,
        }
    }
}

/// Dimensions accepted by `list_activities`'s `group_by` server-side rollup.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum GroupByDimension {
    Month,
    Day,
    Facet,
    Activity,
    Person,
}

impl GroupByDimension {
    pub(crate) const ALL: &'static [Self] = &[
        Self::Month,
        Self::Day,
        Self::Facet,
        Self::Activity,
        Self::Person,
    ];

    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Month => "month",
            Self::Day => "day",
            Self::Facet => "facet",
            Self::Activity => "activity",
            Self::Person => "person",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ValidatedListActivities {
    pub(crate) day: Option<String>,
    pub(crate) from: String,
    pub(crate) to: String,
    pub(crate) facet: Option<String>,
    pub(crate) entity: Option<String>,
    pub(crate) person: Option<String>,
    pub(crate) roles: Option<Vec<ActivityRole>>,
    pub(crate) activity: Vec<String>,
    pub(crate) group_by: Option<Vec<GroupByDimension>>,
    pub(crate) compact: bool,
    pub(crate) limit: usize,
    pub(crate) cursor: Option<String>,
}

impl ValidatedListActivities {
    fn role_tokens(&self) -> Vec<String> {
        self.roles
            .as_ref()
            .map(|roles| roles.iter().map(|role| role.as_str().to_owned()).collect())
            .unwrap_or_default()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ValidatedGetActivitySources {
    pub(crate) reference: String,
    pub(crate) cursor: Option<String>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum ActivityTypeFilterInput {
    One(String),
    Several(Vec<String>),
}

impl ActivityTypeFilterInput {
    fn validate(self) -> Result<Vec<String>, ToolError> {
        let raw = match self {
            Self::One(word) => vec![word],
            Self::Several(words) => words,
        };
        if raw.is_empty() || raw.len() > MAX_ACTIVITY_FILTER_ITEMS {
            return Err(ToolError::InvalidInput);
        }
        let mut normalized = BTreeSet::new();
        for word in raw {
            if word.len() > MAX_ACTIVITY_TYPE_BYTES {
                return Err(ToolError::InvalidInput);
            }
            let norm = normalize_activity_type(&word);
            if norm.is_empty() {
                return Err(ToolError::InvalidInput);
            }
            normalized.insert(norm);
        }
        Ok(normalized.into_iter().collect())
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ListParams {
    #[serde(default)]
    day: Option<String>,
    // `from` and `to` are the names this tool took first; they stay accepted.
    #[serde(default, alias = "from")]
    day_from: Option<String>,
    #[serde(default, alias = "to")]
    day_to: Option<String>,
    #[serde(default)]
    facet: Option<String>,
    #[serde(default)]
    entity: Option<String>,
    #[serde(default)]
    person: Option<String>,
    #[serde(default)]
    roles: Option<Vec<ActivityRole>>,
    #[serde(default)]
    activity: Option<ActivityTypeFilterInput>,
    #[serde(default)]
    group_by: Option<Vec<GroupByDimension>>,
    #[serde(default)]
    compact: bool,
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    cursor: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GetSourcesParams {
    reference: String,
    #[serde(default)]
    cursor: Option<String>,
}

const fn default_limit() -> usize {
    10
}

fn validate_limit(limit: usize) -> Result<(), ToolError> {
    if limit == 0 || limit > MAX_LIMIT {
        Err(ToolError::InvalidInput)
    } else {
        Ok(())
    }
}

fn normalize_activity_type(value: &str) -> String {
    value.trim().replace('_', " ").to_lowercase()
}

fn validate_optional_trimmed(
    value: Option<String>,
    max_bytes: usize,
) -> Result<Option<String>, ToolError> {
    let Some(raw) = value else {
        return Ok(None);
    };
    if raw.len() > max_bytes {
        return Err(ToolError::InvalidInput);
    }
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(ToolError::InvalidInput);
    }
    Ok(Some(trimmed.to_owned()))
}

pub(crate) fn validate_list(
    params: Option<&Value>,
    owner_zone: &solstone_core_journal_config::TimeZone,
) -> Result<ValidatedListActivities, ToolError> {
    let params = params.cloned().unwrap_or_else(|| serde_json::json!({}));
    let params =
        serde_json::from_value::<ListParams>(params).map_err(|_| ToolError::InvalidInput)?;

    if !optional_string_within_limit(&params.day, MAX_DAY_BYTES)
        || !optional_string_within_limit(&params.day_from, MAX_DAY_BYTES)
        || !optional_string_within_limit(&params.day_to, MAX_DAY_BYTES)
        || !optional_string_within_limit(&params.facet, MAX_FACET_BYTES)
        || !optional_string_within_limit(&params.cursor, MAX_OPAQUE_REFERENCE_BYTES)
    {
        return Err(ToolError::InvalidInput);
    }

    let entity = validate_optional_trimmed(params.entity, MAX_OPAQUE_REFERENCE_BYTES)?;
    let person = validate_optional_trimmed(params.person, MAX_OPAQUE_REFERENCE_BYTES)?;
    let activity = params
        .activity
        .map(ActivityTypeFilterInput::validate)
        .transpose()?
        .unwrap_or_default();

    let group_by = if let Some(dims) = params.group_by {
        if dims.is_empty() || dims.len() > GroupByDimension::ALL.len() {
            return Err(ToolError::InvalidInput);
        }
        let mut seen = BTreeSet::new();
        for dim in &dims {
            if !seen.insert(*dim) {
                return Err(ToolError::InvalidInput);
            }
        }
        if params.limit.is_some() || params.cursor.is_some() || params.compact {
            return Err(ToolError::InvalidInput);
        }
        Some(dims)
    } else {
        None
    };

    let limit = match params.limit {
        Some(limit) => {
            validate_limit(limit)?;
            limit
        }
        None => default_limit(),
    };

    let roles = if let Some(roles) = params.roles {
        if roles.is_empty() || roles.len() > ActivityRole::ALL.len() {
            return Err(ToolError::InvalidInput);
        }
        let groups_by_person = group_by
            .as_ref()
            .is_some_and(|dims| dims.contains(&GroupByDimension::Person));
        if person.is_none() && entity.is_none() && !groups_by_person {
            return Err(ToolError::InvalidInput);
        }
        let deduped: BTreeSet<ActivityRole> = roles.into_iter().collect();
        Some(deduped.into_iter().collect())
    } else {
        None
    };

    if params.day.is_some() && (params.day_from.is_some() || params.day_to.is_some()) {
        return Err(ToolError::ActivityDayArguments);
    }

    let normalized_day = super::normalize_day(params.day)?;
    let normalized_from = super::normalize_day(params.day_from)?;
    let normalized_to = super::normalize_day(params.day_to)?;

    let (from, to) = if let Some(day) = normalized_day.clone() {
        (day.clone(), day)
    } else {
        match (normalized_from, normalized_to) {
            (None, None) => {
                let today = jiff::Timestamp::now()
                    .to_zoned_datetime(owner_zone.clone())
                    .strftime("%Y%m%d")
                    .to_string();
                (today.clone(), today)
            }
            (Some(f), None) => (f.clone(), f),
            (None, Some(t)) => (t.clone(), t),
            (Some(f), Some(t)) => (f, t),
        }
    };

    if to < from {
        return Err(ToolError::ActivityDayArguments);
    }

    Ok(ValidatedListActivities {
        day: normalized_day,
        from,
        to,
        facet: params.facet,
        entity,
        person,
        roles,
        activity,
        group_by,
        compact: params.compact,
        limit,
        cursor: params.cursor,
    })
}

pub(crate) fn validate_get_sources(
    params: Option<&Value>,
) -> Result<ValidatedGetActivitySources, ToolError> {
    let params = match params {
        None | Some(Value::Null) => return Err(ToolError::InvalidInput),
        Some(val) => val,
    };
    let params = serde_json::from_value::<GetSourcesParams>(params.clone())
        .map_err(|_| ToolError::InvalidInput)?;

    if params.reference.is_empty() || params.reference.len() > MAX_OPAQUE_REFERENCE_BYTES {
        return Err(ToolError::InvalidInput);
    }
    if !optional_string_within_limit(&params.cursor, MAX_OPAQUE_REFERENCE_BYTES) {
        return Err(ToolError::InvalidInput);
    }

    Ok(ValidatedGetActivitySources {
        reference: params.reference,
        cursor: params.cursor,
    })
}

pub(crate) fn activity_title(record: &Map<String, Value>) -> String {
    for key in ["title", "description", "activity", "id"] {
        if let Some(val) = record.get(key).and_then(Value::as_str) {
            let trimmed = val.trim();
            if !trimmed.is_empty() {
                if matches!(key, "activity" | "id") {
                    return trimmed
                        .replace('_', " ")
                        .split_whitespace()
                        .map(|part| {
                            let mut chars = part.chars();
                            chars
                                .next()
                                .map(|first| {
                                    first.to_uppercase().collect::<String>() + chars.as_str()
                                })
                                .unwrap_or_default()
                        })
                        .collect::<Vec<_>>()
                        .join(" ");
                } else {
                    return trimmed.to_string();
                }
            }
        }
    }
    "untitled activity".to_string()
}

pub(crate) fn parse_segment_time(segment: &str) -> Option<(u32, u32, u32, u32)> {
    let (time_part, length_part) = segment.split_once('_')?;
    if time_part.len() != 6
        || !time_part.bytes().all(|byte| byte.is_ascii_digit())
        || length_part.is_empty()
        || !length_part.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    let hour = time_part[0..2].parse::<u32>().ok()?;
    let minute = time_part[2..4].parse::<u32>().ok()?;
    let second = time_part[4..6].parse::<u32>().ok()?;
    if hour > 23 || minute > 59 || second > 59 {
        return None;
    }
    let duration = length_part.parse::<u32>().ok()?;
    Some((hour, minute, second, duration))
}

fn parse_time_string_to_seconds(s: &str) -> Option<u32> {
    let parts: Vec<&str> = s.split(':').collect();
    if parts.len() == 2 {
        let h = parts[0].parse::<u32>().ok()?;
        let m = parts[1].parse::<u32>().ok()?;
        if h < 24 && m < 60 {
            return Some(h * 3600 + m * 60);
        }
    } else if parts.len() == 3 {
        let h = parts[0].parse::<u32>().ok()?;
        let m = parts[1].parse::<u32>().ok()?;
        let sec = parts[2].parse::<u32>().ok()?;
        if h < 24 && m < 60 && sec < 60 {
            return Some(h * 3600 + m * 60 + sec);
        }
    }
    None
}

pub(crate) fn activity_times_and_duration(
    record: &Map<String, Value>,
) -> (Option<String>, Option<String>, Option<u64>) {
    if let Some(segments) = record.get("segments").and_then(Value::as_array)
        && !segments.is_empty()
    {
        let mut total_duration = 0u64;
        let mut first_start: Option<(u32, u32, u32)> = None;
        let mut last_end_seconds: Option<u32> = None;
        let mut parsed_any = false;
        for seg_val in segments {
            if let Some(seg_str) = seg_val.as_str()
                && let Some((h, m, s, dur)) = parse_segment_time(seg_str)
            {
                parsed_any = true;
                if first_start.is_none() {
                    first_start = Some((h, m, s));
                }
                let seg_end = h * 3600 + m * 60 + s + dur;
                last_end_seconds = Some(last_end_seconds.map_or(seg_end, |cur| cur.max(seg_end)));
                total_duration += dur as u64;
            }
        }
        let start = first_start.map(|(h, m, s)| format!("{h:02}:{m:02}:{s:02}"));
        let end = last_end_seconds.map(|sec| {
            let clamped = sec.min(23 * 3600 + 59 * 60 + 59);
            format!(
                "{:02}:{:02}:{:02}",
                clamped / 3600,
                (clamped % 3600) / 60,
                clamped % 60
            )
        });
        let duration = if parsed_any {
            Some(total_duration)
        } else {
            None
        };
        return (start, end, duration);
    }

    let start = record
        .get("start")
        .and_then(Value::as_str)
        .map(str::to_string);
    let end = record
        .get("end")
        .and_then(Value::as_str)
        .map(str::to_string);
    let duration = match (&start, &end) {
        (Some(s), Some(e)) => {
            if let (Some(sec_s), Some(sec_e)) = (
                parse_time_string_to_seconds(s),
                parse_time_string_to_seconds(e),
            ) {
                if sec_e >= sec_s {
                    Some((sec_e - sec_s) as u64)
                } else {
                    None
                }
            } else {
                None
            }
        }
        _ => None,
    };
    (start, end, duration)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ImageSegmentKind {
    Photo,
    Screenshot,
    Other,
}

fn classify_image_segment(
    journal_root: &Path,
    day: &str,
    stream: &str,
    segment: &str,
) -> ImageSegmentKind {
    if !solstone_core_journal_io::is_day_key(day)
        || segment.is_empty()
        || segment.contains('/')
        || segment.contains('\\')
        || segment == "."
        || segment == ".."
    {
        return ImageSegmentKind::Other;
    }
    for prefix in ["chronicle/", ""] {
        let rel = format!("{prefix}{day}/{stream}/{segment}");
        let Ok(dir) = solstone_core_journal_io::contained_path(journal_root, &rel) else {
            continue;
        };
        let Ok(Some(entries)) = solstone_core_journal_io::list_dir_entries_bounded(&dir, 64) else {
            continue;
        };
        let mut has_photo = false;
        let mut has_screenshot = false;
        for entry in entries {
            if entry.kind != solstone_core_journal_io::DirEntryKind::File {
                continue;
            }
            if entry.name == "image_transcript.md" {
                has_photo = true;
            } else if entry.name == "screenshot_transcript.md" {
                has_screenshot = true;
            }
        }
        if has_screenshot {
            return ImageSegmentKind::Screenshot;
        }
        if has_photo {
            return ImageSegmentKind::Photo;
        }
    }
    ImageSegmentKind::Other
}

/// True when `record` comes from `import.image` or `photos` and every segment
/// it names on `day` is a real photo segment (`image_transcript.md` present and
/// no `screenshot_transcript.md`). Never infers from activity IDs.
pub(crate) fn is_real_photo_record(
    journal_root: &Path,
    day: &str,
    record: &Map<String, Value>,
    cache: &mut BTreeMap<(String, String), ImageSegmentKind>,
) -> bool {
    let Some(stream) = record.get("stream").and_then(Value::as_str) else {
        return false;
    };
    if stream != "import.image"
        && stream != "photos"
        && !solstone_core_segment::is_photos_stream(journal_root, stream)
    {
        return false;
    }
    let Some(segments) = record.get("segments").and_then(Value::as_array) else {
        return false;
    };
    let seg_names: Vec<&str> = segments
        .iter()
        .filter_map(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    if seg_names.is_empty() {
        return false;
    }
    seg_names.into_iter().all(|seg| {
        let key = (day.to_owned(), format!("{stream}/{seg}"));
        let kind = *cache
            .entry(key)
            .or_insert_with(|| classify_image_segment(journal_root, day, stream, seg));
        kind == ImageSegmentKind::Photo
    })
}

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct ActivityPeople {
    pub participants: Vec<String>,
    pub heard: Vec<String>,
    pub corresponded: Vec<String>,
    pub in_photo: Vec<String>,
    pub mentioned: Vec<String>,
    pub on_screen: Vec<String>,
    /// Voices heard that the journal has no name for.
    pub unnamed_voices: usize,
}

/// A voice the journal has no name for, as participation records it:
/// `Speaker 3`. It is a voice, not a person.
pub(crate) fn is_unnamed_voice(name: &str) -> bool {
    let lower = name.to_lowercase();
    let Some(rest) = lower.strip_prefix("speaker") else {
        return false;
    };
    let rest = rest.trim_start();
    !rest.is_empty() && rest.bytes().all(|byte| byte.is_ascii_digit())
}

/// How strongly a role places someone in an activity; a name recorded under
/// two roles is listed under the stronger.
fn role_rank(role: &str, photo_record: bool) -> u8 {
    match role {
        "attendee" => 5,
        "heard" => 4,
        "corresponded" => 3,
        "visible" if photo_record => 2,
        "visible" => 0,
        _ => 1,
    }
}

/// Classifies an activity's participants into participants, heard, in_photo,
/// mentioned, and on-screen names.
#[cfg(test)]
pub(crate) fn activity_people(record: &Map<String, Value>) -> ActivityPeople {
    activity_people_across(std::iter::once(record))
}

/// The people of one moment, read from every facet's copy of it. Each name is
/// listed once, under the strongest role any copy gives it, in the order the
/// copies first name it. A voice with no name is counted, never listed.
#[cfg(test)]
pub(crate) fn activity_people_across<'a>(
    records: impl Iterator<Item = &'a Map<String, Value>>,
) -> ActivityPeople {
    activity_people_in_context(records, None, None, None, None)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ResolvedParticipant {
    name: String,
    entity_ids: Vec<String>,
    role: ActivityRole,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct ResolvedMomentPeople {
    participants: Vec<ResolvedParticipant>,
    unnamed_voices: usize,
}

impl ResolvedMomentPeople {
    fn to_activity_people(&self) -> ActivityPeople {
        let mut people = ActivityPeople {
            unnamed_voices: self.unnamed_voices,
            ..ActivityPeople::default()
        };
        let push_unique = |bucket: &mut Vec<String>, name: &str| {
            if !bucket.iter().any(|known| known.eq_ignore_ascii_case(name)) {
                bucket.push(name.to_owned());
            }
        };
        for participant in &self.participants {
            match participant.role {
                ActivityRole::Attendee => push_unique(&mut people.participants, &participant.name),
                ActivityRole::Heard => push_unique(&mut people.heard, &participant.name),
                ActivityRole::Corresponded => {
                    push_unique(&mut people.corresponded, &participant.name)
                }
                ActivityRole::InPhoto => push_unique(&mut people.in_photo, &participant.name),
                ActivityRole::Mentioned => push_unique(&mut people.mentioned, &participant.name),
                ActivityRole::OnScreen => push_unique(&mut people.on_screen, &participant.name),
            }
        }
        people
    }
}

fn resolve_activity_participants<'a>(
    records: impl Iterator<Item = &'a Map<String, Value>>,
    journal_root: Option<&Path>,
    day: Option<&str>,
    mut photo_cache: Option<&mut BTreeMap<(String, String), ImageSegmentKind>>,
    exclude_owner: Option<&crate::people_view::OwnerIdentity>,
) -> ResolvedMomentPeople {
    let mut local_cache = BTreeMap::new();
    // (display name as first written, seen entity_ids, strongest role rank so far).
    let mut named: Vec<(String, Vec<String>, u8)> = Vec::new();
    let mut unnamed_voices = 0usize;
    for record in records {
        let Some(entries) = record.get("participation").and_then(Value::as_array) else {
            continue;
        };
        let mut photo_record: Option<bool> = None;
        let mut voices = 0usize;
        for entry in entries.iter().filter_map(Value::as_object) {
            let Some(name) = entry.get("name").and_then(Value::as_str) else {
                continue;
            };
            let trimmed = name.trim();
            if trimmed.is_empty() {
                continue;
            }
            let role = entry
                .get("role")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if is_unnamed_voice(trimmed) {
                // Counted only where it was heard: an attendee or a heard voice.
                if role_rank(role, false) >= 4 {
                    voices += 1;
                }
                continue;
            }
            let explicit_id = entry
                .get("entity_id")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty());
            if let Some(owner) = exclude_owner
                && owner.excludes(explicit_id, trimmed)
            {
                continue;
            }
            let is_photo = if role == "visible" {
                *photo_record.get_or_insert_with(|| match (journal_root, day) {
                    (Some(root), Some(day_key)) => {
                        let cache = photo_cache.as_deref_mut().unwrap_or(&mut local_cache);
                        is_real_photo_record(root, day_key, record, cache)
                    }
                    _ => false,
                })
            } else {
                false
            };
            let rank = role_rank(role, is_photo);
            let existing_idx = if let Some(eid) = explicit_id {
                named
                    .iter()
                    .position(|(_, ids, _)| ids.iter().any(|known| known == eid))
                    .or_else(|| {
                        named.iter().position(|(known, ids, _)| {
                            ids.is_empty() && known.eq_ignore_ascii_case(trimmed)
                        })
                    })
            } else {
                named
                    .iter()
                    .position(|(known, _, _)| known.eq_ignore_ascii_case(trimmed))
            };
            match existing_idx {
                Some(idx) => {
                    let (_, ids, known_rank) = &mut named[idx];
                    *known_rank = (*known_rank).max(rank);
                    if let Some(eid) = explicit_id
                        && !ids.iter().any(|known| known == eid)
                    {
                        ids.push(eid.to_owned());
                    }
                }
                None => {
                    let ids = explicit_id
                        .map(|eid| vec![eid.to_owned()])
                        .unwrap_or_default();
                    named.push((trimmed.to_owned(), ids, rank));
                }
            }
        }
        // Copies hear the same recording, so the most any one copy counted.
        unnamed_voices = unnamed_voices.max(voices);
    }
    let participants = named
        .into_iter()
        .map(|(name, entity_ids, rank)| ResolvedParticipant {
            name,
            entity_ids,
            role: ActivityRole::from_rank(rank),
        })
        .collect();
    ResolvedMomentPeople {
        participants,
        unnamed_voices,
    }
}

/// Like `activity_people_across`, resolving real photo provenance when
/// `journal_root` and `day` are given (so `visible` on an `import.image`
/// photo splits into `in_photo` while screen captures and imported screenshots
/// remain `on_screen`) and optionally excluding the journal's owner.
pub(crate) fn activity_people_in_context<'a>(
    records: impl Iterator<Item = &'a Map<String, Value>>,
    journal_root: Option<&Path>,
    day: Option<&str>,
    photo_cache: Option<&mut BTreeMap<(String, String), ImageSegmentKind>>,
    exclude_owner: Option<&crate::people_view::OwnerIdentity>,
) -> ActivityPeople {
    resolve_activity_participants(records, journal_root, day, photo_cache, exclude_owner)
        .to_activity_people()
}

/// What kind of activity the journal recorded: meeting, writing and so on,
/// in its own word.
pub(crate) fn activity_kind(record: &Map<String, Value>) -> Option<String> {
    let kind = record.get("activity").and_then(Value::as_str)?.trim();
    (!kind.is_empty()).then(|| kind.replace('_', " "))
}

pub(crate) fn activity_entities(record: &Map<String, Value>) -> Vec<String> {
    record
        .get("active_entities")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(Value::as_str)
                .map(str::trim)
                .filter(|e| !e.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// One item a beside line can name: its span as instants, and the line.
type BesideLine = ((i64, i64), Value);

/// A face's span on its first account's instants.
fn face_span(face: &Value) -> Option<(i64, i64)> {
    let first = face.get("accounts")?.get(0)?;
    let time = |field: &str| {
        solstone_core_format::content::body_records::parse_source_time(first.get(field)?.as_str()?)
            .map(|time| time.timestamp().as_second())
    };
    let (start, end) = (time("start")?, time("end")?);
    (end > start).then_some((start, end))
}

/// What a beside line says of a body face: its kind, the owner's title or the
/// source's label, and its times. No reading values.
fn body_line(face: &Value) -> Value {
    let first = face.get("accounts").and_then(|accounts| accounts.get(0));
    json!({
        "kind": face.get("activity").cloned().unwrap_or(Value::Null),
        "title": face
            .get("title")
            .cloned()
            .or_else(|| first.and_then(|first| first.get("label")).cloned())
            .unwrap_or(Value::Null),
        "start": first.and_then(|first| first.get("start")).cloned().unwrap_or(Value::Null),
        "end": first.and_then(|first| first.get("end")).cloned().unwrap_or(Value::Null),
    })
}

/// Body faces this connection reads that can sit beside a capture activity on
/// `day`: those shown under it and under the day before, for a night.
fn body_lines_near(
    journal_root: &Path,
    body: &mut [(String, solstone_core_location::body_records::Groups)],
    day: &str,
    targets: &mut Vec<String>,
) -> Vec<BesideLine> {
    let previous = solstone_core_time::parse_day(day)
        .and_then(|date| date.yesterday().ok())
        .map(solstone_core_time::day_key);
    let mut lines = Vec::new();
    for on in std::iter::once(day.to_owned()).chain(previous) {
        for (_, groups) in body.iter_mut() {
            for group in groups.on(&on).to_vec() {
                let face = groups.compose(journal_root, &group);
                if !solstone_core_location::body_records::face_still_there(&face) {
                    continue;
                }
                crate::body_source::name_windows(targets, crate::body_source::face_windows(&face));
                let value = solstone_core_location::body_records::face_json(&face);
                if let Some(span) = face_span(&value) {
                    lines.push((span, body_line(&value)));
                }
            }
        }
    }
    lines
}

/// How many days of body groups `list_activities` reads at a time.
const BODY_STRETCH_DAYS: i64 = 31;

/// Capture activities this connection reads that can sit beside a body item
/// shown under `day`: those on it and on the next day, for a night. Beside
/// lines: title and type.
fn capture_lines_on(
    journal_root: &Path,
    facets: &[(String, String, String)],
    day: &str,
) -> Vec<BesideLine> {
    let mut lines = Vec::new();
    let next = solstone_core_location::body_records::next_day(day);
    for on in std::iter::once(day.to_owned()).chain(next) {
        lines.extend(capture_lines_of(journal_root, facets, &on));
    }
    lines
}

fn capture_lines_of(
    journal_root: &Path,
    facets: &[(String, String, String)],
    day: &str,
) -> Vec<BesideLine> {
    let mut lines = Vec::new();
    for (_, facet_dir, _) in facets {
        for record in
            solstone_core_facets::load_activity_records(journal_root, facet_dir, day, false)
                .unwrap_or_default()
        {
            if let Some(span) =
                solstone_core_location::body_records::capture_instants(journal_root, day, &record)
            {
                lines.push((
                    span,
                    json!({"title": activity_title(&record), "activity": record.get("activity").cloned().unwrap_or(Value::Null)}),
                ));
            }
        }
    }
    lines
}

/// A body face as `list_activities` gives it: the owner's title when they set
/// one, else the first account's label, its kind, and every account with its
/// own composed line. Nothing is summed or chosen.
pub(crate) fn body_activity(
    face: &Value,
    facet_title: &str,
    day: &str,
    reference: String,
    zone: &str,
) -> Value {
    let accounts = face
        .get("accounts")
        .cloned()
        .unwrap_or(Value::Array(Vec::new()));
    let first = accounts.get(0).cloned().unwrap_or(Value::Null);
    let title = face
        .get("title")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .or_else(|| {
            first
                .get("label")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .unwrap_or_default();
    let mut activity = json!({
        "title": title,
        "facet": facet_title,
        "day": day,
        "content_kind": "body",
        "source": "body",
        "kind": face.get("activity").cloned().unwrap_or(Value::Null),
        "start": first.get("start").cloned().unwrap_or(Value::Null),
        "end": first.get("end").cloned().unwrap_or(Value::Null),
        "accounts": accounts,
        "reference": reference,
        "zone": zone,
    });
    for field in ["description", "details"] {
        if let Some(value) = face.get(field) {
            activity[field] = value.clone();
        }
    }
    activity
}

pub(crate) fn sort_activity_key(
    file_day: &str,
    facet_id: &str,
    record: &Map<String, Value>,
) -> (String, String, i64, String) {
    let created_at = record
        .get("created_at")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let id_val = record
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    (
        file_day.to_string(),
        facet_id.to_string(),
        created_at,
        id_val,
    )
}

pub(crate) fn is_path_or_segment_pointer(val: &Value) -> bool {
    match val {
        Value::Object(_) => true,
        Value::String(s) => {
            s.contains('/')
                || s.contains('\\')
                || s.contains(':')
                || s.ends_with(".md")
                || s.ends_with(".json")
                || s.ends_with(".jsonl")
        }
        _ => false,
    }
}

#[cfg(test)]
pub(crate) fn clean_activity_record_for_wire(record: Map<String, Value>) -> Map<String, Value> {
    clean_activity_record_for_wire_with_owner(record, None)
}

pub(crate) fn clean_activity_record_for_wire_with_owner(
    mut record: Map<String, Value>,
    owner: Option<&crate::people_view::OwnerIdentity>,
) -> Map<String, Value> {
    record.remove("stream");
    // The talent's list before the journal judged it: it can name the owner
    // and call someone an attendee the journal found no proof of.
    record.remove("participation_said");
    if let Some(owner) = owner
        && let Some(Value::Array(participation)) = record.get_mut("participation")
    {
        participation.retain(|entry| {
            let Some(obj) = entry.as_object() else {
                return true;
            };
            let entity_id = obj
                .get("entity_id")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty());
            let name = obj.get("name").and_then(Value::as_str).unwrap_or_default();
            !owner.excludes(entity_id, name)
        });
    }
    if let Some(src) = record.get("source")
        && is_path_or_segment_pointer(src)
    {
        record.remove("source");
    }
    // What the owner retracted, or wrote over, never reaches their agent: an
    // edit goes out without the values it recorded, and a removed one not at all.
    if let Some(Value::Array(edits)) = record.get_mut("edits") {
        edits.retain(|edit| {
            !edit
                .as_object()
                .is_some_and(solstone_core_facets::edit_removed)
        });
        for edit in edits.iter_mut().filter_map(Value::as_object_mut) {
            edit.remove(solstone_core_facets::EDIT_SET);
            edit.remove(solstone_core_facets::EDIT_BEFORE);
            edit.remove(solstone_core_facets::EDIT_LINES);
            edit.remove(solstone_core_facets::EDIT_SPEAKER);
        }
    }
    record
}

enum EntityFilter {
    Token {
        entity_id: String,
        folded_name: String,
    },
    /// Words a record's names contain, or the ids of the entities this
    /// list reaches that the words name exactly.
    Literal {
        target: String,
        entity_ids: HashSet<String>,
    },
}

fn resolve_entity_filter(
    journal_root: &Path,
    principal: DispatchPrincipal<'_>,
    snapshot: &ConnectionReadSnapshot,
    facets_to_search: &[(String, String, String)],
    raw: &str,
) -> Result<EntityFilter, DispatchError> {
    if is_framed_entity_token(raw) {
        if !snapshot.categories.contains(&AdmittedCategory::Entities) {
            return Err(DispatchError::Tool(ToolError::ReferenceNotFound));
        }
        let ReferenceTarget::Entity(entity_ref) = codec()
            .resolve(
                raw,
                ReferenceKind::Entity,
                principal.connection,
                snapshot.generation,
            )
            .map_err(|_| DispatchError::Tool(ToolError::ReferenceNotFound))?
        else {
            return Err(DispatchError::Tool(ToolError::ReferenceNotFound));
        };
        let Some((_, facet_dir, _)) = facets_to_search
            .iter()
            .find(|(id, _, _)| id == &entity_ref.facet_id)
        else {
            return Err(DispatchError::Tool(ToolError::ReferenceNotFound));
        };
        let item = read_scoped_entities(journal_root, facet_dir)?
            .into_iter()
            .find(|item| item.entity_id == entity_ref.entity_id)
            .ok_or(DispatchError::Tool(ToolError::ReferenceNotFound))?;
        let name = entity_name(&item.identity);
        Ok(EntityFilter::Token {
            entity_id: entity_ref.entity_id,
            folded_name: fold_literal(&name),
        })
    } else if let Some((entity_id, facet_dir)) =
        crate::names::parse_entity(raw).and_then(|(entity_id, facet)| {
            facets_to_search
                .iter()
                .find(|(id, _, _)| id == &facet)
                .or_else(|| facets_to_search.iter().find(|(_, dir, _)| dir == &facet))
                .map(|(_, dir, _)| (entity_id, dir.clone()))
        })
    {
        // An entity's name in a facet this list reaches: the entity this
        // connection's own list shows there, or not found. Anything else
        // written with an `@` is read as the words it is.
        if !snapshot.categories.contains(&AdmittedCategory::Entities) {
            return Err(DispatchError::Tool(ToolError::ReferenceNotFound));
        }
        let item = read_scoped_entities(journal_root, &facet_dir)?
            .into_iter()
            .find(|item| item.entity_id == entity_id)
            .ok_or(DispatchError::Tool(ToolError::ReferenceNotFound))?;
        Ok(EntityFilter::Token {
            entity_id,
            folded_name: fold_literal(&entity_name(&item.identity)),
        })
    } else {
        let target = fold_literal(raw);
        let mut entity_ids = HashSet::new();
        // A record can name an entity by its id: merges before 2.0.82
        // wrote the survivor's id where its name belonged. Words that are
        // a reachable entity's name find those records too, and only for
        // a connection that reads entities. An aka is never matched: it
        // stays out of every answer.
        if snapshot.categories.contains(&AdmittedCategory::Entities) {
            // It reads every entity in the journal, so once for all facets.
            let identities = solstone_core_facets::read_identity_map(journal_root)
                .map_err(|_| DispatchError::Tool(ToolError::FileUnreadable))?;
            for (_, facet_dir, _) in facets_to_search {
                for item in read_scoped_entities_resolved(journal_root, facet_dir, &identities)? {
                    if fold_literal(&entity_name(&item.identity)) == target {
                        entity_ids.insert(item.entity_id);
                    }
                }
            }
        }
        Ok(EntityFilter::Literal { target, entity_ids })
    }
}

fn participant_matches_filter(participant: &ResolvedParticipant, filter: &EntityFilter) -> bool {
    match filter {
        EntityFilter::Token {
            entity_id,
            folded_name,
        } => {
            participant.entity_ids.iter().any(|id| id == entity_id)
                || participant.name == *entity_id
                || fold_literal(&participant.name) == *folded_name
        }
        EntityFilter::Literal { target, entity_ids } => {
            participant
                .entity_ids
                .iter()
                .any(|id| entity_ids.contains(id))
                || entity_ids.contains(&participant.name)
                || fold_literal(&participant.name).contains(target.as_str())
        }
    }
}

fn active_entities_match_filter<'a>(
    records: impl Iterator<Item = &'a Map<String, Value>>,
    filter: &EntityFilter,
) -> bool {
    for record in records {
        for entity in activity_entities(record) {
            let matched = match filter {
                EntityFilter::Token {
                    entity_id,
                    folded_name,
                } => entity == *entity_id || fold_literal(&entity) == *folded_name,
                EntityFilter::Literal { target, entity_ids } => {
                    entity_ids.contains(&entity) || fold_literal(&entity).contains(target.as_str())
                }
            };
            if matched {
                return true;
            }
        }
    }
    false
}

fn record_matches_activity_filter(record: &Map<String, Value>, wanted: &[String]) -> bool {
    if wanted.is_empty() {
        return true;
    }
    activity_kind(record)
        .map(|kind| normalize_activity_type(&kind))
        .is_some_and(|kind| wanted.iter().any(|target| target == &kind))
}

fn effective_rollup_roles(request: &ValidatedListActivities) -> Option<&[ActivityRole]> {
    if let Some(ref roles) = request.roles {
        Some(roles.as_slice())
    } else if request
        .group_by
        .as_ref()
        .is_some_and(|dims| dims.contains(&GroupByDimension::Person))
    {
        Some(ActivityRole::DEFAULT_PERSON_GROUP)
    } else {
        None
    }
}

const NO_ACTIVITIES_NOTE: &str = "no activities. this connection can see none that match. this is a complete answer, not an error.";

fn empty_activities_response(request: &ValidatedListActivities) -> Prepared {
    if let Some(ref dims) = request.group_by {
        let mut response = json!({
            "group_by": dims.iter().map(|d| d.as_str()).collect::<Vec<_>>(),
            "groups": [],
            "total": {
                "count": 0,
                "timed_count": 0,
                "duration_sec": 0,
            },
        });
        if let Some(roles) = effective_rollup_roles(request) {
            response["roles"] = json!(roles.iter().map(|r| r.as_str()).collect::<Vec<_>>());
        }
        Prepared::new(response, 0, Vec::new()).when_empty(NO_ACTIVITIES_NOTE)
    } else {
        Prepared::new(json!({ "activities": [] }), 0, Vec::new()).when_empty(NO_ACTIVITIES_NOTE)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum GroupKeyPart {
    Month(String),
    Day(String),
    Facet {
        title_folded: String,
        id: String,
        title: String,
    },
    Activity(Option<String>),
    Person(String),
}

#[derive(Default)]
struct GroupBucket {
    count: usize,
    timed_count: usize,
    duration_sec: u64,
    person_display: Option<String>,
}

fn day_to_month(day: &str) -> String {
    match (day.get(0..4), day.get(4..6)) {
        (Some(year), Some(month)) => format!("{year}-{month}"),
        _ => day.to_owned(),
    }
}

pub(crate) fn execute_list(
    journal_root: &Path,
    principal: DispatchPrincipal<'_>,
    snapshot: &ConnectionReadSnapshot,
    request: ValidatedListActivities,
) -> Result<Prepared, DispatchError> {
    let available = available_facets(journal_root, snapshot)?;
    let all_facets: Vec<(String, String, String)> = available
        .into_iter()
        .map(|(id, dir, decl)| {
            let title = decl.as_ref().map_or_else(
                || dir.clone(),
                |v| {
                    if v.title.is_empty() {
                        dir.clone()
                    } else {
                        v.title.clone()
                    }
                },
            );
            (id, dir, title)
        })
        .collect();

    let facets_to_search: Vec<(String, String, String)> =
        if let Some(ref facet_filter) = request.facet {
            let matching: Vec<_> = all_facets
                .into_iter()
                .filter(|(id, dir, _)| id == facet_filter || dir == facet_filter)
                .collect();
            if matching.is_empty() {
                return Ok(empty_activities_response(&request));
            }
            matching
        } else {
            all_facets
        };

    let entity_filter: Option<EntityFilter> = match request.entity.as_deref() {
        Some(raw) => Some(resolve_entity_filter(
            journal_root,
            principal,
            snapshot,
            &facets_to_search,
            raw,
        )?),
        None => None,
    };
    let person_filter: Option<EntityFilter> = match request.person.as_deref() {
        Some(raw) => Some(resolve_entity_filter(
            journal_root,
            principal,
            snapshot,
            &facets_to_search,
            raw,
        )?),
        None => None,
    };
    let has_person_or_entity = entity_filter.is_some() || person_filter.is_some();

    let cursor_bound = if let Some(ref cursor_token) = request.cursor {
        let ReferenceTarget::ActivityList(c) = codec()
            .resolve(
                cursor_token,
                ReferenceKind::ActivityList,
                principal.connection,
                snapshot.generation,
            )
            .map_err(|_| DispatchError::Tool(ToolError::ReferenceNotFound))?
        else {
            return Err(DispatchError::Tool(ToolError::ReferenceNotFound));
        };
        if c.day != request.day
            || c.from.as_deref() != Some(&request.from)
            || c.to.as_deref() != Some(&request.to)
            || c.facet != request.facet
            || c.entity != request.entity
            || c.person != request.person
            || c.roles != request.role_tokens()
            || c.activity != request.activity
            || c.compact != request.compact
        {
            return Err(DispatchError::Tool(ToolError::ReferenceNotFound));
        }
        Some((c.last_day, c.last_facet, c.last_created_at, c.last_id))
    } else {
        None
    };

    struct CollectedItem {
        sort_key: (String, String, i64, String),
        record: Map<String, Value>,
        facet_id: String,
        facet_dir: String,
        facet_title: String,
        day: String,
        file_digest: String,
        rec_identity: String,
        /// A body record's group as this connection reads it, and the
        /// stretch it was read in; composed once the item is on the page.
        group: Option<(usize, solstone_core_location::body_records::Group)>,
        /// The other facets holding a copy of this moment, by title, and
        /// those copies' records.
        also_in: Vec<String>,
        copies: Vec<Map<String, Value>>,
        /// Cached resolved participants across primary + copies, computed once
        /// when person/role filtering, `group_by: ["person"]`, or full row
        /// serialization needs it.
        resolved_people: Option<ResolvedMomentPeople>,
    }

    let mut collected = Vec::new();
    let mut targets = Vec::new();
    let gate = crate::body_source::window_gate(journal_root, snapshot);
    // Each facet's body groups this connection reads, grouped once: from the
    // day before the first day this page can show (a night beside it) to the
    // range's last. Nothing anchored before a cursor's day can be on its page.
    let first_shown = match &cursor_bound {
        Some((last_day, ..)) if last_day > &request.from => last_day,
        _ => &request.from,
    };
    let before_from = solstone_core_time::parse_day(first_shown)
        .and_then(|date| date.yesterday().ok())
        .map_or_else(|| first_shown.clone(), solstone_core_time::day_key);
    // Body rows as read, by facet, day and id, until their groups are read.
    let mut body_rows: BTreeMap<(String, String, String), CollectedItem> = BTreeMap::new();
    // A body record or Strava line this connection would list with the body
    // permission, so the answer says it needs it.
    let mut body_withheld = false;

    for (facet_id, facet_dir, facet_title) in &facets_to_search {
        let stems = solstone_core_facets::list_activity_day_stems(journal_root, facet_dir)
            .map_err(|_| DispatchError::Tool(ToolError::FileUnreadable))?;
        for day in stems {
            if day < request.from || day > request.to {
                continue;
            }
            targets.push(format!("facets/{facet_dir}/activities/{day}.jsonl"));
            let normalized =
                solstone_core_facets::read_activity_records_strict(journal_root, facet_dir, &day)
                    .map_err(|_| DispatchError::Tool(ToolError::FileUnreadable))?;

            let rel_path = format!("facets/{facet_dir}/activities/{day}.jsonl");
            let Some(bytes) = crate::activity_admit::read_contained_source(journal_root, &rel_path)
            else {
                return Err(DispatchError::Tool(ToolError::FileUnreadable));
            };
            let file_digest = crate::activity_admit::hex_encode(&Sha256::digest(&bytes));
            let Ok(text) = String::from_utf8(bytes) else {
                return Err(DispatchError::Tool(ToolError::FileUnreadable));
            };
            let mut raw_rows = Vec::new();
            for line in text.lines() {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                let Ok(Value::Object(record)) = serde_json::from_str::<Value>(trimmed) else {
                    return Err(DispatchError::Tool(ToolError::FileUnreadable));
                };
                raw_rows.push((record, trimmed.to_owned()));
            }
            if raw_rows.len() != normalized.len() {
                return Err(DispatchError::Tool(ToolError::FileUnreadable));
            }
            // The same test every read of one of these records makes again.
            let judged = crate::may_see::ActivityDay::new(
                journal_root,
                snapshot,
                facet_dir,
                &day,
                &normalized,
            );
            for (record, trimmed) in raw_rows {
                // Identity stays the raw line, not the normalized object.
                match judged.shows(&record) {
                    crate::may_see::Shown::Yes => {}
                    crate::may_see::Shown::No => continue,
                    // A body record or Strava line this connection would list
                    // with the body permission, so the answer says it needs it.
                    crate::may_see::Shown::NeedsBody => {
                        body_withheld = body_withheld
                            || (!has_person_or_entity
                                && record_matches_activity_filter(&record, &request.activity));
                        continue;
                    }
                }
                if solstone_core_facets::is_body_record(&record) {
                    // A body record shows as its face, only for windows this
                    // connection reads; it names no entity or person, so those
                    // filters leave it out.
                    if !has_person_or_entity
                        && record_matches_activity_filter(&record, &request.activity)
                        && let Some(id) = record.get("id").and_then(Value::as_str)
                    {
                        body_rows.insert(
                            (facet_dir.clone(), day.clone(), id.to_owned()),
                            CollectedItem {
                                sort_key: sort_activity_key(&day, facet_id, &record),
                                rec_identity: crate::activity_admit::compute_record_identity(
                                    &record, &trimmed,
                                ),
                                record,
                                facet_id: facet_id.clone(),
                                facet_dir: facet_dir.clone(),
                                facet_title: facet_title.clone(),
                                day: day.clone(),
                                file_digest: file_digest.clone(),
                                group: None,
                                also_in: Vec::new(),
                                copies: Vec::new(),
                                resolved_people: None,
                            },
                        );
                    }
                    continue;
                }
                let rec_identity =
                    crate::activity_admit::compute_record_identity(&record, &trimmed);
                let sort_key = sort_activity_key(&day, facet_id, &record);
                collected.push(CollectedItem {
                    sort_key,
                    record,
                    facet_id: facet_id.clone(),
                    facet_dir: facet_dir.clone(),
                    facet_title: facet_title.clone(),
                    day: day.clone(),
                    file_digest: file_digest.clone(),
                    rec_identity,
                    group: None,
                    also_in: Vec::new(),
                    copies: Vec::new(),
                    resolved_people: None,
                });
            }
        }
    }

    let needs_owner_now = has_person_or_entity
        || request
            .group_by
            .as_ref()
            .is_some_and(|dims| dims.contains(&GroupByDimension::Person))
        || (request.group_by.is_none() && !request.compact);
    let owner = needs_owner_now.then(|| crate::people_view::OwnerIdentity::load(journal_root));
    let mut photo_cache: BTreeMap<(String, String), ImageSegmentKind> = BTreeMap::new();

    // A moment recorded in several facets is listed once, from the facet it
    // belongs to most, with the others named beside it. Activity-type and
    // role-aware person/entity filters run on the folded moment so multi-facet
    // role promotion matches the rendered row and rollup identically.
    collected.sort_by(|a, b| a.sort_key.cmp(&b.sort_key));
    let mut collected: Vec<CollectedItem> = solstone_core_facets::merge_moment_copies(
        collected,
        |item| item.day.as_str(),
        |item| &item.record,
    )
    .into_iter()
    .filter_map(|moment| {
        let mut primary = moment.primary;
        for copy in moment.copies {
            primary.also_in.push(copy.facet_title);
            primary.copies.push(copy.record);
        }
        if !record_matches_activity_filter(&primary.record, &request.activity) {
            return None;
        }
        if has_person_or_entity {
            let resolved = resolve_activity_participants(
                std::iter::once(&primary.record).chain(primary.copies.iter()),
                Some(journal_root),
                Some(&primary.day),
                Some(&mut photo_cache),
                owner.as_ref(),
            );
            if let Some(ref pf) = person_filter {
                let person_ok = resolved.participants.iter().any(|participant| {
                    participant_matches_filter(participant, pf)
                        && request
                            .roles
                            .as_ref()
                            .is_none_or(|allowed| allowed.contains(&participant.role))
                });
                if !person_ok {
                    return None;
                }
            }
            if let Some(ref ef) = entity_filter {
                let entity_ok = if person_filter.is_none()
                    && let Some(ref allowed) = request.roles
                {
                    resolved.participants.iter().any(|participant| {
                        participant_matches_filter(participant, ef)
                            && allowed.contains(&participant.role)
                    })
                } else {
                    active_entities_match_filter(
                        std::iter::once(&primary.record).chain(primary.copies.iter()),
                        ef,
                    ) || resolved
                        .participants
                        .iter()
                        .any(|participant| participant_matches_filter(participant, ef))
                };
                if !entity_ok {
                    return None;
                }
            }
            primary.resolved_people = Some(resolved);
        }
        Some(primary)
    })
    .collect();

    // Body groups, read a stretch of days at a time from the day before the
    // first this page can show, until the page and the item after it are
    // settled: everything sorts by day first, so nothing on a later day can
    // come before them. Each stretch groups as its days do alone.
    // Compact and group_by modes emit no beside lines, so they skip stretches
    // altogether when no body rows matched.
    let mut body: Vec<(String, solstone_core_location::body_records::Groups)> = Vec::new();
    let need_body_groups =
        !body_rows.is_empty() || (request.group_by.is_none() && !request.compact);
    if need_body_groups
        && let (Some(mut from), Some(last)) = (
            solstone_core_time::parse_day(&before_from),
            solstone_core_time::parse_day(&request.to),
        )
    {
        while from <= last {
            // A stretch reaching past the last representable day ends at `last`.
            let to = from
                .checked_add(Span::new().days(BODY_STRETCH_DAYS - 1))
                .map_or(last, |to| to.min(last));
            let (from_day, to_day) = (
                solstone_core_time::day_key(from),
                solstone_core_time::day_key(to),
            );
            for (_, facet_dir, _) in &facets_to_search {
                let groups = match solstone_core_location::body_records::body_groups_range(
                    journal_root,
                    facet_dir,
                    &from_day,
                    &to_day,
                    Some(&gate),
                    false,
                ) {
                    Ok(groups) => groups,
                    // A body record shows only as its face, so a list that would
                    // hold one can't go on without it; beside lines alone can.
                    Err(_)
                        if !body_rows.is_empty()
                            || (!has_person_or_entity && request.activity.is_empty()) =>
                    {
                        return Err(DispatchError::Tool(ToolError::FileUnreadable));
                    }
                    Err(_) => solstone_core_location::body_records::Groups::default(),
                };
                let mut on = from;
                while on <= to {
                    let day = solstone_core_time::day_key(on);
                    for group in groups.on(&day) {
                        if let Some(mut item) = body_rows.remove(&(
                            facet_dir.clone(),
                            day.clone(),
                            group.anchor_id.clone(),
                        )) {
                            item.group = Some((body.len(), group.clone()));
                            collected.push(item);
                        }
                    }
                    let Ok(next) = on.tomorrow() else {
                        break;
                    };
                    on = next;
                }
                body.push((facet_dir.clone(), groups));
            }
            if request.group_by.is_none() {
                let settled = collected
                    .iter()
                    .filter(|item| item.day <= to_day)
                    .filter(|item| {
                        cursor_bound
                            .as_ref()
                            .is_none_or(|cursor_key| &item.sort_key > cursor_key)
                    })
                    .count();
                if settled > request.limit {
                    break;
                }
            }
            let Ok(next) = to.tomorrow() else {
                break;
            };
            from = next;
        }
    }

    collected.sort_by(|a, b| a.sort_key.cmp(&b.sort_key));

    if let Some(ref dims) = request.group_by {
        let person_roles = effective_rollup_roles(&request);
        let group_roles = person_roles.unwrap_or(ActivityRole::DEFAULT_PERSON_GROUP);
        let groups_by_person = dims.contains(&GroupByDimension::Person);
        let mut buckets: BTreeMap<Vec<GroupKeyPart>, GroupBucket> = BTreeMap::new();
        let mut total_count = 0usize;
        let mut total_timed_count = 0usize;
        let mut total_duration_sec = 0u64;

        for item in &mut collected {
            let (duration_opt, kind_opt) = if let Some((stretch, group)) = &item.group {
                let face = body[*stretch].1.compose(journal_root, group);
                crate::body_source::name_windows(
                    &mut targets,
                    crate::body_source::face_windows(&face),
                );
                let face_val = solstone_core_location::body_records::face_json(&face);
                let dur = face_span(&face_val)
                    .and_then(|(start, end)| end.checked_sub(start))
                    .and_then(|diff| u64::try_from(diff).ok());
                let kind = face_val
                    .get("activity")
                    .and_then(Value::as_str)
                    .map(|k| k.trim().replace('_', " "))
                    .filter(|k| !k.is_empty());
                (dur, kind)
            } else {
                let (_, _, dur) = activity_times_and_duration(&item.record);
                (dur, activity_kind(&item.record))
            };
            let is_timed = duration_opt.is_some_and(|d| d > 0);
            let dur_sec = duration_opt.unwrap_or(0);
            total_count += 1;
            if is_timed {
                total_timed_count += 1;
            }
            total_duration_sec = total_duration_sec.saturating_add(dur_sec);

            let mut person_entries: Vec<(String, String)> = Vec::new();
            if groups_by_person {
                if item.group.is_none() {
                    let resolved = item.resolved_people.get_or_insert_with(|| {
                        resolve_activity_participants(
                            std::iter::once(&item.record).chain(item.copies.iter()),
                            Some(journal_root),
                            Some(&item.day),
                            Some(&mut photo_cache),
                            owner.as_ref(),
                        )
                    });
                    let mut seen_folded = BTreeSet::new();
                    for participant in &resolved.participants {
                        if !group_roles.contains(&participant.role) {
                            continue;
                        }
                        let folded = fold_literal(&participant.name);
                        if seen_folded.insert(folded.clone()) {
                            person_entries.push((folded, participant.name.clone()));
                        }
                    }
                }
                if person_entries.is_empty() {
                    continue;
                }
            }

            let add_to_bucket =
                |key: Vec<GroupKeyPart>,
                 person_display: Option<String>,
                 buckets: &mut BTreeMap<Vec<GroupKeyPart>, GroupBucket>| {
                    let bucket = buckets.entry(key).or_default();
                    bucket.count += 1;
                    if is_timed {
                        bucket.timed_count += 1;
                    }
                    bucket.duration_sec = bucket.duration_sec.saturating_add(dur_sec);
                    if bucket.person_display.is_none() {
                        bucket.person_display = person_display;
                    }
                };

            let build_key = |person_folded: Option<&str>| -> Vec<GroupKeyPart> {
                dims.iter()
                    .map(|dim| match dim {
                        GroupByDimension::Month => GroupKeyPart::Month(day_to_month(&item.day)),
                        GroupByDimension::Day => GroupKeyPart::Day(item.day.clone()),
                        GroupByDimension::Facet => GroupKeyPart::Facet {
                            title_folded: fold_literal(&item.facet_title),
                            id: item.facet_id.clone(),
                            title: item.facet_title.clone(),
                        },
                        GroupByDimension::Activity => GroupKeyPart::Activity(kind_opt.clone()),
                        GroupByDimension::Person => {
                            GroupKeyPart::Person(person_folded.unwrap_or_default().to_owned())
                        }
                    })
                    .collect()
            };

            if groups_by_person {
                for (folded, display) in person_entries {
                    add_to_bucket(build_key(Some(&folded)), Some(display), &mut buckets);
                }
            } else {
                add_to_bucket(build_key(None), None, &mut buckets);
            }
        }

        let groups_total = buckets.len();
        let truncated = groups_total > MAX_GROUP_BY_GROUPS;
        let groups: Vec<Value> = buckets
            .into_iter()
            .take(MAX_GROUP_BY_GROUPS)
            .map(|(key, bucket)| {
                let mut row = Map::new();
                for part in key {
                    match part {
                        GroupKeyPart::Month(month) => {
                            row.insert("month".to_owned(), json!(month));
                        }
                        GroupKeyPart::Day(day) => {
                            row.insert("day".to_owned(), json!(day));
                        }
                        GroupKeyPart::Facet { id, title, .. } => {
                            row.insert("facet".to_owned(), json!(title));
                            row.insert("facet_id".to_owned(), json!(id));
                        }
                        GroupKeyPart::Activity(kind) => {
                            row.insert("activity".to_owned(), json!(kind));
                        }
                        GroupKeyPart::Person(folded) => {
                            let label = bucket.person_display.as_deref().unwrap_or(&folded);
                            row.insert("person".to_owned(), json!(label));
                        }
                    }
                }
                row.insert("count".to_owned(), json!(bucket.count));
                row.insert("timed_count".to_owned(), json!(bucket.timed_count));
                row.insert("duration_sec".to_owned(), json!(bucket.duration_sec));
                Value::Object(row)
            })
            .collect();

        let count = groups.len();
        let mut response_val = json!({
            "group_by": dims.iter().map(|d| d.as_str()).collect::<Vec<_>>(),
            "groups": groups,
            "total": {
                "count": total_count,
                "timed_count": total_timed_count,
                "duration_sec": total_duration_sec,
            },
        });
        if let Some(roles) = person_roles {
            response_val["roles"] = json!(roles.iter().map(|r| r.as_str()).collect::<Vec<_>>());
        }
        if truncated {
            response_val["truncated"] = Value::Bool(true);
            response_val["groups_total"] = json!(groups_total);
        }
        if body_withheld {
            response_val["body_note"] = json!(crate::body_source::BODY_NOT_GRANTED);
            return Ok(Prepared::new(response_val, count, targets)
                .when_empty(crate::body_source::BODY_NOT_GRANTED));
        }
        let empty_note = if total_count > 0 {
            "no people matched those roles in these activities. this is a complete answer, not an error."
        } else {
            NO_ACTIVITIES_NOTE
        };
        return Ok(Prepared::new(response_val, count, targets).when_empty(empty_note));
    }

    let items_iter = collected.into_iter();
    let unskipped: Vec<CollectedItem> = if let Some(ref cursor_key) = cursor_bound {
        items_iter
            .filter(|item| &item.sort_key > cursor_key)
            .collect()
    } else {
        items_iter.collect()
    };

    let has_more = unskipped.len() > request.limit;
    let page: Vec<CollectedItem> = unskipped.into_iter().take(request.limit).collect();

    let sources = crate::dispatch::source_labels(journal_root);
    let mut activities = Vec::new();

    if request.compact {
        for item in &page {
            let reference = crate::dispatch::activity_reference(
                principal,
                snapshot,
                ActivityReference {
                    facet_id: item.facet_id.clone(),
                    facet_dir: item.facet_dir.clone(),
                    day: item.day.clone(),
                    record_identity: item.rec_identity.clone(),
                    file_digest: item.file_digest.clone(),
                    unit_index: None,
                    source_binding: None,
                },
            )?;
            if let Some((stretch, group)) = &item.group {
                let face = body[*stretch].1.compose(journal_root, group);
                crate::body_source::name_windows(
                    &mut targets,
                    crate::body_source::face_windows(&face),
                );
                let face_val = solstone_core_location::body_records::face_json(&face);
                let accounts = face_val.get("accounts").and_then(Value::as_array);
                let first = accounts.and_then(|arr| arr.first());
                let title = face_val
                    .get("title")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .or_else(|| {
                        first
                            .and_then(|f| f.get("label"))
                            .and_then(Value::as_str)
                            .map(str::to_owned)
                    })
                    .unwrap_or_default();
                let duration = face_span(&face_val)
                    .and_then(|(start, end)| end.checked_sub(start))
                    .and_then(|diff| u64::try_from(diff).ok());
                let mut activity = json!({
                    "title": title,
                    "facet": item.facet_title,
                    "day": item.day,
                    "content_kind": "body",
                    "source": "body",
                    "kind": face_val.get("activity").cloned().unwrap_or(Value::Null),
                    "start": first.and_then(|f| f.get("start")).cloned().unwrap_or(Value::Null),
                    "end": first.and_then(|f| f.get("end")).cloned().unwrap_or(Value::Null),
                    "duration": duration,
                    "reference": reference,
                });
                crate::journal_links::activity(
                    journal_root,
                    &mut activity,
                    &item.facet_dir,
                    &item.day,
                    &item.rec_identity,
                );
                activities.push(activity);
                continue;
            }
            let (start, _, duration) = activity_times_and_duration(&item.record);
            let title = activity_title(&item.record);
            let mut activity = json!({
                "title": title,
                "facet": item.facet_title,
                "day": item.day,
                "start": start,
                "duration": duration,
                "reference": reference,
            });
            crate::journal_links::activity(
                journal_root,
                &mut activity,
                &item.facet_dir,
                &item.day,
                &item.rec_identity,
            );
            if let Some(kind) = activity_kind(&item.record) {
                activity["kind"] = json!(kind);
            }
            if let Some(stream) = item.record.get("stream").and_then(Value::as_str) {
                activity["source"] = json!(crate::dispatch::source_label(&sources, stream));
            }
            if !item.also_in.is_empty() {
                activity["also_in"] = json!(item.also_in);
            }
            if sources_out_of_reach(
                journal_root,
                snapshot,
                &item.facet_dir,
                &item.day,
                &item.record,
            ) {
                activity["sources_unreadable"] = json!(true);
            }
            activities.push(activity);
        }
    } else {
        let owner_zone = solstone_core_journal_config::owner_zone(journal_root);
        let now = jiff::Timestamp::now().to_zoned_datetime(owner_zone.clone());
        let zone_str =
            solstone_core_journal_config::zone_label(Some(&owner_zone), now.offset().seconds());

        // The calendar and day location belong to no facet.
        let whole_journal = matches!(snapshot.scope, ConnectionScope::WholeJournal);
        let sees_calendar =
            whole_journal && snapshot.categories.contains(&AdmittedCategory::Calendar);
        let places_precision = snapshot.places_precision();
        let mut calendars = BTreeMap::new();
        let mut locations = BTreeMap::new();

        // Beside lines between body faces and capture activities, both ways, only
        // between items this connection reads. Worked out when read; never stored.
        let mut body_near: BTreeMap<String, Vec<BesideLine>> = BTreeMap::new();
        let mut capture_on: BTreeMap<String, Vec<BesideLine>> = BTreeMap::new();
        for item in &page {
            let reference = crate::dispatch::activity_reference(
                principal,
                snapshot,
                ActivityReference {
                    facet_id: item.facet_id.clone(),
                    facet_dir: item.facet_dir.clone(),
                    day: item.day.clone(),
                    record_identity: item.rec_identity.clone(),
                    file_digest: item.file_digest.clone(),
                    unit_index: None,
                    source_binding: None,
                },
            )?;

            let face = item.group.as_ref().map(|(stretch, group)| {
                let face = body[*stretch].1.compose(journal_root, group);
                crate::body_source::name_windows(
                    &mut targets,
                    crate::body_source::face_windows(&face),
                );
                (
                    solstone_core_location::body_records::face_json(&face),
                    solstone_core_location::body_records::face_still_there(&face),
                )
            });
            if let Some((face, there)) = &face {
                let mut activity =
                    body_activity(face, &item.facet_title, &item.day, reference, &zone_str);
                crate::journal_links::activity(
                    journal_root,
                    &mut activity,
                    &item.facet_dir,
                    &item.day,
                    &item.rec_identity,
                );
                if let Some(span) = face_span(face) {
                    let captures = capture_on.entry(item.day.clone()).or_insert_with(|| {
                        capture_lines_on(journal_root, &facets_to_search, &item.day)
                    });
                    // A record with nothing still there sits beside nothing, either way.
                    let beside = captures
                        .iter()
                        .filter(|_| *there)
                        .filter(|(other, _)| {
                            solstone_core_location::body_records::half_cover(span, *other)
                        })
                        .map(|(_, line)| line.clone())
                        .collect::<Vec<_>>();
                    if !beside.is_empty() {
                        activity["beside"] = Value::Array(beside);
                    }
                    // Where a device was at its middle, as that device timed it.
                    if let Some(precision) = places_precision
                        && let Some(beside) = solstone_core_location::place_at(
                            journal_root,
                            span.0 + (span.1 - span.0) / 2,
                            &mut locations,
                        )
                    {
                        activity["place"] =
                            crate::places_view::beside_place_json(&beside, precision);
                    }
                }
                activities.push(activity);
                continue;
            }
            let (start, end, duration) = activity_times_and_duration(&item.record);
            let people = match &item.resolved_people {
                Some(resolved) => resolved.to_activity_people(),
                None => activity_people_in_context(
                    std::iter::once(&item.record).chain(item.copies.iter()),
                    Some(journal_root),
                    Some(&item.day),
                    Some(&mut photo_cache),
                    owner.as_ref(),
                ),
            };
            let entities = activity_entities(&item.record);
            let title = activity_title(&item.record);

            let activity_content_kind =
                if solstone_core_format::content::is_strava_reading(&item.record) {
                    "body"
                } else {
                    "facets"
                };
            let mut activity = json!({
                "title": title,
                "facet": item.facet_title,
                "day": item.day,
                "content_kind": activity_content_kind,
                "start": start,
                "end": end,
                "duration": duration,
                "participants": people.participants,
                "heard": people.heard,
                "in_photo": people.in_photo,
                "mentioned": people.mentioned,
                "on_screen": people.on_screen,
                "entities": entities,
                "reference": reference,
                "zone": zone_str,
            });
            if !people.corresponded.is_empty() {
                activity["corresponded"] = json!(people.corresponded);
            }
            crate::journal_links::activity(
                journal_root,
                &mut activity,
                &item.facet_dir,
                &item.day,
                &item.rec_identity,
            );
            if let Some(kind) = activity_kind(&item.record) {
                activity["kind"] = json!(kind);
            }
            if let Some(stream) = item.record.get("stream").and_then(Value::as_str) {
                activity["source"] = json!(crate::dispatch::source_label(&sources, stream));
            }
            if people.unnamed_voices > 0 {
                activity["unnamed_voices"] = json!(people.unnamed_voices);
            }
            if !item.also_in.is_empty() {
                activity["also_in"] = json!(item.also_in);
            }
            if sources_out_of_reach(
                journal_root,
                snapshot,
                &item.facet_dir,
                &item.day,
                &item.record,
            ) {
                activity["sources_unreadable"] = json!(true);
            }
            if let Some(span) = solstone_core_facets::activity_span(&item.record) {
                if sees_calendar {
                    let calendar = calendars.entry(item.day.clone()).or_insert_with(|| {
                        solstone_core_facets::calendar_on_day(journal_root, &item.day)
                    });
                    let mut scheduled =
                        solstone_core_facets::calendar_beside(calendar, &item.day, span);
                    // Naming the people on an invite takes the people grant too.
                    if !snapshot.categories.contains(&AdmittedCategory::Entities) {
                        for event in &mut scheduled {
                            if let Some(event) = event.as_object_mut() {
                                event.remove("people");
                            }
                        }
                    }
                    if !scheduled.is_empty() {
                        activity["scheduled"] = Value::Array(scheduled);
                    }
                }
                if let Some(precision) = places_precision {
                    let location = locations.entry(item.day.clone()).or_insert_with(|| {
                        solstone_core_location::DayBeside::read(journal_root, &item.day)
                    });
                    let stream = item.record.get("stream").and_then(Value::as_str);
                    if let Some(beside) = location.beside(span, stream) {
                        activity["place"] =
                            crate::places_view::beside_place_json(&beside, precision);
                    }
                }
            }
            // A capture's span is worked out only when a body face could sit
            // beside it.
            let faces = body_near.entry(item.day.clone()).or_insert_with(|| {
                body_lines_near(journal_root, &mut body, &item.day, &mut targets)
            });
            if !faces.is_empty()
                && let Some(span) = solstone_core_location::body_records::capture_instants(
                    journal_root,
                    &item.day,
                    &item.record,
                )
            {
                let beside = faces
                    .iter()
                    .filter(|(other, _)| {
                        solstone_core_location::body_records::half_cover(span, *other)
                    })
                    .map(|(_, line)| line.clone())
                    .collect::<Vec<_>>();
                if !beside.is_empty() {
                    activity["beside"] = Value::Array(beside);
                }
            }
            activities.push(activity);
        }
    }

    let next_cursor = if has_more {
        if let Some(last) = page.last() {
            Some(
                codec()
                    .mint(
                        principal.connection,
                        snapshot.generation,
                        ReferenceTarget::ActivityList(ActivityListCursor {
                            day: request.day.clone(),
                            from: Some(request.from.clone()),
                            to: Some(request.to.clone()),
                            facet: request.facet.clone(),
                            entity: request.entity.clone(),
                            person: request.person.clone(),
                            roles: request.role_tokens(),
                            activity: request.activity.clone(),
                            compact: request.compact,
                            last_day: last.sort_key.0.clone(),
                            last_facet: last.sort_key.1.clone(),
                            last_created_at: last.sort_key.2,
                            last_id: last.sort_key.3.clone(),
                        }),
                    )
                    .map_err(|_| DispatchError::Tool(ToolError::ReferenceNotFound))?,
            )
        } else {
            None
        }
    } else {
        None
    };

    let count = activities.len();
    let mut response_val = json!({ "activities": activities });
    if let Some(next_cursor) = next_cursor {
        response_val["next_cursor"] = json!(next_cursor);
    }
    if body_withheld {
        response_val["body_note"] = json!(crate::body_source::BODY_NOT_GRANTED);
        return Ok(Prepared::new(response_val, count, targets)
            .when_empty(crate::body_source::BODY_NOT_GRANTED));
    }

    Ok(Prepared::new(response_val, count, targets).when_empty(NO_ACTIVITIES_NOTE))
}

/// An activity whose every segment sits outside a facet-limited connection's
/// facets says so on its row, so an agent doesn't ask get_activity_sources
/// for what it can never read. The same scope test that read makes.
fn sources_out_of_reach(
    journal_root: &Path,
    snapshot: &ConnectionReadSnapshot,
    facet_dir: &str,
    day: &str,
    record: &Map<String, Value>,
) -> bool {
    if matches!(snapshot.scope, ConnectionScope::WholeJournal) {
        return false;
    }
    let rel_path = format!("facets/{facet_dir}/activities/{day}.jsonl");
    let segments = activity_segment_candidates(journal_root, &rel_path, record);
    crate::may_see::segments_all_out_of_scope(journal_root, snapshot, day, &segments)
}

pub(crate) fn execute_get_sources(
    journal_root: &Path,
    principal: DispatchPrincipal<'_>,
    snapshot: &ConnectionReadSnapshot,
    request: ValidatedGetActivitySources,
) -> Result<Prepared, DispatchError> {
    // An activity's name, or a sealed reference from an earlier release.
    let address: ActivityAddress = if crate::names::is_name(&request.reference) {
        crate::names::parse_activity(&request.reference)
            .ok_or(DispatchError::Tool(ToolError::ReferenceNotFound))?
    } else {
        let ReferenceTarget::Activity(activity_ref) = codec()
            .resolve(
                &request.reference,
                ReferenceKind::Activity,
                principal.connection,
                snapshot.generation,
            )
            .map_err(|_| DispatchError::Tool(ToolError::ReferenceNotFound))?
        else {
            return Err(DispatchError::Tool(ToolError::ReferenceNotFound));
        };
        activity_ref.into()
    };
    sources_of(
        journal_root,
        principal,
        snapshot,
        &address,
        request.cursor.as_deref(),
    )
}

/// The verbatim sources of one activity, when this connection's own list
/// shows that activity now.
pub(crate) fn sources_of(
    journal_root: &Path,
    principal: DispatchPrincipal<'_>,
    snapshot: &ConnectionReadSnapshot,
    address: &ActivityAddress,
    cursor: Option<&str>,
) -> Result<Prepared, DispatchError> {
    // Hidden, superseded, out of scope and gone all read as not found.
    let seen = crate::may_see::activity_at(
        journal_root,
        snapshot,
        &address.facet,
        &address.day,
        &address.record_identity,
        address.file_digest.as_deref(),
    )
    .ok_or(DispatchError::Tool(ToolError::ReferenceNotFound))?;
    let place = seen.place;
    let file_digest = seen.file_digest;
    let rel_path = place.rel_path.clone();
    let record = match seen.record {
        // A body record's sources are its accounts, composed for this connection.
        crate::may_see::SeenRecord::Body(body) => {
            let face = crate::dispatch::body_face_for(
                journal_root,
                snapshot,
                &place.facet_dir,
                &place.day,
                &body,
            )?;
            // A hidden group is not found, as a missing one isn't: hidden by the
            // owner's hide of a member or of the day it shows under.
            if face.hidden {
                return Err(DispatchError::Tool(ToolError::ReferenceNotFound));
            }
            let item = solstone_core_location::body_records::face_json(&face);
            let mut units = item
                .get("accounts")
                .cloned()
                .unwrap_or(Value::Array(Vec::new()));
            if let Some(items) = units.as_array_mut() {
                for unit in items {
                    if let Some(obj) = unit.as_object_mut() {
                        obj.insert("content_kind".to_owned(), json!("body"));
                    }
                }
            }
            let mut targets = vec![rel_path];
            crate::body_source::name_windows(&mut targets, crate::body_source::face_windows(&face));
            return Ok(Prepared::new(json!({ "units": units }), 1, targets));
        }
        crate::may_see::SeenRecord::Activity(record) => record,
    };

    let segments = record.get("segments").and_then(Value::as_array);
    let Some(segment_vals) = segments else {
        return Ok(Prepared::new(json!({ "units": [] }), 0, vec![rel_path])
            .when_empty(crate::dispatch::ACTIVITY_SOURCES_EMPTY));
    };
    if segment_vals.is_empty() {
        return Ok(Prepared::new(json!({ "units": [] }), 0, vec![rel_path])
            .when_empty(crate::dispatch::ACTIVITY_SOURCES_EMPTY));
    }

    let activity_kind = record.get("activity").and_then(Value::as_str).unwrap_or("");

    // Each segment is read as what it is: a body window under body, any other
    // under transcripts.
    if crate::dispatch::segment_boundary(snapshot, None).is_none() {
        return Err(DispatchError::Tool(ToolError::ReferenceNotFound));
    }
    let hides = crate::body_source::BodyHides::read(journal_root);
    let mut admitted_transcripts = Vec::new();
    let mut any_unreadable = false;
    // A segment this connection may not read stays out of reach on every
    // retry, so it is never reported as a file that failed to read.
    let mut any_out_of_reach = false;
    let mut segment_link_map = std::collections::BTreeMap::new();
    let candidates = activity_segment_candidates(journal_root, &rel_path, &record);
    let mut ordered = Vec::new();
    for seg_val in segment_vals {
        let Some(key) = seg_val.as_str() else {
            any_unreadable = true;
            continue;
        };
        let matched: Vec<_> = candidates
            .iter()
            .filter(|segment| segment.name().to_str() == Some(key) || segment.key() == key)
            .cloned()
            .collect();
        if matched.is_empty() {
            any_unreadable = true;
            continue;
        }
        ordered.extend(matched);
    }

    // The same voice gate the activity's Story reads names who spoke.
    let speakers = solstone_core_talent_runtime::agent_speakers(journal_root);
    // Source units are the activity's own source, word for word: no possible
    // name is inserted into them. Links to a segment still bind the version
    // this connection's `get_transcript` reads, possible names included.
    let possible_names =
        crate::possible_names::ConnectionPossibleNames::new(journal_root, snapshot);
    for segment in ordered {
        let Ok(identity) = segment.record_identity() else {
            any_unreadable = true;
            continue;
        };
        let stream = identity.stream;
        let seg_key = identity.name;
        // Scope first, before anything of the segment is opened.
        match crate::may_see::segment_scope(journal_root, snapshot, &place.day, stream, seg_key) {
            Some(true) => {}
            Some(false) => {
                any_out_of_reach = true;
                continue;
            }
            None => {
                any_unreadable = true;
                continue;
            }
        }
        // A workout tile holds no transcript: nothing of it is a source.
        if solstone_core_segment::is_body_only_segment(&segment)
            && !crate::may_see::is_body_window(&segment)
        {
            continue;
        }
        // The same test a read of this segment alone makes.
        let Some(left_out) = crate::may_see::segment_shown(
            journal_root,
            snapshot,
            &hides,
            None,
            &place.day,
            stream,
            seg_key,
            &segment,
        ) else {
            any_out_of_reach = true;
            continue;
        };

        #[cfg(feature = "test-hooks")]
        crate::source_freeze_barrier::checkpoint(crate::source_freeze_barrier::FreezePoint::Before);
        let allowed_before = crate::source_release::check_segment_transcript_release(
            journal_root,
            &place.day,
            stream,
            seg_key,
        );
        if !allowed_before {
            #[cfg(feature = "test-hooks")]
            crate::source_freeze_barrier::checkpoint(
                crate::source_freeze_barrier::FreezePoint::After,
            );
            any_unreadable = true;
            continue;
        }

        let mut segment_lines = Vec::new();
        let mut page_cursor = None;
        let mut read_failed = false;
        let mut segment_untimed = false;
        let mut segment_version = None;

        loop {
            match crate::body_source::read_body_aware_page(
                &segment,
                page_cursor.as_ref(),
                speakers.as_ref(),
                None,
                &left_out,
                &snapshot.categories,
                snapshot.places_precision(),
            ) {
                Ok(page) => {
                    segment_untimed = page.untimed;
                    segment_version = Some(page.version.fingerprint());
                    for (entry, content_kind) in page.entries_with_content_kind() {
                        segment_lines.push(SourcedLine {
                            part: entry.turn_part.clone(),
                            channel: entry.channel,
                            content_kind,
                            text: entry.text.clone(),
                        });
                    }
                    if page.next.is_some() {
                        page_cursor = page.next;
                    } else {
                        break;
                    }
                }
                Err(solstone_core_transcripts::SegmentTranscriptReadError::NoPermittedFiles) => {
                    // A withheld sibling contributes no unit or read failure.
                    segment_lines.clear();
                    break;
                }
                Err(_) => {
                    read_failed = true;
                    break;
                }
            }
        }

        let allowed_after = crate::source_release::check_segment_transcript_release(
            journal_root,
            &place.day,
            stream,
            seg_key,
        );
        #[cfg(feature = "test-hooks")]
        crate::source_freeze_barrier::checkpoint(crate::source_freeze_barrier::FreezePoint::After);

        if !allowed_after || read_failed {
            any_unreadable = true;
            continue;
        }

        if segment_lines.is_empty() {
            continue;
        }

        let timestamp = if segment_untimed {
            None
        } else {
            match format_segment_timestamp(&place.day, seg_key) {
                Some(timestamp) => Some(timestamp),
                None => {
                    any_unreadable = true;
                    continue;
                }
            }
        };
        let Some(version) = segment_version else {
            any_unreadable = true;
            continue;
        };

        let channels = group_segment_channels(&segment_lines);
        let mut dummy_record = record.clone();
        dummy_record.insert("segments".to_string(), json!([seg_key]));
        // This candidate's identity, so a streamless record's other matches
        // do not take the link.
        dummy_record.insert("stream".to_string(), json!(stream));
        let mut single_subject = [LinkSubject {
            path: &rel_path,
            idx: 0,
            text: "",
            segments: Vec::new(),
            more_segments: false,
            activity_record: Some(&dummy_record),
        }];
        let mut checker = LiveTranscriptPages::with_speakers(
            speakers.clone(),
            std::sync::Arc::clone(&possible_names),
            snapshot,
        );
        let mut response_checks = 0;
        issue_segment_links(
            journal_root,
            principal,
            snapshot,
            &mut single_subject,
            48,
            48,
            &mut response_checks,
            &mut checker,
        );
        if let Some(link) = single_subject[0].segments.first().cloned() {
            segment_link_map.insert(admitted_transcripts.len(), link);
        }
        admitted_transcripts.push(AdmittedTranscript {
            version,
            timestamp,
            channels,
            untimed: segment_untimed,
        });
    }

    if admitted_transcripts.is_empty() {
        if any_unreadable {
            return Err(DispatchError::Tool(ToolError::FileUnreadable));
        } else if any_out_of_reach {
            let target = format!(
                "facet:{}/activity:{}",
                place.facet_id, address.record_identity
            );
            return Ok(
                Prepared::new(json!({ "units": [], "unreadable": true }), 0, vec![target])
                    .when_empty(crate::dispatch::ACTIVITY_SOURCES_OUT_OF_REACH),
            );
        } else {
            let target = format!(
                "facet:{}/activity:{}",
                place.facet_id, address.record_identity
            );
            return Ok(Prepared::new(json!({ "units": [] }), 0, vec![target])
                .when_empty(crate::dispatch::ACTIVITY_SOURCES_EMPTY));
        }
    }

    let mut all_units = Vec::new();
    for (index, admitted) in admitted_transcripts.iter().enumerate() {
        let channels = admitted
            .channels
            .iter()
            .map(
                |channel| solstone_core_talent_runtime::source_passages::SourceChannel {
                    channel: channel.channel,
                    text: &channel.text,
                },
            )
            .collect::<Vec<_>>();
        for (channel_index, mut unit) in
            solstone_core_talent_runtime::source_passages::source_units_from_channels(
                activity_kind,
                admitted.timestamp.as_deref(),
                &channels,
            )
        {
            // The rendered text keeps the native projection, including a
            // synthetic [00:00:00] on an untimed chat line. The page flag is
            // the qualifier that clock is not a captured time.
            unit.untimed = admitted.untimed;
            let content_kind = admitted.channels[channel_index].content_kind;
            all_units.push((index, content_kind, unit));
        }
    }
    let source_binding = admitted_source_binding(&admitted_transcripts);

    let start_unit = if let Some(cursor_token) = cursor {
        let ReferenceTarget::Activity(c) = codec()
            .resolve(
                cursor_token,
                ReferenceKind::Activity,
                principal.connection,
                snapshot.generation,
            )
            .map_err(|_| DispatchError::Tool(ToolError::ReferenceNotFound))?
        else {
            return Err(DispatchError::Tool(ToolError::ReferenceNotFound));
        };
        // A cursor keeps its pins: the same activity, in a day file and with
        // sources that haven't changed since the page before.
        if c.facet_id != place.facet_id
            || c.day != place.day
            || c.record_identity != address.record_identity
            || c.file_digest != file_digest
            || c.source_binding.as_deref() != Some(source_binding.as_str())
        {
            return Err(DispatchError::Tool(ToolError::ReferenceNotFound));
        }
        c.unit_index.unwrap_or(0)
    } else if address.unit_index.is_some()
        && address.source_binding.as_deref() != Some(source_binding.as_str())
    {
        return Err(DispatchError::Tool(ToolError::ReferenceNotFound));
    } else {
        address.unit_index.unwrap_or(0)
    };

    let mut page_units = Vec::new();
    let mut page_bytes = 0usize;
    let mut next_unit_index = None;

    for (idx, (segment_index, content_kind, unit)) in all_units.iter().enumerate().skip(start_unit)
    {
        let unit_bytes = unit.text.len();
        if !page_units.is_empty()
            && (page_units.len() >= MAX_TRANSCRIPT_PAGE_ITEMS
                || page_bytes + unit_bytes > MAX_TRANSCRIPT_PAGE_BYTES)
        {
            next_unit_index = Some(idx);
            break;
        }

        let source_link = segment_link_map.get(segment_index).cloned();

        let mut unit_json = json!({
            "text": unit.text,
            "kind": unit.kind.as_str(),
        });
        if let Some(kind) = content_kind {
            unit_json["content_kind"] = json!(kind);
        } else {
            unit_json["conversation_state"] = json!("unresolved");
        }
        if let Some(ref ts) = unit.timestamp {
            unit_json["timestamp"] = json!(ts);
        }
        if unit.untimed {
            unit_json["untimed"] = json!(true);
        }
        if let Some(link) = source_link {
            unit_json["source_link"] = link;
        }
        page_units.push(unit_json);
        page_bytes += unit_bytes;
    }

    let next_cursor = if let Some(next_idx) = next_unit_index {
        Some(
            codec()
                .mint(
                    principal.connection,
                    snapshot.generation,
                    ReferenceTarget::Activity(ActivityReference {
                        facet_id: place.facet_id.clone(),
                        facet_dir: place.facet_dir.clone(),
                        day: place.day.clone(),
                        record_identity: address.record_identity.clone(),
                        file_digest: file_digest.clone(),
                        unit_index: Some(next_idx),
                        source_binding: Some(source_binding.clone()),
                    }),
                )
                .map_err(|_| DispatchError::Tool(ToolError::ReferenceNotFound))?,
        )
    } else {
        None
    };

    let count = page_units.len();
    let mut response_val = json!({ "units": page_units });
    if any_unreadable || any_out_of_reach {
        response_val["unreadable"] = Value::Bool(true);
    }
    if let Some(next_cursor) = next_cursor {
        response_val["next_cursor"] = json!(next_cursor);
    }

    let target = format!(
        "facet:{}/activity:{}",
        place.facet_id, address.record_identity
    );
    Ok(Prepared::new(response_val, count, vec![target])
        .when_empty(crate::dispatch::ACTIVITY_SOURCES_EMPTY))
}

struct SourcedLine {
    part: Option<solstone_core_transcripts::TurnPart>,
    channel: &'static str,
    content_kind: Option<&'static str>,
    text: String,
}

struct AdmittedChannel {
    channel: &'static str,
    content_kind: Option<&'static str>,
    text: String,
}

struct AdmittedTranscript {
    version: u64,
    timestamp: Option<String>,
    channels: Vec<AdmittedChannel>,
    untimed: bool,
}

fn admitted_source_binding(admitted: &[AdmittedTranscript]) -> String {
    let mut hasher = Sha256::new();
    for source in admitted {
        hasher.update(source.version.to_le_bytes());
        hasher.update([u8::from(source.untimed)]);
        if let Some(ref timestamp) = source.timestamp {
            hasher.update((timestamp.len() as u64).to_le_bytes());
            hasher.update(timestamp.as_bytes());
        } else {
            hasher.update(u64::MAX.to_le_bytes());
        }
        hasher.update((source.channels.len() as u64).to_le_bytes());
        for channel in &source.channels {
            hasher.update((channel.channel.len() as u64).to_le_bytes());
            hasher.update(channel.channel.as_bytes());
            let kind = channel.content_kind.unwrap_or_default();
            hasher.update((kind.len() as u64).to_le_bytes());
            hasher.update(kind.as_bytes());
            hasher.update((channel.text.len() as u64).to_le_bytes());
            hasher.update(channel.text.as_bytes());
        }
    }
    crate::activity_admit::hex_encode(&hasher.finalize())
}

/// One segment's admitted lines, grouped by the channel and registry content
/// kind the page reader assigned to each file. Carrying these boundaries
/// directly keeps lines of journal text from being re-parsed as channel or
/// segment headings.
fn group_segment_channels(lines: &[SourcedLine]) -> Vec<AdmittedChannel> {
    let mut channels = Vec::new();
    let mut index = 0;
    while index < lines.len() {
        let channel = lines[index].channel;
        let content_kind = lines[index].content_kind;
        let mut text = String::new();
        let start = index;
        let mut notice = None;
        while index < lines.len()
            && lines[index].channel == channel
            && lines[index].content_kind == content_kind
        {
            let line = &lines[index];
            let part = line.part.as_ref();
            let continuation = index > start
                && part.is_some_and(|p| {
                    p.byte_offset > 0
                        && lines[index - 1].part.as_ref().is_some_and(|previous| {
                            !previous.complete && previous.turn_id == p.turn_id
                        })
                });
            if index > start && !continuation {
                text.push('\n');
            }
            text.push_str(&line.text);
            if let Some(part) = part {
                if part.byte_offset == 0 {
                    notice = part.source_notice.as_deref();
                }
                if part.complete
                    && let Some(suffix) = notice.take()
                {
                    text.push('\n');
                    text.push_str(suffix);
                }
            }
            index += 1;
        }
        channels.push(AdmittedChannel {
            channel,
            content_kind,
            text,
        });
    }
    channels
}

fn format_segment_timestamp(day: &str, seg_key: &str) -> Option<String> {
    let times = solstone_core_format::segment::segment_parse(seg_key)?;
    let length = seg_key.split_once('_')?.1.parse::<i64>().ok()?;
    let date = solstone_core_time::parse_day(day)
        .or_else(|| jiff::civil::Date::strptime("%Y-%m-%d", day).ok())?;
    let start = date.to_datetime(
        jiff::civil::Time::new(
            i8::try_from(times.hour).ok()?,
            i8::try_from(times.minute).ok()?,
            i8::try_from(times.second).ok()?,
            0,
        )
        .ok()?,
    );
    // Only the end's time of day is shown, so it wraps past midnight.
    let end = start
        .time()
        .wrapping_add(jiff::SignedDuration::from_secs(length));
    Some(format!(
        "{} - {}",
        start.strftime("%Y-%m-%d %H:%M:%S"),
        end.strftime("%H:%M:%S")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn activity_channels_join_one_turn_verbatim_and_keep_its_source_notice() {
        let line = |text: &str, id: &str, offset, complete, cap: Option<&str>| SourcedLine {
            channel: "Imported Messages",
            content_kind: Some("messages"),
            text: text.into(),
            part: Some(solstone_core_transcripts::TurnPart {
                turn_id: id.into(),
                byte_offset: offset,
                complete,
                source_notice: cap.map(str::to_owned),
            }),
        };
        let lines = [
            line("booksh", "a", 0, false, Some("partial input")),
            line("elf", "a", 6, true, None),
            line("next", "b", 0, true, None),
        ];
        assert_eq!(
            group_segment_channels(&lines)[0].text,
            "bookshelf\npartial input\nnext"
        );
    }

    #[test]
    fn an_agent_never_reads_what_the_owner_retracted_or_wrote_over() {
        let record = |edits: Value| {
            serde_json::json!({"id":"a","description":"kept","edits":edits})
                .as_object()
                .unwrap()
                .clone()
        };
        let cleaned = clean_activity_record_for_wire(record(serde_json::json!([
            {"timestamp":"t1","actor":"owner:update","fields":["description"],"set":{"description":"retracted"},"before":{"description":"old"},"removed":"t3"},
            {"timestamp":"t2","actor":"owner:update","fields":["description"],"set":{"description":"kept"},"before":{"description":"old"}},
            {"timestamp":"t4","actor":"owner:lines","fields":[],"note":"","lines":[{"stream":"mic","segment":"120000_300","source":"audio","sentence_id":1,"set_at":"s","prior":null}],"speaker":{"kind":"unknown"}},
        ])));
        assert_eq!(
            Value::Object(cleaned),
            Value::Object(record(serde_json::json!([
                {"timestamp":"t2","actor":"owner:update","fields":["description"]},
                {"timestamp":"t4","actor":"owner:lines","fields":[],"note":""},
            ])))
        );
        let plain = record(serde_json::json!([
            {"timestamp":"t0","actor":"cli:create","fields":["title"],"note":"created"},
        ]));
        assert_eq!(clean_activity_record_for_wire(plain.clone()), plain);
    }

    #[test]
    fn activity_heard_and_role_classification() {
        let record = json!({
            "participation": [
                {"name": "Alice", "role": "attendee"},
                {"name": "Bob", "role": "heard"},
                {"name": "Carol", "role": "mentioned"},
                {"name": "Dave", "role": "visible"},
                {"name": "Eve", "role": "organizer"},
                {"name": "Frank"},
            ]
        });
        let people = activity_people(record.as_object().unwrap());
        assert_eq!(people.participants, vec!["Alice"]);
        assert_eq!(people.heard, vec!["Bob"]);
        assert!(people.in_photo.is_empty());
        assert_eq!(people.mentioned, vec!["Carol", "Eve", "Frank"]);
        assert_eq!(people.on_screen, vec!["Dave"]);
    }

    #[test]
    fn real_photos_split_into_in_photo_while_screenshots_and_decoys_stay_on_screen() {
        let dir = tempfile::Builder::new()
            .prefix("solstone-photo-visible-")
            .tempdir_in(crate::test_scratch())
            .unwrap();
        let root = dir.path();

        // Owner identity with an alias to verify entity_id precedence.
        let owner_dir = root.join("entities/owner");
        std::fs::create_dir_all(&owner_dir).unwrap();
        std::fs::write(
            owner_dir.join("entity.json"),
            json!({
                "id": "owner_id",
                "name": "William Shakespeare",
                "type": "Person",
                "aka": ["Will"],
                "is_principal": true
            })
            .to_string(),
        )
        .unwrap();

        // Real photo segment: image_transcript.md only.
        let photo_seg = root.join("chronicle/20250501/import.image/120000_0");
        std::fs::create_dir_all(&photo_seg).unwrap();
        std::fs::write(photo_seg.join("image_transcript.md"), "Garden photo").unwrap();

        // Imported screenshot segment: screenshot_transcript.md only.
        let shot_seg = root.join("chronicle/20250501/import.image/130000_0");
        std::fs::create_dir_all(&shot_seg).unwrap();
        std::fs::write(shot_seg.join("screenshot_transcript.md"), "Desktop UI").unwrap();

        // Ambiguous segment carrying both fails closed to on_screen.
        let both_seg = root.join("chronicle/20250501/import.image/140000_0");
        std::fs::create_dir_all(&both_seg).unwrap();
        std::fs::write(both_seg.join("image_transcript.md"), "Photo").unwrap();
        std::fs::write(both_seg.join("screenshot_transcript.md"), "Shot").unwrap();

        let mut cache = BTreeMap::new();
        let owner = crate::people_view::OwnerIdentity::load(root);

        let photo_rec = json!({
            "id": "moment_01",
            "stream": "import.image",
            "segments": ["120000_0"],
            "participation": [
                {"name": "Anne", "entity_id": "anne", "role": "visible"},
                {"name": "William Shakespeare", "entity_id": "owner_id", "role": "visible"},
                {"name": "Will", "entity_id": "other_will", "role": "visible"},
                {"name": "Will", "role": "visible"},
                {"name": "Carol", "role": "mentioned"},
            ]
        });
        let p_photo = activity_people_in_context(
            std::iter::once(photo_rec.as_object().unwrap()),
            Some(root),
            Some("20250501"),
            Some(&mut cache),
            Some(&owner),
        );
        assert_eq!(p_photo.in_photo, vec!["Anne", "Will"]);
        assert!(p_photo.on_screen.is_empty());
        assert_eq!(p_photo.mentioned, vec!["Carol"]);

        let shot_rec = json!({
            "id": "photo_999",
            "stream": "import.image",
            "segments": ["130000_0"],
            "participation": [{"name": "Screen User", "role": "visible"}]
        });
        let p_shot = activity_people_in_context(
            std::iter::once(shot_rec.as_object().unwrap()),
            Some(root),
            Some("20250501"),
            Some(&mut cache),
            None,
        );
        assert!(p_shot.in_photo.is_empty());
        assert_eq!(p_shot.on_screen, vec!["Screen User"]);

        let both_rec = json!({
            "id": "photo_both",
            "stream": "import.image",
            "segments": ["140000_0"],
            "participation": [{"name": "Ambiguous", "role": "visible"}]
        });
        let p_both = activity_people_in_context(
            std::iter::once(both_rec.as_object().unwrap()),
            Some(root),
            Some("20250501"),
            Some(&mut cache),
            None,
        );
        assert!(p_both.in_photo.is_empty());
        assert_eq!(p_both.on_screen, vec!["Ambiguous"]);

        // Non-photo stream with a synthetic "photo_" id must remain on_screen.
        let decoy_rec = json!({
            "id": "photo_decoy_01",
            "stream": "desk",
            "segments": ["120000_0"],
            "participation": [{"name": "Desk Face", "role": "visible"}]
        });
        let p_decoy = activity_people_in_context(
            std::iter::once(decoy_rec.as_object().unwrap()),
            Some(root),
            Some("20250501"),
            Some(&mut cache),
            None,
        );
        assert!(p_decoy.in_photo.is_empty());
        assert_eq!(p_decoy.on_screen, vec!["Desk Face"]);

        // Attendee outranks in_photo across merged copies.
        let attendee_copy = json!({
            "id": "moment_01_b",
            "stream": "talk",
            "segments": ["120000_0"],
            "participation": [{"name": "Anne", "role": "attendee"}]
        });
        let p_merged = activity_people_in_context(
            [
                photo_rec.as_object().unwrap(),
                attendee_copy.as_object().unwrap(),
            ]
            .into_iter(),
            Some(root),
            Some("20250501"),
            Some(&mut cache),
            Some(&owner),
        );
        assert_eq!(p_merged.participants, vec!["Anne"]);
        assert_eq!(p_merged.in_photo, vec!["Will"]);
    }

    #[test]
    fn validate_list_accepts_and_rejects_role_activity_group_by_and_compact_combinations() {
        let zone = solstone_core_time::zone("UTC").expect("UTC parses");

        let ok_roles = validate_list(
            Some(&json!({
                "day_from": "2025-01-01",
                "day_to": "2025-12-31",
                "person": "Ada Lovelace",
                "roles": ["heard", "attendee", "heard"],
                "activity": ["Reading", "deep_work"],
                "compact": true,
                "limit": 100
            })),
            &zone,
        )
        .expect("valid native filters");
        assert_eq!(ok_roles.person.as_deref(), Some("Ada Lovelace"));
        assert_eq!(
            ok_roles.roles,
            Some(vec![ActivityRole::Attendee, ActivityRole::Heard])
        );
        assert_eq!(
            ok_roles.activity,
            vec!["deep work".to_owned(), "reading".to_owned()]
        );
        assert!(ok_roles.compact);
        assert_eq!(ok_roles.limit, 100);

        let ok_rollup = validate_list(
            Some(&json!({
                "day_from": "20250101",
                "day_to": "20251231",
                "group_by": ["month", "facet"]
            })),
            &zone,
        )
        .expect("valid group_by");
        assert_eq!(
            ok_rollup.group_by,
            Some(vec![GroupByDimension::Month, GroupByDimension::Facet])
        );

        let ok_person_rollup = validate_list(
            Some(&json!({
                "day_from": "20250101",
                "day_to": "20251231",
                "group_by": ["person"],
                "roles": ["in_photo"]
            })),
            &zone,
        )
        .expect("roles allowed with group_by person");
        assert_eq!(ok_person_rollup.roles, Some(vec![ActivityRole::InPhoto]));

        for bad in [
            json!({"day": "20250101", "roles": ["attendee"]}),
            json!({"day": "20250101", "person": "Ada", "roles": []}),
            json!({"day": "20250101", "person": "   "}),
            json!({"day": "20250101", "activity": ""}),
            json!({"day": "20250101", "activity": []}),
            json!({"day": "20250101", "group_by": []}),
            json!({"day": "20250101", "group_by": ["month", "month"]}),
            json!({"day": "20250101", "group_by": ["month"], "limit": 10}),
            json!({"day": "20250101", "group_by": ["month"], "cursor": "abc"}),
            json!({"day": "20250101", "group_by": ["month"], "compact": true}),
            json!({"day": "20250101", "group_by": ["month"], "roles": ["attendee"]}),
            json!({"day": "20250101", "kind": "photos"}),
            json!({"day": "20250101", "place": "London"}),
        ] {
            assert_eq!(
                validate_list(Some(&bad), &zone),
                Err(ToolError::InvalidInput),
                "{bad}"
            );
        }
    }
}
