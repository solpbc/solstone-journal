// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Read-time hydration of lightweight Experience internal refs and decoupled
//! share-time detail policy projection.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use solstone_core_entity::{
    hidden_participation_entity_ids, live_merge_successor, load_all_journal_entities,
};
use solstone_core_format::content::experiences::{
    ActivityRef, ExperienceMoment, ExperienceRecord, ShareDetailPolicy,
};
use solstone_core_segment::{effective_activity_stream, live_segment_dir};

use crate::store::experiences::{activity_row_bounds_secs, format_hm, format_iso_day};
use crate::{list_declared_facet_names, load_activity_records, read_facet_declaration};

/// Hydrated corroborated person attached to an Experience.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HydratedPerson {
    pub entity_id: String,
    pub name: String,
    #[serde(rename = "type")]
    pub entity_type: String,
}

/// Segment evidence link attached to a hydrated constituent activity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HydratedSegmentEvidence {
    pub day: String,
    pub stream: String,
    pub segment: String,
    pub transcript_url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub photo_url: Option<String>,
}

/// Hydrated constituent activity view resolved live from facet JSONL.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HydratedActivity {
    #[serde(rename = "ref")]
    pub ref_item: ActivityRef,
    pub facet: String,
    #[serde(default)]
    pub stream: String,
    pub activity: String,
    pub title: String,
    pub description: String,
    pub started_at: String,
    pub ended_at: String,
    pub duration_min: u32,
    pub seg_count: usize,
    #[serde(default)]
    pub segments: Vec<String>,
    #[serde(default)]
    pub segment_evidence: Vec<HydratedSegmentEvidence>,
    pub has_story: bool,
    pub hidden: bool,
    pub missing: bool,
}

/// Hydrated media item resolved live from chronicle segments.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HydratedMedia {
    pub media_ref: String,
    pub day: String,
    pub stream: String,
    pub segment: String,
    /// `"photo"` or `"screenshot"`.
    pub kind: String,
    pub caption: String,
    pub live: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transcript_url: Option<String>,
}

/// Nearby activity candidate offered to the owner editor for structural add
/// (including optional sibling-facet candidates for explicit cross-facet add).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NearbyCandidateActivity {
    #[serde(rename = "ref")]
    pub ref_item: ActivityRef,
    pub facet: String,
    pub activity: String,
    pub title: String,
    pub description: String,
    pub started_at: String,
    pub ended_at: String,
    pub duration_min: u32,
    pub seg_count: usize,
    pub is_cross_facet: bool,
}

/// Stored Experience record combined with live hydrated people, activities, and media.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HydratedExperience {
    #[serde(flatten)]
    pub experience: ExperienceRecord,
    pub hydrated: HydratedRefs,
    /// All live photo/screenshot media co-present across this Experience's attached
    /// activities (including media not yet selected in any moment), for the
    /// per-moment media picker.
    #[serde(default)]
    pub available_media: Vec<HydratedMedia>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nearby_candidates: Option<Vec<NearbyCandidateActivity>>,
}

/// Live resolved internal references attached to a hydrated Experience.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HydratedRefs {
    pub people: Vec<HydratedPerson>,
    pub activities: Vec<HydratedActivity>,
    pub media: Vec<HydratedMedia>,
}

/// Projected moment shape emitted by [`project_experience_for_share`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShareProjectedMoment {
    pub id: String,
    pub started_at: String,
    pub ended_at: String,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub media_refs: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub activity_refs: Vec<ActivityRef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub segment_refs: Vec<String>,
}

/// Pure share-time projection over a hydrated Experience under a [`ShareDetailPolicy`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShareProjectedExperience {
    pub policy: String,
    pub tier: String,
    pub id: String,
    pub primary_facet: String,
    pub anchor_day: String,
    pub started_at: String,
    pub ended_at: String,
    pub title: String,
    pub summary: String,
    pub people: Vec<String>,
    pub locations: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub moments: Option<Vec<ShareProjectedMoment>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media: Option<Vec<HydratedMedia>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activities: Option<Vec<HydratedActivity>>,
}

