// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Deterministic pre-filter, `m5` subset collapse, storyless fallback,
//! corroborated `Person` extraction, and natural channel-token humanization.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use jiff::Span;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use solstone_core_entity::{
    hidden_participation_entity_ids, live_merge_successor, load_all_journal_entities,
};
use solstone_core_facets::{
    ExperienceRecord, list_declared_facet_names, load_activity_records, load_experiences,
    read_facet_declaration,
};
use solstone_core_format::segment::{segment_key, segment_start_and_end_seconds};
use solstone_core_segment::{effective_activity_stream, live_segment_dir};

use crate::JournalOwner;

/// Journal owner identity used for third-person framing and self-exclusion.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OwnerIdentity {
    pub full_name: String,
    pub preferred: String,
    pub entity_id: String,
    pub aliases: Vec<String>,
}

impl OwnerIdentity {
    /// Load owner identity from `journal.json` and the principal `Person` entity.
    pub fn load(journal: &Path) -> Self {
        let cfg = solstone_core_journal_config::read_journal_config(journal)
            .ok()
            .and_then(|read| read.config)
            .unwrap_or_default();
        let ident = cfg.get("identity").and_then(Value::as_object);
        let principal = JournalOwner::load(journal).ok();

        let cfg_name = ident
            .and_then(|o| o.get("name"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned);
        let cfg_preferred = ident
            .and_then(|o| o.get("preferred"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned);
        let cfg_entity = ident
            .and_then(|o| o.get("entity"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned);

        let full_name = cfg_name
            .or_else(|| principal.as_ref().and_then(|p| p.name.clone()))
            .or_else(|| cfg_preferred.clone())
            .unwrap_or_else(|| "Owner".to_owned());
        let preferred = cfg_preferred
            .or_else(|| full_name.split_whitespace().next().map(str::to_owned))
            .unwrap_or_else(|| "Owner".to_owned());
        let entity_id = cfg_entity
            .or_else(|| principal.as_ref().and_then(|p| p.id.clone()))
            .unwrap_or_default();

        let mut aliases = BTreeSet::new();
        for raw in [full_name.as_str(), preferred.as_str()] {
            let trimmed = raw.trim().to_lowercase();
            if !trimmed.is_empty() {
                aliases.insert(trimmed);
            }
        }
        if let Some(first) = full_name.split_whitespace().next() {
            let low = first.trim().to_lowercase();
            if !low.is_empty() {
                aliases.insert(low);
            }
        }
        if let Some(extra) = ident
            .and_then(|o| o.get("aliases"))
            .and_then(Value::as_array)
        {
            for item in extra {
                if let Some(s) = item.as_str() {
                    let low = s.trim().to_lowercase();
                    if !low.is_empty() {
                        aliases.insert(low);
                    }
                }
            }
        }
        Self {
            full_name,
            preferred,
            entity_id,
            aliases: aliases.into_iter().collect(),
        }
    }

    /// Return whether an entity ID or display name refers to the journal owner.
    pub fn is_owner(&self, entity_id: &str, name: &str) -> bool {
        if !self.entity_id.is_empty() && entity_id == self.entity_id {
            return true;
        }
        let low = name.trim().to_lowercase();
        !low.is_empty() && self.aliases.iter().any(|a| a == &low)
    }
}

/// Active (unmuted) facet metadata.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActiveFacetInfo {
    pub slug: String,
    pub id: String,
    pub description: String,
}

/// Corroborated `Person` entity extracted from activity participation rows.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CorroboratedPerson {
    pub entity_id: String,
    pub name: String,
    pub roles: Vec<String>,
    pub sources: Vec<String>,
    pub mentions: usize,
}

/// Open late-evening Experience from `day - 1` eligible for cross-midnight continuation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenPrevExperience {
    pub id: String,
    pub primary_facet: String,
    pub title: String,
    pub started_at: String,
    pub ended_at: String,
}

/// One post-filtered, `m5`-deduped activity candidate in the day pool.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PoolItem {
    pub handle: String,
    pub day: String,
    pub is_prev_evening: bool,
    pub facet: String,
    pub stream: String,
    pub id: String,
    pub activity: String,
    pub start_s: i64,
    pub end_s: i64,
    pub start_hm: String,
    pub end_hm: String,
    pub duration_min: i64,
    pub segments: Vec<String>,
    pub has_story: bool,
    pub has_media: bool,
    pub playback: Option<bool>,
    pub cross_facet_twins: Vec<String>,
    pub people: Vec<String>,
    pub corroborated_entity_ids: Vec<String>,
    pub location_hints: Vec<String>,
    pub topics: Vec<String>,
    pub description: String,
    pub summary: String,
}

/// Multi-facet post-filtered activity pool for one target day.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DayPool {
    pub day: String,
    pub prev_day: String,
    pub owner: OwnerIdentity,
    pub facets: BTreeMap<String, ActiveFacetInfo>,
    pub facet_prefixes: BTreeMap<String, String>,
    pub items: Vec<PoolItem>,
    pub excluded_keys: BTreeSet<(String, String, String, String)>,
    pub open_prev_experiences: Vec<OpenPrevExperience>,
}

