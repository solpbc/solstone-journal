// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Stored Experience records, lightweight internal references, owner-hold
//! predicates, share-detail policy projection types, and search chunk rendering.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::{IndexChunk, JsonObject, ProducedChunks, json_truthy};

/// Lightweight internal reference to a constituent journal activity.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ActivityRef {
    /// Local journal day key (`YYYYMMDD`).
    pub day: String,
    /// Activity stream identifier (for example `device_watch-audio` or `_default`).
    pub stream: String,
    /// Activity record identifier within `(facet, day)`.
    pub id: String,
    /// Owning facet slug when explicitly added from a sibling facet.
    /// When absent, resolves against the Experience's `primary_facet`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub facet: Option<String>,
}

impl ActivityRef {
    /// Effective facet slug for this activity reference.
    #[must_use]
    pub fn effective_facet<'a>(&'a self, primary_facet: &'a str) -> &'a str {
        self.facet
            .as_deref()
            .filter(|value| !value.is_empty())
            .unwrap_or(primary_facet)
    }

    /// Canonical tuple key for deduplication and exclusion checks.
    #[must_use]
    pub fn key<'a>(&'a self, primary_facet: &'a str) -> (&'a str, &'a str, &'a str, &'a str) {
        (
            self.day.as_str(),
            self.effective_facet(primary_facet),
            self.stream.as_str(),
            self.id.as_str(),
        )
    }
}

/// How an Experience's activity membership was established.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MembershipSource {
    /// Automatically assembled by daily experience detection (single primary facet).
    #[default]
    Auto,
    /// Created or structurally edited by the owner (may include explicit cross-facet activities).
    Owner,
}

/// Per-field and per-moment owner-hold locks that future automated passes never overwrite.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OwnerHolds {
    /// Locks `activities` membership against automated additions or removals.
    #[serde(default)]
    pub activities: bool,
    /// Locks `title` against automated regeneration.
    #[serde(default)]
    pub title: bool,
    /// Locks `summary` against automated regeneration.
    #[serde(default)]
    pub summary: bool,
    /// Locks individual `moments[].id` entries against automated regeneration.
    #[serde(default)]
    pub moment_ids: BTreeSet<String>,
    /// Activities explicitly removed by the owner so future detection never re-adds them.
    #[serde(default)]
    pub excluded_activities: Vec<ActivityRef>,
}

impl OwnerHolds {
    /// Whether `ref_item` was explicitly excluded from this Experience by the owner.
    #[must_use]
    pub fn excludes_activity(&self, primary_facet: &str, candidate: &ActivityRef) -> bool {
        let key = candidate.key(primary_facet);
        self.excluded_activities
            .iter()
            .any(|excluded| excluded.key(primary_facet) == key)
    }

    /// Record an excluded activity reference idempotently.
    pub fn add_excluded(&mut self, primary_facet: &str, removed: ActivityRef) {
        if !self.excludes_activity(primary_facet, &removed) {
            self.excluded_activities.push(removed);
        }
    }

    /// Remove an activity reference from exclusions when explicitly re-added by the owner.
    pub fn clear_excluded(&mut self, primary_facet: &str, added: &ActivityRef) {
        let key = added.key(primary_facet);
        self.excluded_activities
            .retain(|excluded| excluded.key(primary_facet) != key);
    }
}

/// One chronological narrative moment within an Experience (many-to-many with `activities`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExperienceMoment {
    /// Stable moment identifier within the Experience (for example `m1`, `m2`).
    pub id: String,
    /// Start timestamp (`YYYY-MM-DD HH:MM` or ISO-8601 local timestamp).
    pub started_at: String,
    /// End timestamp (`YYYY-MM-DD HH:MM` or ISO-8601 local timestamp).
    pub ended_at: String,
    /// Concise third-person narrative summary of the moment.
    pub summary: String,
    /// Constituent activities that support this moment.
    #[serde(default)]
    pub activity_refs: Vec<ActivityRef>,
    /// Underlying chronicle segment anchors (`YYYYMMDD/stream/HHMMSS_LEN`).
    #[serde(default)]
    pub segment_refs: Vec<String>,
    /// Underlying media segment anchors (`YYYYMMDD/stream/HHMMSS_LEN`).
    #[serde(default)]
    pub media_refs: Vec<String>,
}