/// Hydrate a stored Experience record live against current journal state.
///
/// - Respects `hidden_participation_entity_ids` (dropping trashed/blocked/principal/non-Person IDs)
///   and remaps merged entities to their live survivor.
/// - Hydrates constituent activities from each `ActivityRef`'s effective facet and day.
/// - Hydrates `media_refs` against live chronicle segment directories (`live_segment_dir`).
/// - Optionally collects nearby candidate activities (`Some(false)` for primary facet only,
///   `Some(true)` to include sibling unmuted facets for explicit cross-facet add).
pub fn hydrate_experience(
    journal_root: &Path,
    experience: ExperienceRecord,
    include_nearby: Option<bool>,
) -> HydratedExperience {
    let hidden_ids = hidden_participation_entity_ids(journal_root);
    let entities_by_id: BTreeMap<String, (String, String)> =
        load_all_journal_entities(journal_root)
            .unwrap_or_default()
            .into_iter()
            .filter(|e| !e.is_blocked())
            .map(|e| {
                let name = e
                    .value
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .trim()
                    .to_owned();
                let etype = e.entity_type().unwrap_or("Person").to_owned();
                (e.id, (name, etype))
            })
            .collect();

    let mut seen_people = BTreeSet::new();
    let mut people = Vec::new();
    for raw_eid in &experience.entity_refs {
        let resolved_id =
            live_merge_successor(journal_root, raw_eid).unwrap_or_else(|| raw_eid.clone());
        if hidden_ids.contains(&resolved_id) || hidden_ids.contains(raw_eid) {
            continue;
        }
        if !seen_people.insert(resolved_id.clone()) {
            continue;
        }
        let (name, entity_type) = match entities_by_id.get(&resolved_id) {
            Some((name, etype)) => {
                if !etype.eq_ignore_ascii_case("Person") {
                    continue;
                }
                let display = if name.is_empty() {
                    resolved_id.clone()
                } else {
                    name.clone()
                };
                (display, etype.clone())
            }
            None => (resolved_id.clone(), "Person".to_owned()),
        };
        people.push(HydratedPerson {
            entity_id: resolved_id,
            name,
            entity_type,
        });
    }

    let mut day_cache: BTreeMap<(String, String), Vec<Map<String, Value>>> = BTreeMap::new();
    let mut activities = Vec::with_capacity(experience.activities.len());
    for act_ref in &experience.activities {
        let facet = act_ref
            .effective_facet(&experience.primary_facet)
            .to_owned();
        let rows = day_cache
            .entry((facet.clone(), act_ref.day.clone()))
            .or_insert_with(|| {
                load_activity_records(journal_root, &facet, &act_ref.day, true).unwrap_or_default()
            });
        let found = rows.iter().find(|row| {
            let row_id = row.get("id").and_then(Value::as_str).unwrap_or("");
            let row_stream =
                effective_activity_stream(journal_root, &act_ref.day, row).unwrap_or("");
            row_id == act_ref.id && (act_ref.stream.is_empty() || row_stream == act_ref.stream)
        });
        match found {
            Some(row) => {
                activities.push(hydrate_activity_row(journal_root, act_ref, &facet, row));
            }
            None => {
                activities.push(HydratedActivity {
                    ref_item: act_ref.clone(),
                    facet,
                    stream: act_ref.stream.clone(),
                    activity: String::new(),
                    title: String::new(),
                    description: String::new(),
                    started_at: String::new(),
                    ended_at: String::new(),
                    duration_min: 0,
                    seg_count: 0,
                    segments: Vec::new(),
                    segment_evidence: Vec::new(),
                    has_story: false,
                    hidden: false,
                    missing: true,
                });
            }
        }
    }

    let mut seen_media = BTreeSet::new();
    let mut media = Vec::new();
    for moment in &experience.moments {
        for mref in &moment.media_refs {
            if !seen_media.insert(mref.clone()) {
                continue;
            }
            if let Some(item) = hydrate_media_ref(journal_root, mref) {
                media.push(item);
            }
        }
    }

    // Collect full co-present live photo/screenshot pool across all attached activities
    // plus any live moment media_refs (for the Level 1 per-moment photo picker).
    let mut available_media = Vec::new();
    let mut seen_avail = BTreeSet::new();
    for act in &activities {
        for seg in &act.segment_evidence {
            if seg.photo_url.is_some() {
                let mref = format!("{}/{}/{}", seg.day, seg.stream, seg.segment);
                if seen_avail.insert(mref.clone())
                    && let Some(item) = hydrate_media_ref(journal_root, &mref)
                    && item.live
                {
                    available_media.push(item);
                }
            }
        }
    }
    for item in &media {
        if item.live && seen_avail.insert(item.media_ref.clone()) {
            available_media.push(item.clone());
        }
    }

    let nearby_candidates = include_nearby.map(|include_cross_facet| {
        nearby_candidate_activities(journal_root, &experience, include_cross_facet)
    });

    HydratedExperience {
        experience,
        hydrated: HydratedRefs {
            people,
            activities,
            media,
        },
        available_media,
        nearby_candidates,
    }
}