impl DayPool {
    pub fn find_by_handle(&self, handle: &str) -> Option<&PoolItem> {
        self.items.iter().find(|it| it.handle == handle)
    }
}

/// Humanize internal stream identifiers into natural channel words so internal
/// storage/pipeline tokens never leak into Experience prompts or prose.
pub fn humanize_stream_label(stream: &str) -> String {
    let low = stream.trim().to_lowercase();
    if low.is_empty() {
        return "activity".to_owned();
    }
    if low == "obsidian" || low.contains("obsidian") || low.contains("notes") {
        return "notes".to_owned();
    }
    if low == "import.image" || low.contains("image") || low.contains("photo") {
        return "photo".to_owned();
    }
    if low == "ios_screenshots" || low.contains("screenshot") {
        return "screenshot".to_owned();
    }
    if low.contains("tmux") || low.contains("terminal") {
        return "terminal".to_owned();
    }
    if low == "desk" || low == "screen" || low.contains("desktop") || low.contains("screen") {
        return "screen".to_owned();
    }
    if low == "ios_mobile" || low.contains("mobile") || low.contains("phone") {
        return "mobile".to_owned();
    }
    if low == "watch"
        || low.contains("watch-audio")
        || low.contains("audio")
        || low == "talk"
        || low == "mic"
        || low.contains("voice")
    {
        return "audio".to_owned();
    }
    if low.contains("browser") || low.contains("web") {
        return "browser".to_owned();
    }
    if low.contains("message")
        || low.contains("imessage")
        || low.contains("sms")
        || low.contains("chat")
        || low.contains("slack")
        || low.contains("discord")
        || low.contains("whatsapp")
        || low.contains("mail")
        || low.contains("email")
    {
        return "messages".to_owned();
    }
    if low.contains("document") || low.contains("import.doc") || low.contains("file") {
        return "document".to_owned();
    }
    "activity".to_owned()
}

/// Return whether a stream is a secondary desktop/screen/terminal channel that
/// can bleed unrelated desktop text into concurrent live audio/call episodes.
pub fn is_secondary_screen_stream(stream: &str) -> bool {
    let low = stream.trim().to_lowercase();
    low == "desk"
        || low == "screen"
        || low.contains("tmux")
        || low.contains("terminal")
        || low.contains("desktop")
        || low.contains("screen_text")
        || (low.contains("screen") && !low.contains("screenshot"))
}

/// Return whether a stream or activity represents a live audio/call/meeting channel.
pub fn is_live_audio_or_call(stream: &str, activity_type: &str) -> bool {
    let sl = stream.trim().to_lowercase();
    let al = activity_type.trim().to_lowercase();
    sl == "watch"
        || sl.contains("watch-audio")
        || sl == "talk"
        || sl == "mic"
        || sl.contains("audio")
        || matches!(al.as_str(), "meeting" | "call" | "conversation")
}

/// Return `day - 1` as `YYYYMMDD`.
pub fn prev_day_key(day: &str) -> Option<String> {
    let parsed = solstone_core_time::parse_day(day)?;
    let prev = parsed.checked_sub(Span::new().days(1)).ok()?;
    Some(solstone_core_time::day_key(prev))
}

/// Format `YYYYMMDD` as `YYYY-MM-DD`.
pub fn format_iso_day(day: &str) -> String {
    if day.len() == 8 && day.bytes().all(|b| b.is_ascii_digit()) {
        format!("{}-{}-{}", &day[0..4], &day[4..6], &day[6..8])
    } else {
        day.to_owned()
    }
}

/// Format seconds-since-midnight as `HH:MM`.
pub fn format_hm(secs: i64) -> String {
    let norm = secs.rem_euclid(86_400);
    let hh = norm / 3600;
    let mm = (norm % 3600) / 60;
    format!("{hh:02}:{mm:02}")
}

/// Parse one segment key `HHMMSS_LEN` into `(start_secs, end_secs)`.
pub fn parse_seg_bounds(seg: &str) -> Option<(i64, i64)> {
    let key = segment_key(seg)?;
    let (times, end_secs) = segment_start_and_end_seconds(&key)?;
    let start_secs =
        i64::from(times.hour) * 3600 + i64::from(times.minute) * 60 + i64::from(times.second);
    Some((start_secs, i64::try_from(end_secs).ok()?))
}

/// Compute `(min_start_secs, max_end_secs)` for an activity's segments, handling
/// within-activity midnight wraps (e.g., `23:55` -> `00:05`).
pub fn activity_segments_bounds(segments: &[String]) -> Option<(i64, i64)> {
    let pairs: Vec<(i64, i64)> = segments
        .iter()
        .filter_map(|s| parse_seg_bounds(s))
        .collect();
    if pairs.is_empty() {
        return None;
    }
    let has_late = pairs.iter().any(|(s, _)| *s >= 20 * 3600);
    let has_early = pairs.iter().any(|(s, _)| *s < 4 * 3600);
    if has_late && has_early {
        let mut min_s = i64::MAX;
        let mut max_e = i64::MIN;
        for (s, e) in pairs {
            let ns = if s < 4 * 3600 { s + 86_400 } else { s };
            let ne = if s < 4 * 3600 { e + 86_400 } else { e };
            min_s = min_s.min(ns);
            max_e = max_e.max(ne);
        }
        return Some((min_s, max_e));
    }
    let min_s = pairs.iter().map(|(s, _)| *s).min()?;
    let max_e = pairs.iter().map(|(_, e)| *e).max()?;
    Some((min_s, max_e))
}