/// Stored facet-scoped Experience record (`facets/<facet>/experiences/<YYYYMMDD>.jsonl`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExperienceRecord {
    /// Unique Experience identifier (for example `exp_20261122_a1b2c3d4`).
    pub id: String,
    /// Single primary facet slug owning this Experience.
    pub primary_facet: String,
    /// Stable UUID of the owning facet declaration, used to adopt rows after facet merges.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub destination_id: Option<String>,
    /// Local anchor day (`YYYYMMDD`), matching the day file basename.
    pub anchor_day: String,
    /// Start timestamp across constituent activities.
    pub started_at: String,
    /// End timestamp across constituent activities.
    pub ended_at: String,
    /// Whether membership was auto-detected or owner-curated.
    #[serde(default)]
    pub membership_source: MembershipSource,
    /// Muted Experiences stay in the day file to prevent re-detection while hidden everywhere.
    #[serde(default)]
    pub hidden: bool,
    /// Per-field and per-moment owner holds and activity exclusions.
    #[serde(default)]
    pub owner_holds: OwnerHolds,
    /// Chronological audit trail of owner and automated mutations.
    #[serde(default)]
    pub edits: Vec<Value>,
    /// Concise third-person Experience heading, distinct from `summary`'s opening sentence.
    #[serde(default)]
    pub title: String,
    /// Third-person overview across the full Experience arc.
    #[serde(default)]
    pub summary: String,
    /// Coarse physical places grounded in constituent activities.
    #[serde(default)]
    pub locations: Vec<String>,
    /// Constituent activity references (1..N).
    #[serde(default)]
    pub activities: Vec<ActivityRef>,
    /// Corroborated `Person` entity IDs (owner excluded).
    #[serde(default)]
    pub entity_refs: Vec<String>,
    /// Flat chronological list of key narrative moments.
    #[serde(default)]
    pub moments: Vec<ExperienceMoment>,
    /// Optional detector category hint (`social_gathering`, `meeting_or_call`, `outing_or_errand`, `solo_milestone`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    /// RFC-3339 UTC timestamp when composition last populated derived fields.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub composed_at: Option<String>,
    /// RFC-3339 UTC timestamp when the Experience record was first created.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
    /// RFC-3339 UTC timestamp of the most recent update.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
}

/// Pure share-time detail projection policy, decoupled from local Experience storage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShareDetailPolicy {
    /// Title, summary, span, corroborated people names, and coarse locations only.
    OverviewOnly,
    /// Overview plus narrative moments and attached media references.
    OverviewMomentsMedia,
    /// Full hydrated detail including underlying activity summaries and segment anchors.
    FullHydratedDetail,
}

impl ShareDetailPolicy {
    /// Parse a CLI or query tier token (`overview`, `moments`, `full`, or canonical snake_case).
    #[must_use]
    pub fn parse_tier(token: &str) -> Option<Self> {
        match token.trim().to_ascii_lowercase().as_str() {
            "overview" | "overview_only" => Some(Self::OverviewOnly),
            "moments" | "overview_moments_media" => Some(Self::OverviewMomentsMedia),
            "full" | "full_hydrated" | "full_hydrated_detail" => Some(Self::FullHydratedDetail),
            _ => None,
        }
    }

    /// Short query/CLI tier token.
    #[must_use]
    pub const fn tier_token(self) -> &'static str {
        match self {
            Self::OverviewOnly => "overview",
            Self::OverviewMomentsMedia => "moments",
            Self::FullHydratedDetail => "full",
        }
    }

    /// Canonical policy name.
    #[must_use]
    pub const fn policy_name(self) -> &'static str {
        match self {
            Self::OverviewOnly => "overview_only",
            Self::OverviewMomentsMedia => "overview_moments_media",
            Self::FullHydratedDetail => "full_hydrated_detail",
        }
    }
}