/// Pure share-time projection over a hydrated Experience.
#[must_use]
pub fn project_experience_for_share(
    hydrated: &HydratedExperience,
    policy: ShareDetailPolicy,
) -> ShareProjectedExperience {
    let exp = &hydrated.experience;
    let people = hydrated
        .hydrated
        .people
        .iter()
        .map(|person| person.name.clone())
        .collect::<Vec<_>>();
    let live_media_refs: BTreeSet<&str> = hydrated
        .hydrated
        .media
        .iter()
        .filter(|item| item.live)
        .map(|item| item.media_ref.as_str())
        .collect();

    let (moments, media, activities) = match policy {
        ShareDetailPolicy::OverviewOnly => (None, None, None),
        ShareDetailPolicy::OverviewMomentsMedia => {
            let projected_moments = exp
                .moments
                .iter()
                .map(|m| project_moment(m, false, &live_media_refs))
                .collect();
            let live_media = hydrated
                .hydrated
                .media
                .iter()
                .filter(|item| item.live)
                .cloned()
                .collect();
            (Some(projected_moments), Some(live_media), None)
        }
        ShareDetailPolicy::FullHydratedDetail => {
            let projected_moments = exp
                .moments
                .iter()
                .map(|m| project_moment(m, true, &live_media_refs))
                .collect();
            let live_media = hydrated
                .hydrated
                .media
                .iter()
                .filter(|item| item.live)
                .cloned()
                .collect();
            let live_activities = hydrated
                .hydrated
                .activities
                .iter()
                .filter(|act| !act.missing && !act.hidden)
                .cloned()
                .collect();
            (
                Some(projected_moments),
                Some(live_media),
                Some(live_activities),
            )
        }
    };

    ShareProjectedExperience {
        policy: policy.policy_name().to_owned(),
        tier: policy.tier_token().to_owned(),
        id: exp.id.clone(),
        primary_facet: exp.primary_facet.clone(),
        anchor_day: exp.anchor_day.clone(),
        started_at: exp.started_at.clone(),
        ended_at: exp.ended_at.clone(),
        title: exp.title.clone(),
        summary: exp.summary.clone(),
        people,
        locations: exp.locations.clone(),
        moments,
        media,
        activities,
    }
}

fn project_moment(
    moment: &ExperienceMoment,
    include_full_refs: bool,
    live_media_refs: &BTreeSet<&str>,
) -> ShareProjectedMoment {
    let media_refs = moment
        .media_refs
        .iter()
        .filter(|mref| live_media_refs.contains(mref.as_str()))
        .cloned()
        .collect();
    if include_full_refs {
        ShareProjectedMoment {
            id: moment.id.clone(),
            started_at: moment.started_at.clone(),
            ended_at: moment.ended_at.clone(),
            summary: moment.summary.clone(),
            media_refs,
            activity_refs: moment.activity_refs.clone(),
            segment_refs: moment.segment_refs.clone(),
        }
    } else {
        ShareProjectedMoment {
            id: moment.id.clone(),
            started_at: moment.started_at.clone(),
            ended_at: moment.ended_at.clone(),
            summary: moment.summary.clone(),
            media_refs,
            activity_refs: Vec::new(),
            segment_refs: Vec::new(),
        }
    }
}