/// Bounded segment-level artifacts read from a live chronicle segment directory.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SegmentArtifacts {
    pub summary: String,
    pub facet_activity: String,
    pub topics: Vec<String>,
    pub image_caption: String,
    pub has_media: bool,
}

/// Read bounded facet-scoped segment summary, topics, and image caption from chronicle.
pub fn read_segment_artifacts(
    journal: &Path,
    day: &str,
    stream: &str,
    seg: &str,
    facet: &str,
) -> SegmentArtifacts {
    let mut out = SegmentArtifacts::default();
    let Some(sdir) = live_segment_dir(journal, day, stream, seg) else {
        if matches!(stream, "import.image" | "ios_screenshots") {
            out.has_media = true;
        }
        return out;
    };
    let tdir = sdir.join("talents");
    let sense_p = tdir.join("sense.json");
    if let Ok(text) = fs::read_to_string(&sense_p)
        && let Ok(sj) = serde_json::from_str::<Value>(&text)
    {
        if let Some(fmap) = sj.get("facets").and_then(Value::as_object) {
            if let Some(fa) = fmap
                .get(facet)
                .and_then(|v| v.get("activity"))
                .and_then(Value::as_str)
            {
                out.facet_activity = fa.trim().to_owned();
            }
        } else if let Some(farr) = sj.get("facets").and_then(Value::as_array) {
            for item in farr {
                if item.get("facet").and_then(Value::as_str) == Some(facet)
                    && let Some(act) = item.get("activity").and_then(Value::as_str)
                {
                    out.facet_activity = act.trim().to_owned();
                    break;
                }
            }
        }
        if let Some(summary) = sj.get("activity_summary").and_then(Value::as_str) {
            out.summary = summary.trim().to_owned();
        }
        if let Some(topics) = sj.get("topics").and_then(Value::as_array) {
            out.topics = topics
                .iter()
                .filter_map(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .take(8)
                .map(str::to_owned)
                .collect();
        }
    }
    if out.summary.is_empty()
        && let Ok(text) = fs::read_to_string(tdir.join("activity.md"))
    {
        out.summary = text.trim().to_owned();
    }
    if out.topics.is_empty()
        && let Ok(text) = fs::read_to_string(tdir.join("topics.md"))
    {
        let mut in_topics = true;
        for line in text.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("## ") {
                in_topics = trimmed.to_lowercase().contains("topic");
                continue;
            }
            if in_topics && let Some(rest) = trimmed.strip_prefix("- ") {
                let t = rest.trim();
                if !t.is_empty() {
                    out.topics.push(t.to_owned());
                    if out.topics.len() >= 8 {
                        break;
                    }
                }
            }
        }
    }
    let img_p = sdir.join("image_transcript.md");
    if let Ok(text) = fs::read_to_string(&img_p) {
        let trimmed = text.trim();
        let cap_lines: Vec<String> = trimmed
            .lines()
            .filter(|ln| {
                let s = ln.trim();
                s.starts_with('>') && !s.contains("[image description")
            })
            .map(|ln| ln.trim().trim_start_matches('>').trim().to_owned())
            .filter(|s| !s.is_empty())
            .collect();
        let joined = cap_lines
            .join(" ")
            .replace("\\,", ",")
            .replace("\\-", "-")
            .replace("\\'", "'");
        let caption = if joined.trim().is_empty() {
            truncate_chars(trimmed, 320)
        } else {
            joined.trim().to_owned()
        };
        out.image_caption = caption;
        out.has_media = true;
    }
    if sdir.join("original.jpg").is_file()
        || sdir.join("original.png").is_file()
        || matches!(stream, "import.image" | "ios_screenshots")
    {
        out.has_media = true;
    }
    out
}

/// Deterministic live-activity filter (excludes hidden, idle, gaming, body, anticipated, synthetic, empty segments).
pub fn is_live_activity(row: &Map<String, Value>) -> bool {
    if row
        .get("hidden")
        .is_some_and(solstone_core_facets::activity_value_truthy)
    {
        return false;
    }
    let act = row.get("activity").and_then(Value::as_str).unwrap_or("");
    if matches!(act, "idle" | "gaming") {
        return false;
    }
    let source = row.get("source").and_then(Value::as_str).unwrap_or("");
    if matches!(source, "body" | "anticipated") {
        return false;
    }
    if row
        .get("synthetic")
        .is_some_and(solstone_core_facets::activity_value_truthy)
    {
        return false;
    }
    let segs = row.get("segments").and_then(Value::as_array);
    segs.is_some_and(|arr| !arr.is_empty())
}