/// Whether an Experience JSON object is muted (`hidden: true`).
#[must_use]
pub fn is_hidden_experience(record: &JsonObject) -> bool {
    json_truthy(record.get("hidden"))
}

/// Whether `field` (`"title"`, `"summary"`, or `"activities"`) is held by the owner.
#[must_use]
pub fn experience_field_held(record: &ExperienceRecord, field: &str) -> bool {
    match field {
        "title" => record.owner_holds.title,
        "summary" => record.owner_holds.summary,
        "activities" => record.owner_holds.activities,
        _ => false,
    }
}

/// Whether an individual moment ID is held by the owner.
#[must_use]
pub fn experience_moment_held(record: &ExperienceRecord, moment_id: &str) -> bool {
    record.owner_holds.moment_ids.contains(moment_id)
}

/// Render Experience JSONL records into canonical search chunks.
///
/// Hidden records still emit an indexed-position slot carrying `source.hidden = true`
/// so [`super::indexed`] drops them from SQLite FTS while preserving positional indices
/// for live sibling records in the same day file.
pub fn render(rel: Option<&str>, records: &[JsonObject]) -> ProducedChunks {
    render_for_locale(rel, records, solstone_core_window_catalog::ENGLISH)
}

pub fn render_for_locale(
    rel: Option<&str>,
    records: &[JsonObject],
    _locale: &str,
) -> ProducedChunks {
    let mut chunks = Vec::with_capacity(records.len());
    for record in records {
        let mut source = Map::new();
        if let Some(id) = record.get("id").and_then(Value::as_str) {
            source.insert("id".to_owned(), Value::String(id.to_owned()));
        }
        if let Some(facet) = record.get("primary_facet").and_then(Value::as_str) {
            source.insert("primary_facet".to_owned(), Value::String(facet.to_owned()));
        }
        if let Some(day) = record.get("anchor_day").and_then(Value::as_str) {
            source.insert("anchor_day".to_owned(), Value::String(day.to_owned()));
        }
        let hidden = is_hidden_experience(record);
        source.insert("hidden".to_owned(), Value::Bool(hidden));

        let content = if hidden {
            String::new()
        } else {
            render_experience_markdown(rel, record)
        };
        let occurrence_time_ms = record
            .get("started_at")
            .and_then(Value::as_str)
            .and_then(parse_occurrence_ms)
            .map(super::OccurrenceTimeMs);

        chunks.push(IndexChunk {
            content,
            occurrence_time_ms,
            source: Some(source),
            group_offset_ms: None,
        });
    }

    ProducedChunks {
        chunks,
        agent_override: Some("experience".to_owned()),
        header: None,
        error: None,
        warnings: Vec::new(),
    }
}

fn render_experience_markdown(rel: Option<&str>, record: &JsonObject) -> String {
    let title = record
        .get("title")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("Experience");
    let summary = record
        .get("summary")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("");
    let started_at = record
        .get("started_at")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("");
    let ended_at = record
        .get("ended_at")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("");
    let facet = record
        .get("primary_facet")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_else(|| facet_from_rel(rel));

    let mut lines = Vec::new();
    lines.push(format!("# {title}"));
    if !facet.is_empty() || !started_at.is_empty() {
        let mut meta = Vec::new();
        if !facet.is_empty() {
            meta.push(format!("Facet: {facet}"));
        }
        if !started_at.is_empty() && !ended_at.is_empty() {
            meta.push(format!("Span: {started_at} to {ended_at}"));
        } else if !started_at.is_empty() {
            meta.push(format!("Span: {started_at}"));
        }
        lines.push(meta.join(" | "));
    }
    if !summary.is_empty() {
        lines.push(String::new());
        lines.push(summary.to_owned());
    }
    if let Some(locations) = record.get("locations").and_then(Value::as_array) {
        let place_names = locations
            .iter()
            .filter_map(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .collect::<Vec<_>>();
        if !place_names.is_empty() {
            lines.push(String::new());
            lines.push(format!("Locations: {}", place_names.join(", ")));
        }
    }
    if let Some(moments) = record.get("moments").and_then(Value::as_array) {
        let moment_lines = moments
            .iter()
            .filter_map(Value::as_object)
            .filter_map(|moment| {
                let text = moment
                    .get("summary")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())?;
                let start = moment
                    .get("started_at")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .unwrap_or("");
                let end = moment
                    .get("ended_at")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .unwrap_or("");
                if !start.is_empty() && !end.is_empty() {
                    Some(format!("- {start} to {end}: {text}"))
                } else {
                    Some(format!("- {text}"))
                }
            })
            .collect::<Vec<_>>();
        if !moment_lines.is_empty() {
            lines.push(String::new());
            lines.push("Moments:".to_owned());
            lines.extend(moment_lines);
        }
    }
    lines.join("\n")
}