/// Find nearby live activities on the Experience's days that are not yet included.
/// When `include_cross_facet` is true, also searches sibling unmuted facets for explicit cross-facet add.
pub fn nearby_candidate_activities(
    journal_root: &Path,
    experience: &ExperienceRecord,
    include_cross_facet: bool,
) -> Vec<NearbyCandidateActivity> {
    let mut days = BTreeSet::new();
    days.insert(experience.anchor_day.clone());
    for act in &experience.activities {
        days.insert(act.day.clone());
    }

    let existing_keys: BTreeSet<(String, String, String, String)> = experience
        .activities
        .iter()
        .map(|act| {
            (
                act.day.clone(),
                act.effective_facet(&experience.primary_facet).to_owned(),
                act.stream.clone(),
                act.id.clone(),
            )
        })
        .collect();

    let target_facets: Vec<String> = if include_cross_facet {
        list_declared_facet_names(journal_root)
            .unwrap_or_else(|_| vec![experience.primary_facet.clone()])
            .into_iter()
            .filter(|slug| {
                slug == &experience.primary_facet
                    || read_facet_declaration(journal_root, slug)
                        .ok()
                        .flatten()
                        .and_then(|decl| decl.muted)
                        != Some(true)
            })
            .collect()
    } else {
        vec![experience.primary_facet.clone()]
    };

    let mut out = Vec::new();
    for facet in target_facets {
        let is_cross = facet != experience.primary_facet;
        for day in &days {
            let rows = load_activity_records(journal_root, &facet, day, false).unwrap_or_default();
            for row in rows {
                let act_type = row.get("activity").and_then(Value::as_str).unwrap_or("");
                let source = row.get("source").and_then(Value::as_str).unwrap_or("");
                if matches!(act_type, "idle" | "gaming") || matches!(source, "body" | "anticipated")
                {
                    continue;
                }
                let id = row
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned();
                if id.is_empty() {
                    continue;
                }
                let stream = effective_activity_stream(journal_root, day, &row)
                    .unwrap_or("")
                    .to_owned();
                if existing_keys.contains(&(day.clone(), facet.clone(), stream.clone(), id.clone()))
                {
                    continue;
                }
                let ref_item = ActivityRef {
                    day: day.clone(),
                    stream,
                    id,
                    facet: is_cross.then(|| facet.clone()),
                };
                let hydrated = hydrate_activity_row(journal_root, &ref_item, &facet, &row);
                out.push(NearbyCandidateActivity {
                    ref_item,
                    facet: facet.clone(),
                    activity: hydrated.activity,
                    title: hydrated.title,
                    description: hydrated.description,
                    started_at: hydrated.started_at,
                    ended_at: hydrated.ended_at,
                    duration_min: hydrated.duration_min,
                    seg_count: hydrated.seg_count,
                    is_cross_facet: is_cross,
                });
            }
        }
    }
    out.sort_by(|a, b| {
        a.started_at
            .cmp(&b.started_at)
            .then(a.is_cross_facet.cmp(&b.is_cross_facet))
            .then(a.ref_item.id.cmp(&b.ref_item.id))
    });
    out
}

fn transcript_segment_url(day: &str, stream: &str, segment: &str) -> String {
    if stream.is_empty() {
        format!("/app/transcripts/{day}#{segment}")
    } else {
        format!("/app/transcripts/{day}?stream={stream}#{segment}")
    }
}

fn find_segment_image_url(
    journal_root: &Path,
    day: &str,
    stream: &str,
    segment: &str,
) -> Option<String> {
    let dir = live_segment_dir(journal_root, day, stream, segment)?;
    const CANDIDATES: &[&str] = &[
        "original.jpg",
        "original.jpeg",
        "original.png",
        "original.webp",
        "original.heic",
        "original.screenshot.png",
        "original.screenshot.jpg",
        "image.jpg",
        "image.png",
    ];
    for name in CANDIDATES {
        if dir.join(name).is_file() {
            return if stream.is_empty() {
                Some(format!(
                    "/app/transcripts/api/serve_file/{day}/{segment}/{name}"
                ))
            } else {
                Some(format!(
                    "/app/transcripts/api/serve_file/{day}/{stream}/{segment}/{name}"
                ))
            };
        }
    }
    if stream.contains("image")
        || stream.contains("photo")
        || stream.contains("screenshot")
        || dir.join("image_transcript.md").is_file()
        || dir.join("screenshot_transcript.md").is_file()
    {
        return Some(transcript_segment_url(day, stream, segment));
    }
    None
}