/// Extract clean segment strings from an activity row.
pub fn row_segments(row: &Map<String, Value>) -> Vec<String> {
    row.get("segments")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

/// Return trimmed `story.body` when present.
pub fn row_story_body(row: &Map<String, Value>) -> String {
    row.get("story")
        .and_then(Value::as_object)
        .and_then(|s| s.get("body"))
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("")
        .to_owned()
}

/// Return the effective stream name for an activity row, resolving legacy
/// single-stream fallback when needed.
pub(crate) fn row_stream(journal: &Path, day: &str, row: &Map<String, Value>) -> String {
    effective_activity_stream(journal, day, row)
        .or_else(|| row.get("stream").and_then(Value::as_str))
        .unwrap_or("")
        .to_owned()
}

/// Deterministically collapse strict same-facet, same-stream segment-subset duplicates (`m5`).
pub fn collapse_m5_subsets(
    journal: &Path,
    day: &str,
    rows: Vec<Map<String, Value>>,
) -> Vec<Map<String, Value>> {
    let mut by_stream: BTreeMap<String, Vec<Map<String, Value>>> = BTreeMap::new();
    for row in rows {
        let stream = row_stream(journal, day, &row);
        by_stream.entry(stream).or_default().push(row);
    }
    let mut kept = Vec::new();
    for (_stream, mut group) in by_stream {
        group.sort_by(|a, b| {
            let seg_cmp = row_segments(b).len().cmp(&row_segments(a).len());
            let story_a = usize::from(row_story_body(a).is_empty());
            let story_b = usize::from(row_story_body(b).is_empty());
            let id_a = a.get("id").and_then(Value::as_str).unwrap_or("");
            let id_b = b.get("id").and_then(Value::as_str).unwrap_or("");
            seg_cmp
                .then_with(|| story_a.cmp(&story_b))
                .then_with(|| id_a.cmp(id_b))
        });
        let mut kept_sets: Vec<BTreeSet<String>> = Vec::new();
        for row in group {
            let rset: BTreeSet<String> = row_segments(&row).into_iter().collect();
            if rset.is_empty() {
                continue;
            }
            if kept_sets.iter().any(|kset| rset.is_subset(kset)) {
                continue;
            }
            kept_sets.push(rset);
            kept.push(row);
        }
    }
    kept.sort_by(|a, b| {
        let sa = activity_segments_bounds(&row_segments(a)).map_or(i64::MAX, |(s, _)| s);
        let sb = activity_segments_bounds(&row_segments(b)).map_or(i64::MAX, |(s, _)| s);
        let id_a = a.get("id").and_then(Value::as_str).unwrap_or("");
        let id_b = b.get("id").and_then(Value::as_str).unwrap_or("");
        sa.cmp(&sb).then_with(|| id_a.cmp(id_b))
    });
    kept
}

/// Load enabled (unmuted) facets.
pub fn load_active_facets(
    journal: &Path,
    facets_filter: Option<&[String]>,
) -> Result<BTreeMap<String, ActiveFacetInfo>, String> {
    let names = list_declared_facet_names(journal).map_err(|e| e.to_string())?;
    let filter_set: Option<BTreeSet<&str>> =
        facets_filter.map(|list| list.iter().map(String::as_str).collect());
    let mut out = BTreeMap::new();
    for slug in names {
        if let Some(ref set) = filter_set
            && !set.contains(slug.as_str())
        {
            continue;
        }
        let Ok(Some(decl)) = read_facet_declaration(journal, &slug) else {
            continue;
        };
        if decl.muted == Some(true) {
            continue;
        }
        let id = decl
            .value()
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        let desc = if !decl.description.trim().is_empty() {
            decl.description.trim().to_owned()
        } else if !decl.title.trim().is_empty() {
            decl.title.trim().to_owned()
        } else {
            slug.clone()
        };
        out.insert(
            slug.clone(),
            ActiveFacetInfo {
                slug,
                id,
                description: desc,
            },
        );
    }
    Ok(out)
}

/// Entity index for fast `Person` validation and canonical name lookup.
pub struct PersonIndex {
    hidden: BTreeSet<String>,
    persons: BTreeMap<String, String>,
}

impl PersonIndex {
    pub fn load(journal: &Path) -> Self {
        let hidden = hidden_participation_entity_ids(journal);
        let mut persons = BTreeMap::new();
        for ent in load_all_journal_entities(journal).unwrap_or_default() {
            if ent.is_blocked() || ent.is_principal() || hidden.contains(&ent.id) {
                continue;
            }
            if ent
                .entity_type()
                .is_some_and(|t| t.eq_ignore_ascii_case("Person"))
            {
                let name = ent
                    .value
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .trim()
                    .to_owned();
                persons.insert(ent.id, name);
            }
        }
        Self { hidden, persons }
    }

    pub fn resolve_person(&self, journal: &Path, raw_id: &str) -> Option<(String, String)> {
        let trimmed = raw_id.trim();
        if trimmed.is_empty() || self.hidden.contains(trimmed) {
            return None;
        }
        let resolved = live_merge_successor(journal, trimmed).unwrap_or_else(|| trimmed.to_owned());
        if self.hidden.contains(&resolved) {
            return None;
        }
        let name = self.persons.get(&resolved)?;
        Some((resolved, name.clone()))
    }
}

fn is_speaker_or_unknown_name(name: &str) -> bool {
    let low = name.trim().to_lowercase();
    if low.is_empty()
        || low.starts_with("unidentified")
        || low.starts_with("unknown")
        || low == "your agent"
    {
        return true;
    }
    if let Some(rest) = low.strip_prefix("speaker") {
        let rest = rest.trim();
        if !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit()) {
            return true;
        }
    }
    false
}