fn facet_from_rel(rel: Option<&str>) -> &str {
    let Some(rel) = rel else {
        return "";
    };
    let mut parts = rel.split('/');
    if parts.next() == Some("facets") {
        parts.next().unwrap_or("")
    } else {
        ""
    }
}

fn parse_occurrence_ms(raw: &str) -> Option<i64> {
    if let Ok(ts) = raw.parse::<jiff::Timestamp>() {
        return Some(ts.as_millisecond());
    }
    let normalized = raw.replace(' ', "T");
    if let Ok(dt) = normalized.parse::<jiff::civil::DateTime>()
        && let Ok(ts) = jiff::tz::Offset::UTC.to_timestamp(dt)
    {
        return Some(ts.as_millisecond());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn hidden_experience_is_excluded_from_index_while_preserving_position() {
        let live = json!({
            "id": "exp_20260304_live",
            "primary_facet": "personal",
            "anchor_day": "20260304",
            "started_at": "2026-03-04T18:00:00",
            "ended_at": "2026-03-04T20:00:00",
            "hidden": false,
            "title": "Evening dinner with Maya",
            "summary": "Alex met Maya for dinner downtown.",
            "locations": ["Downtown"],
            "moments": [{
                "id": "m1",
                "started_at": "2026-03-04 18:00",
                "ended_at": "2026-03-04 19:30",
                "summary": "Caught up over tacos.",
                "activity_refs": [],
                "segment_refs": [],
                "media_refs": []
            }]
        });
        let muted = json!({
            "id": "exp_20260304_muted",
            "primary_facet": "personal",
            "anchor_day": "20260304",
            "started_at": "2026-03-04T21:00:00",
            "ended_at": "2026-03-04T21:30:00",
            "hidden": true,
            "title": "Hidden note",
            "summary": "Should never enter FTS."
        });
        let records = vec![
            muted.as_object().unwrap().clone(),
            live.as_object().unwrap().clone(),
        ];
        let produced = render(Some("facets/personal/experiences/20260304.jsonl"), &records);
        assert_eq!(produced.agent_override.as_deref(), Some("experience"));
        assert_eq!(produced.chunks.len(), 2);
        assert!(!super::super::indexed(
            super::super::Family::Experience,
            &produced.chunks[0]
        ));
        assert!(super::super::indexed(
            super::super::Family::Experience,
            &produced.chunks[1]
        ));
        assert!(
            produced.chunks[1]
                .content
                .contains("Evening dinner with Maya")
        );
        assert!(produced.chunks[1].content.contains("Caught up over tacos."));
    }

    #[test]
    fn share_detail_policy_tier_round_trip() {
        assert_eq!(
            ShareDetailPolicy::parse_tier("overview"),
            Some(ShareDetailPolicy::OverviewOnly)
        );
        assert_eq!(
            ShareDetailPolicy::parse_tier("moments"),
            Some(ShareDetailPolicy::OverviewMomentsMedia)
        );
        assert_eq!(
            ShareDetailPolicy::parse_tier("full"),
            Some(ShareDetailPolicy::FullHydratedDetail)
        );
        assert_eq!(ShareDetailPolicy::parse_tier("unknown"), None);
    }
}