fn hydrate_activity_row(
    journal_root: &Path,
    act_ref: &ActivityRef,
    facet: &str,
    row: &Map<String, Value>,
) -> HydratedActivity {
    let (started_at, ended_at, duration_min) = match activity_row_bounds_secs(row) {
        Some((s, e)) => (
            format!("{}T{}:00", format_iso_day(&act_ref.day), format_hm(s)),
            format!("{}T{}:00", format_iso_day(&act_ref.day), format_hm(e)),
            e.saturating_sub(s) / 60,
        ),
        None => (String::new(), String::new(), 0),
    };
    let story_body = row
        .get("story")
        .and_then(Value::as_object)
        .and_then(|s| s.get("body"))
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("");
    let title = row
        .get("title")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .or_else(|| {
            row.get("description")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|v| !v.is_empty())
        })
        .unwrap_or("")
        .to_owned();
    let description = row
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_owned();
    let stream = effective_activity_stream(journal_root, &act_ref.day, row)
        .map(str::to_owned)
        .unwrap_or_else(|| act_ref.stream.clone());
    let segments: Vec<String> = row
        .get("segments")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    let seg_count = segments.len();
    let segment_evidence: Vec<HydratedSegmentEvidence> = segments
        .iter()
        .map(|seg| HydratedSegmentEvidence {
            day: act_ref.day.clone(),
            stream: stream.clone(),
            segment: seg.clone(),
            transcript_url: transcript_segment_url(&act_ref.day, &stream, seg),
            photo_url: find_segment_image_url(journal_root, &act_ref.day, &stream, seg),
        })
        .collect();
    HydratedActivity {
        ref_item: act_ref.clone(),
        facet: facet.to_owned(),
        stream,
        activity: row
            .get("activity")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned(),
        title,
        description,
        started_at,
        ended_at,
        duration_min,
        seg_count,
        segments,
        segment_evidence,
        has_story: !story_body.is_empty(),
        hidden: row.get("hidden").and_then(Value::as_bool).unwrap_or(false),
        missing: false,
    }
}

fn hydrate_media_ref(journal_root: &Path, media_ref: &str) -> Option<HydratedMedia> {
    let mut parts = media_ref.splitn(3, '/');
    let day = parts.next()?.to_owned();
    let stream = parts.next()?.to_owned();
    let segment = parts.next()?.to_owned();
    if day.len() != 8 || stream.is_empty() || segment.is_empty() {
        return None;
    }
    let live_dir = live_segment_dir(journal_root, &day, &stream, &segment);
    let live = live_dir.is_some();
    let image_url = if live {
        find_segment_image_url(journal_root, &day, &stream, &segment)
    } else {
        None
    };
    let transcript_url = Some(transcript_segment_url(&day, &stream, &segment));
    let (kind, caption) = if let Some(dir) = live_dir {
        let is_screenshot = stream.contains("screenshot")
            || dir.join("screenshot_transcript.md").is_file()
            || dir.join("original.screenshot.png").is_file()
            || dir.join("original.screenshot.jpg").is_file();
        let kind = if is_screenshot {
            "screenshot".to_owned()
        } else {
            "photo".to_owned()
        };
        let transcript_file = if is_screenshot && dir.join("screenshot_transcript.md").is_file() {
            dir.join("screenshot_transcript.md")
        } else {
            dir.join("image_transcript.md")
        };
        let caption = std::fs::read_to_string(transcript_file)
            .ok()
            .map(|text| extract_compact_image_caption(&text))
            .unwrap_or_default();
        (kind, caption)
    } else {
        let kind = if stream.contains("screenshot") {
            "screenshot".to_owned()
        } else {
            "photo".to_owned()
        };
        (kind, String::new())
    };
    Some(HydratedMedia {
        media_ref: media_ref.to_owned(),
        day,
        stream,
        segment,
        kind,
        caption,
        live,
        image_url,
        transcript_url,
    })
}

/// Extract clean image caption lines from an `image_transcript.md` or `screenshot_transcript.md`.
#[must_use]
pub fn extract_compact_image_caption(text: &str) -> String {
    let mut quoted = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('>') && !trimmed.contains("[image description") {
            let cleaned = trimmed.trim_start_matches('>').trim();
            if !cleaned.is_empty() {
                quoted.push(cleaned);
            }
        }
    }
    let joined = if !quoted.is_empty() {
        quoted.join(" ")
    } else {
        text.lines()
            .map(str::trim)
            .filter(|line| !line.is_empty() && !line.starts_with("---") && !line.starts_with('#'))
            .take(3)
            .collect::<Vec<_>>()
            .join(" ")
    };
    let unescaped = joined
        .replace("\\,", ",")
        .replace("\\-", "-")
        .replace("\\'", "'");
    unescaped
        .chars()
        .take(260)
        .collect::<String>()
        .trim()
        .to_owned()
}