/// Extract corroborated `Person` entities from a set of activity rows (owner excluded).
pub fn extract_corroborated_people(
    journal: &Path,
    rows: &[(String, Map<String, Value>)],
    owner: &OwnerIdentity,
    persons: &PersonIndex,
) -> Vec<CorroboratedPerson> {
    struct Acc {
        name: String,
        roles: BTreeSet<String>,
        sources: BTreeSet<String>,
        mentions: usize,
    }
    let mut by_eid: BTreeMap<String, Acc> = BTreeMap::new();
    for (day, row) in rows {
        let act_type = row
            .get("activity")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_lowercase();
        let stream = row_stream(journal, day, row).to_lowercase();
        let story_text = format!(
            "{} {}",
            row_story_body(row),
            row.get("description").and_then(Value::as_str).unwrap_or("")
        )
        .to_lowercase();
        let Some(parts) = row.get("participation").and_then(Value::as_array) else {
            continue;
        };
        for pt in parts {
            let Some(obj) = pt.as_object() else {
                continue;
            };
            let raw_eid = obj
                .get("entity_id")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim();
            let raw_name = obj.get("name").and_then(Value::as_str).unwrap_or("").trim();
            let role = obj
                .get("role")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_lowercase();
            let src = obj
                .get("source")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_lowercase();
            if raw_eid.is_empty() || raw_name.is_empty() {
                continue;
            }
            if owner.is_owner(raw_eid, raw_name) || is_speaker_or_unknown_name(raw_name) {
                continue;
            }
            let Some((resolved_eid, canonical_name)) = persons.resolve_person(journal, raw_eid)
            else {
                continue;
            };
            if owner.is_owner(&resolved_eid, &canonical_name) {
                continue;
            }
            if !matches!(role.as_str(), "attendee" | "corresponded" | "heard") {
                continue;
            }
            if src == "screen" {
                if !matches!(
                    act_type.as_str(),
                    "meeting" | "social" | "call" | "conversation"
                ) {
                    continue;
                }
                if stream.contains("tmux") || stream == "desk" {
                    continue;
                }
                let low_name = raw_name.to_lowercase();
                let last_tok = low_name.split_whitespace().last().unwrap_or("");
                if !story_text.contains(&low_name)
                    && (last_tok.is_empty() || !story_text.contains(last_tok))
                    && !stream.contains("mobile")
                {
                    continue;
                }
            }
            let display_name = if canonical_name.is_empty() {
                raw_name.to_owned()
            } else {
                canonical_name
            };
            let entry = by_eid.entry(resolved_eid).or_insert_with(|| Acc {
                name: display_name,
                roles: BTreeSet::new(),
                sources: BTreeSet::new(),
                mentions: 0,
            });
            entry.roles.insert(role);
            entry.sources.insert(if src.is_empty() {
                "unknown".to_owned()
            } else {
                src
            });
            entry.mentions += 1;
        }
    }
    by_eid
        .into_iter()
        .map(|(entity_id, acc)| CorroboratedPerson {
            entity_id,
            name: acc.name,
            roles: acc.roles.into_iter().collect(),
            sources: acc.sources.into_iter().collect(),
            mentions: acc.mentions,
        })
        .collect()
}

/// Extract compact summary, topics, and `has_media` flag handling `story: null` cleanly.
pub fn extract_row_fallback_summary(
    journal: &Path,
    row: &Map<String, Value>,
    day: &str,
    facet: &str,
    max_chars: usize,
) -> (String, Vec<String>, bool) {
    let story_body = row_story_body(row);
    let mut topics = Vec::new();
    if let Some(arr) = row
        .get("story")
        .and_then(Value::as_object)
        .and_then(|s| s.get("topics"))
        .and_then(Value::as_array)
    {
        for v in arr {
            if let Some(t) = v.as_str().map(str::trim).filter(|s| !s.is_empty())
                && !topics.iter().any(|existing: &String| existing == t)
            {
                topics.push(t.to_owned());
            }
        }
    }
    if let Some(arr) = row.get("topics").and_then(Value::as_array) {
        for v in arr {
            if let Some(t) = v.as_str().map(str::trim).filter(|s| !s.is_empty())
                && !topics.iter().any(|existing: &String| existing == t)
            {
                topics.push(t.to_owned());
            }
        }
    }

    let stream = row_stream(journal, day, row);
    let segs = row_segments(row);
    let mut has_media = matches!(stream.as_str(), "import.image" | "ios_screenshots");

    if !story_body.is_empty() {
        let collapsed = collapse_whitespace(&story_body);
        if !has_media
            && let Some(first_seg) = segs.first()
            && let Some(sdir) = live_segment_dir(journal, day, &stream, first_seg)
            && (sdir.join("image_transcript.md").is_file() || sdir.join("original.jpg").is_file())
        {
            has_media = true;
        }
        topics.truncate(6);
        return (truncate_chars(&collapsed, max_chars), topics, has_media);
    }

    // Storyless fallback: description, details, sense.facets[f].activity, topics, image_transcript.md
    let mut parts = Vec::new();
    let desc = row
        .get("description")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("");
    if !desc.is_empty() {
        parts.push(desc.to_owned());
    }
    let details = row
        .get("details")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("");
    if !details.is_empty() && details != desc {
        parts.push(details.to_owned());
    }

    if let Some(first_seg) = segs.first() {
        let seg_art = read_segment_artifacts(journal, day, &stream, first_seg, facet);
        if seg_art.has_media {
            has_media = true;
        }
        if !seg_art.image_caption.is_empty() {
            parts.push(format!("[image: {}]", seg_art.image_caption));
        } else if !seg_art.facet_activity.is_empty()
            && !parts.iter().any(|p| p == &seg_art.facet_activity)
        {
            parts.push(seg_art.facet_activity);
        } else if !seg_art.summary.is_empty() && !parts.iter().any(|p| p == &seg_art.summary) {
            parts.push(seg_art.summary);
        }
        for t in seg_art.topics {
            if !topics.iter().any(|existing| existing == &t) {
                topics.push(t);
            }
        }
    }

    let mut text = collapse_whitespace(&parts.join(" | "));
    if text.is_empty() {
        let act = row
            .get("activity")
            .and_then(Value::as_str)
            .unwrap_or("activity");
        text = format!("({act} via {}, no story)", humanize_stream_label(&stream));
    }
    topics.truncate(6);
    (
        format!("[storyless] {}", truncate_chars(&text, max_chars)),
        topics,
        has_media,
    )
}

type ExcludedActivityKey = (String, String, String, String);

fn collect_existing_exclusions(
    journal: &Path,
    facets: &BTreeMap<String, ActiveFacetInfo>,
    day: &str,
    prev_day: &str,
) -> (BTreeSet<ExcludedActivityKey>, Vec<OpenPrevExperience>) {
    let mut excluded = BTreeSet::new();
    let mut open_prev = Vec::new();
    for facet in facets.keys() {
        for check_day in [prev_day, day] {
            let existing = load_experiences(journal, facet, check_day, true).unwrap_or_default();
            for exp in existing {
                // Respect owner-excluded activities unconditionally.
                for exc in &exp.owner_holds.excluded_activities {
                    let ef = exc.effective_facet(&exp.primary_facet).to_owned();
                    excluded.insert((exc.day.clone(), ef, exc.stream.clone(), exc.id.clone()));
                }
                // Respect muted tombstones (`hidden: true`) AND owner-locked membership
                // so neither muted Experiences nor owner-edited memberships get re-detected.
                if exp.hidden || exp.owner_holds.activities {
                    for act in &exp.activities {
                        let af = act.effective_facet(&exp.primary_facet).to_owned();
                        excluded.insert((act.day.clone(), af, act.stream.clone(), act.id.clone()));
                    }
                }
                // Expose non-hidden late-evening previous-day Experiences for cross-midnight continuation.
                if check_day == prev_day && !exp.hidden && is_late_evening_experience(&exp) {
                    open_prev.push(OpenPrevExperience {
                        id: exp.id,
                        primary_facet: exp.primary_facet,
                        title: exp.title,
                        started_at: exp.started_at,
                        ended_at: exp.ended_at,
                    });
                }
            }
        }
    }
    open_prev.sort_by(|a, b| a.started_at.cmp(&b.started_at).then(a.id.cmp(&b.id)));
    (excluded, open_prev)
}

fn is_late_evening_experience(exp: &ExperienceRecord) -> bool {
    let time_part = exp.ended_at.split(['T', ' ']).nth(1).unwrap_or("");
    let hour: u32 = time_part
        .get(0..2)
        .and_then(|h| h.parse().ok())
        .unwrap_or(0);
    !(3..20).contains(&hour)
}

fn assign_facet_prefixes(facets: &BTreeMap<String, ActiveFacetInfo>) -> BTreeMap<String, String> {
    let mut used = BTreeSet::new();
    let mut prefixes = BTreeMap::new();
    for fname in facets.keys() {
        let clean: String = fname
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .map(|c| c.to_ascii_uppercase())
            .collect();
        let mut chosen = None;
        let mut cands = Vec::new();
        if clean.len() >= 2 {
            cands.push(clean[0..2].to_owned());
            let first = clean.chars().next().unwrap();
            let last = clean.chars().last().unwrap();
            cands.push(format!("{first}{last}"));
        } else if !clean.is_empty() {
            cands.push(clean.clone());
        }
        if clean.len() >= 3 {
            cands.push(clean[0..3].to_owned());
        }
        for cand in cands {
            if used.insert(cand.clone()) {
                chosen = Some(cand);
                break;
            }
        }
        if chosen.is_none() {
            for i in 1..100 {
                let cand = format!("F{i}");
                if used.insert(cand.clone()) {
                    chosen = Some(cand);
                    break;
                }
            }
        }
        prefixes.insert(fname.clone(), chosen.unwrap_or_else(|| "FX".to_owned()));
    }
    prefixes
}

fn extract_location_hints(row: &Map<String, Value>) -> Vec<String> {
    let mut out = Vec::new();
    for key in ["location", "place", "venue"] {
        if let Some(s) = row.get(key).and_then(Value::as_str) {
            let t = s.trim();
            if !t.is_empty() && !out.iter().any(|e: &String| e.eq_ignore_ascii_case(t)) {
                out.push(t.to_owned());
            }
        }
    }
    if let Some(arr) = row.get("locations").and_then(Value::as_array) {
        for v in arr {
            if let Some(s) = v.as_str() {
                let t = s.trim();
                if !t.is_empty() && !out.iter().any(|e| e.eq_ignore_ascii_case(t)) {
                    out.push(t.to_owned());
                }
            }
        }
    }
    out
}

/// Build the deterministic multi-facet daily activity pool for `day` (plus `day - 1 >= 20:00`).
pub fn load_day_multi_facet_pool(
    journal: &Path,
    day: &str,
    facets_filter: Option<&[String]>,
) -> Result<DayPool, String> {
    if day.len() != 8 || solstone_core_time::parse_day(day).is_none() {
        return Err(format!("invalid day key: {day}"));
    }
    let prev_day =
        prev_day_key(day).ok_or_else(|| format!("cannot compute previous day for {day}"))?;
    let facets = load_active_facets(journal, facets_filter)?;
    let owner = OwnerIdentity::load(journal);
    let persons = PersonIndex::load(journal);
    let (excluded_keys, open_prev_experiences) =
        collect_existing_exclusions(journal, &facets, day, &prev_day);

    struct RawCandidate {
        day: String,
        is_prev_evening: bool,
        facet: String,
        row: Map<String, Value>,
    }

    let mut kept_all: Vec<RawCandidate> = Vec::new();
    for fname in facets.keys() {
        // Current day
        let raw_cur = load_activity_records(journal, fname, day, true).unwrap_or_default();
        let live_cur: Vec<Map<String, Value>> = raw_cur
            .into_iter()
            .filter(|r| {
                if !is_live_activity(r) {
                    return false;
                }
                let stream = row_stream(journal, day, r);
                let id = r.get("id").and_then(Value::as_str).unwrap_or("");
                !excluded_keys.contains(&(day.to_owned(), fname.clone(), stream, id.to_owned()))
            })
            .collect();
        for row in collapse_m5_subsets(journal, day, live_cur) {
            kept_all.push(RawCandidate {
                day: day.to_owned(),
                is_prev_evening: false,
                facet: fname.clone(),
                row,
            });
        }

        // Previous day >= 20:00 lookback
        let raw_prev = load_activity_records(journal, fname, &prev_day, true).unwrap_or_default();
        let live_prev: Vec<Map<String, Value>> = raw_prev
            .into_iter()
            .filter(|r| {
                if !is_live_activity(r) {
                    return false;
                }
                let segs = row_segments(r);
                let Some((_, end_s)) = activity_segments_bounds(&segs) else {
                    return false;
                };
                if end_s < 20 * 3600 {
                    return false;
                }
                let stream = row_stream(journal, &prev_day, r);
                let id = r.get("id").and_then(Value::as_str).unwrap_or("");
                !excluded_keys.contains(&(prev_day.clone(), fname.clone(), stream, id.to_owned()))
            })
            .collect();
        for row in collapse_m5_subsets(journal, &prev_day, live_prev) {
            kept_all.push(RawCandidate {
                day: prev_day.clone(),
                is_prev_evening: true,
                facet: fname.clone(),
                row,
            });
        }
    }

    // Cross-facet segment map for A1 twin tagging
    let mut seg_to_facets: BTreeMap<(String, String, String), BTreeSet<String>> = BTreeMap::new();
    for cand in &kept_all {
        let stream = row_stream(journal, &cand.day, &cand.row);
        for seg in row_segments(&cand.row) {
            seg_to_facets
                .entry((cand.day.clone(), stream.clone(), seg))
                .or_default()
                .insert(cand.facet.clone());
        }
    }

    kept_all.sort_by(|a, b| {
        let prev_cmp = b.is_prev_evening.cmp(&a.is_prev_evening);
        let sa = activity_segments_bounds(&row_segments(&a.row)).map_or(i64::MAX, |(s, _)| s);
        let sb = activity_segments_bounds(&row_segments(&b.row)).map_or(i64::MAX, |(s, _)| s);
        let id_a = a.row.get("id").and_then(Value::as_str).unwrap_or("");
        let id_b = b.row.get("id").and_then(Value::as_str).unwrap_or("");
        prev_cmp
            .then_with(|| sa.cmp(&sb))
            .then_with(|| a.facet.cmp(&b.facet))
            .then_with(|| id_a.cmp(id_b))
    });

    let facet_prefixes = assign_facet_prefixes(&facets);
    let mut facet_counters: BTreeMap<String, usize> =
        facets.keys().map(|f| (f.clone(), 0)).collect();
    let mut items = Vec::with_capacity(kept_all.len());

    for cand in kept_all {
        let counter = facet_counters.entry(cand.facet.clone()).or_insert(0);
        *counter += 1;
        let pfx = facet_prefixes
            .get(&cand.facet)
            .map(String::as_str)
            .unwrap_or("FX");
        let handle = format!("{pfx}{counter:02}");
        let segs = row_segments(&cand.row);
        let Some((start_s, end_s)) = activity_segments_bounds(&segs) else {
            continue;
        };
        let duration_min = ((end_s - start_s) as f64 / 60.0).round().max(0.0) as i64;
        let stream = row_stream(journal, &cand.day, &cand.row);
        let id = cand
            .row
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        let activity = cand
            .row
            .get("activity")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        let description = cand
            .row
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_owned();
        let has_story = !row_story_body(&cand.row).is_empty();
        let playback = cand.row.get("playback").and_then(Value::as_bool);
        let (summary, topics, has_media) =
            extract_row_fallback_summary(journal, &cand.row, &cand.day, &cand.facet, 260);

        let mut people_labels = Vec::new();
        if let Some(parts) = cand.row.get("participation").and_then(Value::as_array) {
            for pt in parts {
                let Some(obj) = pt.as_object() else {
                    continue;
                };
                let nm = obj.get("name").and_then(Value::as_str).unwrap_or("").trim();
                let eid = obj
                    .get("entity_id")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .trim();
                let role = obj
                    .get("role")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_lowercase();
                if nm.is_empty()
                    || owner.is_owner(eid, nm)
                    || is_speaker_or_unknown_name(nm)
                    || !matches!(role.as_str(), "attendee" | "corresponded" | "heard")
                {
                    continue;
                }
                let tag = if persons.resolve_person(journal, eid).is_some() {
                    ""
                } else {
                    "?"
                };
                let lbl = format!("{nm}{tag}");
                if !people_labels.iter().any(|existing| existing == &lbl) {
                    people_labels.push(lbl);
                    if people_labels.len() >= 6 {
                        break;
                    }
                }
            }
        }
        let corroborated_entity_ids = extract_corroborated_people(
            journal,
            &[(cand.day.clone(), cand.row.clone())],
            &owner,
            &persons,
        )
        .into_iter()
        .map(|p| p.entity_id)
        .collect();

        let mut other_facets = BTreeSet::new();
        for seg in &segs {
            if let Some(fset) = seg_to_facets.get(&(cand.day.clone(), stream.clone(), seg.clone()))
            {
                for f in fset {
                    if f != &cand.facet {
                        other_facets.insert(f.clone());
                    }
                }
            }
        }

        items.push(PoolItem {
            handle,
            day: cand.day,
            is_prev_evening: cand.is_prev_evening,
            facet: cand.facet,
            stream,
            id,
            activity,
            start_s,
            end_s,
            start_hm: format_hm(start_s),
            end_hm: format_hm(end_s),
            duration_min,
            segments: segs,
            has_story,
            has_media,
            playback,
            cross_facet_twins: other_facets.into_iter().collect(),
            people: people_labels,
            corroborated_entity_ids,
            location_hints: extract_location_hints(&cand.row),
            topics: topics.into_iter().take(5).collect(),
            description,
            summary,
        });
    }

    Ok(DayPool {
        day: day.to_owned(),
        prev_day,
        owner,
        facets,
        facet_prefixes,
        items,
        excluded_keys,
        open_prev_experiences,
    })
}

pub(crate) fn collapse_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub(crate) fn truncate_chars(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_owned();
    }
    let keep = max_chars.saturating_sub(1);
    let prefix: String = text.chars().take(keep).collect();
    format!("{}…", prefix.trim_end())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn m5_collapse_keeps_superset_and_drops_strict_subset() {
        let big = json!({
            "id": "a_big",
            "stream": "watch",
            "activity": "social",
            "segments": ["180000_300", "180500_300", "181000_300"],
            "story": {"body": "Full dinner."}
        })
        .as_object()
        .unwrap()
        .clone();
        let sub = json!({
            "id": "a_sub",
            "stream": "watch",
            "activity": "social",
            "segments": ["180500_300"],
            "story": {"body": "Snippet."}
        })
        .as_object()
        .unwrap()
        .clone();
        let tmp = tempfile::tempdir().unwrap();
        let kept = collapse_m5_subsets(tmp.path(), "20260304", vec![sub, big]);
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].get("id").and_then(Value::as_str), Some("a_big"));
    }

    #[test]
    fn humanize_stream_maps_internal_tokens_cleanly() {
        assert_eq!(humanize_stream_label("obsidian"), "notes");
        assert_eq!(humanize_stream_label("import.image"), "photo");
        assert_eq!(humanize_stream_label("ios_screenshots"), "screenshot");
        assert_eq!(humanize_stream_label("tmux"), "terminal");
        assert_eq!(humanize_stream_label("watch-audio"), "audio");
        assert_eq!(humanize_stream_label("ios_mobile"), "mobile");
    }
}
