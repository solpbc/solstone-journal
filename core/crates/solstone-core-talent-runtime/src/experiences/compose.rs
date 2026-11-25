// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Experience Composer (`experience_compose`) with:
//! - Generic 32k-floor bounded packet assembly and per-segment strided excerpts
//! - Omission of `facet.description` + deterministic `locations[]` grounding
//! - Deterministic `moment.started_at..ended_at` clamping
//! - Channel-token humanization + concurrent screen-bleed tagging
//! - Hard deterministic post-hooks + corrective-retry prompt synthesis + safe repair.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::OnceLock;

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use solstone_core_facets::{
    ActivityRef, ComposedExperienceUpdate, ExperienceMoment, ExperienceRecord,
    load_activity_records,
};
use solstone_core_local::generate::compute_input_budget;
use solstone_core_local::plan::LOCAL_MIN_CONTEXT_TOKENS;

use crate::contract::{CommitPlan, ParsedOutput, PrePostState};
use crate::writers::WriteIntent;
use crate::{
    ExecutionContext, PreparedTalent, RuntimeOutcome, StageError, apply_template_vars, stage_error,
};

use super::detect::estimate_prompt_overhead;
use super::prefilter::{
    CorroboratedPerson, OwnerIdentity, PersonIndex, activity_segments_bounds, collapse_m5_subsets,
    collapse_whitespace, extract_corroborated_people, format_hm, format_iso_day,
    humanize_stream_label, is_live_audio_or_call, is_secondary_screen_stream, parse_seg_bounds,
    read_segment_artifacts, row_segments, row_story_body, row_stream, truncate_chars,
};

fn re_quoted_ascii() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#""[^"\n]{1,300}""#).unwrap())
}

fn re_quoted_smart() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new("\u{201c}[^\u{201d}\n]{1,300}\u{201d}").unwrap())
}

fn re_second_person() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)\b(you|your|yours|yourself|yourselves|you're|you've|you'll|you'd)\b")
            .unwrap()
    })
}

fn re_first_person() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"\b(I|I'm|I've|I'll|I'd|my|me|mine|myself|we|we're|we've|we'll|we'd|our|ours|ourselves)\b",
        )
        .unwrap()
    })
}

fn re_speaker_literal() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)\bSpeaker\s+\d+\b").unwrap())
}

fn re_verbatim_dump() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r#"(?m)(^>\s|\b[A-Z][a-z]+:\s+"|\bSpeaker\s+\d+:|An unidentified speaker said:|whom the journal has not identified,\s+said:)"#,
        )
        .unwrap()
    })
}

fn re_credential_or_sensitive() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"(?i)(\b(?:gate\s*code|pin\s*code|passcode|password|access\s*code|door\s*code)\s*(?:is|:|#)?\s*[#*]?\d{3,8}\b|\border\s*(?:#|number|id|no\.?)\s*[:#]?\s*[A-Z0-9-]{5,}\b|\btracking\s*(?:#|number|id|no\.?)\s*[:#]?\s*[A-Z0-9-]{8,}\b|\b1Z[A-Z0-9]{10,}\b|\bsk-[A-Za-z0-9_-]{16,}\b|\b\d{3,5}\s+(?:[NSEW]\.?\s+)?[A-Z][a-z]+(?:\s+[A-Z][a-z]+){0,2}\s+(?:St|Street|Ave|Avenue|Rd|Road|Dr|Drive|Ln|Lane|Blvd|Way|Ct|Court|Pl|Place)\b)",
        )
        .unwrap()
    })
}

fn re_raw_stream_token() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)\b(import\.image|ios_screenshots|ios_mobile|obsidian|tmux)\b").unwrap()
    })
}

/// Remove text inside double quotes (ASCII and smart quotes) before checking pronouns.
pub fn strip_quoted_spans(text: &str) -> String {
    let pass1 = re_quoted_ascii().replace_all(text, "\"\"");
    re_quoted_smart().replace_all(&pass1, "\"\"").into_owned()
}

/// Hard prose violations detected on composed Experience text.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProseViolations {
    pub second_person: Vec<String>,
    pub first_person: Vec<String>,
    pub speaker_literal: Vec<String>,
    pub verbatim_dump: Vec<String>,
    pub sensitive_leak: Vec<String>,
    pub raw_stream_tokens: Vec<String>,
}

/// Run deterministic hard checks over third-person Experience prose.
pub fn inspect_prose_violations(text: &str) -> ProseViolations {
    let unquoted = strip_quoted_spans(text);
    ProseViolations {
        second_person: re_second_person()
            .find_iter(&unquoted)
            .map(|m| m.as_str().to_owned())
            .collect(),
        first_person: re_first_person()
            .find_iter(&unquoted)
            .map(|m| m.as_str().to_owned())
            .collect(),
        speaker_literal: re_speaker_literal()
            .find_iter(text)
            .map(|m| m.as_str().to_owned())
            .collect(),
        verbatim_dump: re_verbatim_dump()
            .find_iter(text)
            .map(|m| m.as_str().to_owned())
            .collect(),
        sensitive_leak: re_credential_or_sensitive()
            .find_iter(text)
            .map(|m| m.as_str().to_owned())
            .collect(),
        raw_stream_tokens: re_raw_stream_token()
            .find_iter(text)
            .map(|m| m.as_str().to_owned())
            .collect(),
    }
}

/// Check whether `title` repeats or duplicates `summary`'s opening sentence.
pub fn title_duplicates_summary_opening(title: &str, summary: &str) -> bool {
    let norm = |s: &str| -> String {
        s.chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() {
                    c.to_ascii_lowercase()
                } else {
                    ' '
                }
            })
            .collect::<String>()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    };
    let t = norm(title);
    let s = norm(summary);
    if t.is_empty() || s.is_empty() {
        return false;
    }
    let first_sent = summary.trim().split(['.', '!', '?']).next().unwrap_or("");
    let fs = norm(first_sent);
    t == fs || (t.len() >= 20 && s.starts_with(&t))
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SegmentExcerpt {
    pub segment_ref: String,
    pub time: String,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaExcerpt {
    pub media_ref: String,
    pub time: String,
    pub caption: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComposerActivityEntry {
    pub handle: String,
    pub activity_ref: ActivityRef,
    pub day: String,
    pub start_s: i64,
    pub end_s: i64,
    pub start_hm: String,
    pub end_hm: String,
    pub duration_min: i64,
    pub stream: String,
    pub channel_label: String,
    pub activity: String,
    pub segments: Vec<String>,
    pub description: String,
    pub has_story: bool,
    pub secondary_screen_context: bool,
    pub story_or_fallback: String,
    pub topics: Vec<String>,
    pub segment_notes: Vec<SegmentExcerpt>,
    pub media_items: Vec<MediaExcerpt>,
}

/// Assembled composer input packet and validation tables for one Experience.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComposePacket {
    pub experience_id: String,
    pub primary_facet: String,
    pub anchor_day: String,
    pub owner: OwnerIdentity,
    pub act_entries: Vec<ComposerActivityEntry>,
    pub valid_act_handles: BTreeMap<String, ActivityRef>,
    pub valid_seg_refs: BTreeSet<String>,
    pub valid_media_refs: BTreeSet<String>,
    pub corroborated_people: Vec<CorroboratedPerson>,
    pub grounding_corpus: String,
    pub prompt_text: String,
}

/// Pre-state for `experience_compose` hook stage.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComposePreState {
    pub packet: ComposePacket,
}

/// Post-processing audit report for one composer attempt.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComposeAudit {
    pub violations: ProseViolations,
    pub title_duplicates_summary: bool,
    pub needs_retry: bool,
    pub retry_reasons: Vec<String>,
    pub invalid_act_refs: usize,
    pub invalid_seg_refs: usize,
    pub invalid_media_refs: usize,
    pub dropped_people: usize,
    pub ungrounded_locations_dropped: usize,
    pub clamped_moment_timestamps: usize,
}

/// Assemble the composer packet for one Experience record, generically
/// fitted against the 32,768-token floor.
pub fn build_composer_packet(
    journal: &Path,
    experience: &ExperienceRecord,
    max_output_tokens: u32,
    overhead_tokens: u32,
) -> Result<ComposePacket, String> {
    let owner = OwnerIdentity::load(journal);
    let persons = PersonIndex::load(journal);

    // Load live activity rows (honoring cross-facet ActivityRef::effective_facet!).
    type DayActivityIndex = BTreeMap<(String, String), Map<String, Value>>;
    let mut day_cache: BTreeMap<(String, String), DayActivityIndex> = BTreeMap::new();
    let mut loaded: Vec<(String, String, Map<String, Value>)> = Vec::new();
    for act_ref in &experience.activities {
        let eff_facet = act_ref
            .effective_facet(&experience.primary_facet)
            .to_owned();
        let ck = (eff_facet.clone(), act_ref.day.clone());
        let map = day_cache.entry(ck).or_insert_with(|| {
            let mut m = BTreeMap::new();
            for row in
                load_activity_records(journal, &eff_facet, &act_ref.day, true).unwrap_or_default()
            {
                let stream = row_stream(journal, &act_ref.day, &row);
                let id = row
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned();
                if !id.is_empty() {
                    m.insert((stream, id.clone()), row.clone());
                    m.insert((String::new(), id), row);
                }
            }
            m
        });
        if let Some(row) = map
            .get(&(act_ref.stream.clone(), act_ref.id.clone()))
            .or_else(|| map.get(&(String::new(), act_ref.id.clone())))
        {
            loaded.push((act_ref.day.clone(), eff_facet, row.clone()));
        }
    }

    // Collapse any m5 subsets defensively per (day, facet).
    let mut by_day_facet: BTreeMap<(String, String), Vec<Map<String, Value>>> = BTreeMap::new();
    for (d, f, row) in loaded {
        by_day_facet.entry((d, f)).or_default().push(row);
    }
    let mut deduped: Vec<(String, String, Map<String, Value>)> = Vec::new();
    for ((d, f), rows) in by_day_facet {
        for row in collapse_m5_subsets(journal, &d, rows) {
            deduped.push((d.clone(), f.clone(), row));
        }
    }
    deduped.sort_by(|a, b| {
        let sa = activity_segments_bounds(&row_segments(&a.2)).map_or(i64::MAX, |(s, _)| s);
        let sb = activity_segments_bounds(&row_segments(&b.2)).map_or(i64::MAX, |(s, _)| s);
        let ida = a.2.get("id").and_then(Value::as_str).unwrap_or("");
        let idb = b.2.get("id").and_then(Value::as_str).unwrap_or("");
        a.0.cmp(&b.0)
            .then_with(|| sa.cmp(&sb))
            .then_with(|| ida.cmp(idb))
    });

    let all_rows_with_days: Vec<(String, Map<String, Value>)> = deduped
        .iter()
        .map(|(d, _, r)| (d.clone(), r.clone()))
        .collect();
    let corroborated_people =
        extract_corroborated_people(journal, &all_rows_with_days, &owner, &persons);

    // Identify live audio/call windows so concurrent screen/desk/terminal
    // activities can be explicitly tagged as secondary screen context.
    let mut live_call_windows: Vec<(String, i64, i64)> = Vec::new();
    for (day, _facet, row) in &deduped {
        let stream = row_stream(journal, day, row);
        let act = row.get("activity").and_then(Value::as_str).unwrap_or("");
        if is_live_audio_or_call(&stream, act)
            && let Some((s, e)) = activity_segments_bounds(&row_segments(row))
        {
            live_call_windows.push((day.clone(), s, e));
        }
    }

    let mut act_entries = Vec::with_capacity(deduped.len());
    let mut valid_act_handles = BTreeMap::new();
    let mut valid_seg_refs = BTreeSet::new();
    let mut valid_media_refs = BTreeSet::new();
    let mut corpus_parts: Vec<String> = Vec::new();

    for (idx, (day, eff_facet, row)) in deduped.into_iter().enumerate() {
        let handle = format!("ACT{}", idx + 1);
        let stream = row_stream(journal, &day, &row);
        let id = row
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        let segs = row_segments(&row);
        let Some((start_s, end_s)) = activity_segments_bounds(&segs) else {
            continue;
        };
        let duration_min = ((end_s - start_s) as f64 / 60.0).round().max(0.0) as i64;
        let act_ref = ActivityRef {
            day: day.clone(),
            stream: stream.clone(),
            id: id.clone(),
            facet: (eff_facet != experience.primary_facet).then(|| eff_facet.clone()),
        };
        valid_act_handles.insert(handle.clone(), act_ref.clone());

        let story_body = row_story_body(&row);
        let desc = row
            .get("description")
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or("")
            .to_owned();
        let details = row
            .get("details")
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or("")
            .to_owned();
        let mut topics: Vec<String> = Vec::new();
        for arr in [
            row.get("story")
                .and_then(Value::as_object)
                .and_then(|s| s.get("topics"))
                .and_then(Value::as_array),
            row.get("topics").and_then(Value::as_array),
        ]
        .into_iter()
        .flatten()
        {
            for v in arr {
                if let Some(t) = v.as_str().map(str::trim).filter(|s| !s.is_empty())
                    && !topics.iter().any(|e| e == t)
                {
                    topics.push(t.to_owned());
                }
            }
        }
        topics.truncate(8);

        for s in &segs {
            valid_seg_refs.insert(format!("{day}/{stream}/{s}"));
        }

        // Uniform stride up to 16 segments preserving first and last.
        let max_seg_notes = 16usize;
        let inspect_indices: Vec<usize> = if segs.len() <= max_seg_notes {
            (0..segs.len()).collect()
        } else {
            let step = (segs.len() - 1) as f64 / (max_seg_notes - 1) as f64;
            (0..max_seg_notes)
                .map(|i| (i as f64 * step).round() as usize)
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect()
        };

        let mut segment_notes = Vec::new();
        let mut media_items = Vec::new();
        for si in inspect_indices {
            let s = &segs[si];
            let sref = format!("{day}/{stream}/{s}");
            let art = read_segment_artifacts(journal, &day, &stream, s, &eff_facet);
            let Some((ss, se)) = parse_seg_bounds(s) else {
                continue;
            };
            if art.has_media {
                valid_media_refs.insert(sref.clone());
                let cap = if !art.image_caption.is_empty() {
                    art.image_caption.clone()
                } else if !art.facet_activity.is_empty() {
                    art.facet_activity.clone()
                } else if !desc.is_empty() {
                    desc.clone()
                } else {
                    art.summary.clone()
                };
                media_items.push(MediaExcerpt {
                    media_ref: sref.clone(),
                    time: format_hm(ss),
                    caption: truncate_chars(&collapse_whitespace(&cap), 260),
                });
            }
            if segs.len() >= 4 || story_body.is_empty() {
                let note_txt = if !art.summary.is_empty() {
                    art.summary
                } else if !art.facet_activity.is_empty() {
                    art.facet_activity
                } else {
                    art.image_caption
                };
                if !note_txt.trim().is_empty() {
                    segment_notes.push(SegmentExcerpt {
                        segment_ref: sref,
                        time: format!("{}-{}", format_hm(ss), format_hm(se)),
                        text: truncate_chars(&collapse_whitespace(&note_txt), 280),
                    });
                }
            }
        }

        let story_or_fallback = if !story_body.is_empty() {
            story_body.clone()
        } else {
            let mut fb_parts = Vec::new();
            if !desc.is_empty() {
                fb_parts.push(desc.clone());
            }
            if !details.is_empty() && details != desc {
                fb_parts.push(details.clone());
            }
            for mi in &media_items {
                if !mi.caption.is_empty() && !fb_parts.contains(&mi.caption) {
                    fb_parts.push(format!("Image caption: {}", mi.caption));
                }
            }
            if fb_parts.is_empty() {
                let act = row
                    .get("activity")
                    .and_then(Value::as_str)
                    .unwrap_or("activity");
                format!("({act} via {})", humanize_stream_label(&stream))
            } else {
                fb_parts.join(" | ")
            }
        };

        // Tag concurrent desktop/screen/terminal activities overlapping a live call/audio window.
        let secondary_screen_context = is_secondary_screen_stream(&stream)
            && live_call_windows
                .iter()
                .any(|(lday, ls, le)| lday == &day && start_s <= *le + 120 && end_s >= *ls - 120);

        // Collect grounding corpus strictly from included activity content
        // (NEVER from facet.description!).
        if !desc.is_empty() {
            corpus_parts.push(desc.clone());
        }
        corpus_parts.push(story_or_fallback.clone());
        for t in &topics {
            corpus_parts.push(t.clone());
        }
        for key in ["location", "place", "venue"] {
            if let Some(s) = row.get(key).and_then(Value::as_str) {
                corpus_parts.push(s.to_owned());
            }
        }
        if let Some(arr) = row.get("locations").and_then(Value::as_array) {
            for v in arr {
                if let Some(s) = v.as_str() {
                    corpus_parts.push(s.to_owned());
                }
            }
        }
        for sn in &segment_notes {
            corpus_parts.push(sn.text.clone());
        }
        for mi in &media_items {
            corpus_parts.push(mi.caption.clone());
        }

        let activity = row
            .get("activity")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        let channel_label = humanize_stream_label(&stream);

        act_entries.push(ComposerActivityEntry {
            handle,
            activity_ref: act_ref,
            day,
            start_s,
            end_s,
            start_hm: format_hm(start_s),
            end_hm: format_hm(end_s),
            duration_min,
            stream,
            channel_label,
            activity,
            segments: segs,
            description: desc,
            has_story: !story_body.is_empty(),
            secondary_screen_context,
            story_or_fallback,
            topics,
            segment_notes,
            media_items,
        });
    }

    // Keep composer packets focused and dense (capped below 7.5k packet tokens
    // and strictly within the 32k context floor) so multi-activity episodes
    // never drown the composer in redundant segment excerpts.
    const TARGET_COMPOSE_PACKET_TOKENS: u32 = 7_500;
    let grounding_corpus = corpus_parts.join("\n");
    let budget = compute_input_budget(max_output_tokens, LOCAL_MIN_CONTEXT_TOKENS)
        .saturating_sub(overhead_tokens)
        .clamp(2048, TARGET_COMPOSE_PACKET_TOKENS);
    let prompt_text = render_composer_packet_fitted(
        &owner,
        &experience.primary_facet,
        &corroborated_people,
        &act_entries,
        budget,
    );

    Ok(ComposePacket {
        experience_id: experience.id.clone(),
        primary_facet: experience.primary_facet.clone(),
        anchor_day: experience.anchor_day.clone(),
        owner,
        act_entries,
        valid_act_handles,
        valid_seg_refs,
        valid_media_refs,
        corroborated_people,
        grounding_corpus,
        prompt_text,
    })
}

fn render_composer_packet_fitted(
    owner: &OwnerIdentity,
    primary_facet: &str,
    corroborated_people: &[CorroboratedPerson],
    act_entries: &[ComposerActivityEntry],
    budget_tokens: u32,
) -> String {
    let anchor_start = act_entries
        .first()
        .map(|a| format!("{} {}", format_iso_day(&a.day), a.start_hm))
        .unwrap_or_default();
    let anchor_end = act_entries
        .last()
        .map(|a| format!("{} {}", format_iso_day(&a.day), a.end_hm))
        .unwrap_or_default();

    // Progressive fitting schedule: keep crisp per-activity detail under the
    // packet density cap and 32k floor, tightening excerpt count and story text cleanly.
    let tiers: &[(usize, usize)] = &[
        (16, 2000),
        (10, 1400),
        (6, 1000),
        (4, 700),
        (2, 500),
        (0, 400),
        (0, 260),
    ];

    let mut rendered = String::new();
    for &(max_notes, max_story_chars) in tiers {
        let mut lines = vec![
            format!(
                "Owner name (ALWAYS refer to the owner in 3rd person using this name): {} (full name: {})",
                owner.preferred, owner.full_name
            ),
            // Omit facet.description completely so facet-level venues never bleed.
            format!("Primary facet: {primary_facet}"),
            format!(
                "Episode span: {anchor_start} to {anchor_end} ({} activities)",
                act_entries.len()
            ),
            "Corroborated Person entities (ONLY use entity_id values from this list in people_refs; never include the owner):".to_owned(),
        ];
        if corroborated_people.is_empty() {
            lines.push("  - (none corroborated)".to_owned());
        } else {
            for cp in corroborated_people {
                lines.push(format!(
                    "  - entity_id={} | name={} | roles={}",
                    cp.entity_id,
                    cp.name,
                    cp.roles.join(",")
                ));
            }
        }
        lines.push("\nIncluded constituent activities:".to_owned());
        for ae in act_entries {
            let story_tag = if ae.has_story {
                ""
            } else {
                " [storyless-fallback]"
            };
            let screen_tag = if ae.secondary_screen_context {
                " [secondary screen context — ignore unrelated desktop text]"
            } else {
                ""
            };
            // Display humanized channel label instead of raw stream identifier.
            lines.push(format!(
                "\n[{}] {} {}-{} ({}m, {} segs) | channel={} | type={}{}{}",
                ae.handle,
                format_iso_day(&ae.day),
                ae.start_hm,
                ae.end_hm,
                ae.duration_min,
                ae.segments.len(),
                ae.channel_label,
                ae.activity,
                story_tag,
                screen_tag
            ));
            if !ae.description.is_empty() {
                lines.push(format!("  description: {}", ae.description));
            }
            let content = if max_story_chars == usize::MAX {
                ae.story_or_fallback.clone()
            } else {
                truncate_chars(&ae.story_or_fallback, max_story_chars)
            };
            lines.push(format!("  content: {content}"));
            if !ae.topics.is_empty() {
                lines.push(format!("  topics: {}", ae.topics.join(", ")));
            }
            for mi in &ae.media_items {
                lines.push(format!(
                    "  media_ref={} ({}): {}",
                    mi.media_ref, mi.time, mi.caption
                ));
            }
            if !ae.segment_notes.is_empty() && max_notes > 0 {
                let sampled: Vec<&SegmentExcerpt> = if ae.segment_notes.len() <= max_notes {
                    ae.segment_notes.iter().collect()
                } else {
                    let step = (ae.segment_notes.len() - 1) as f64 / (max_notes - 1) as f64;
                    (0..max_notes)
                        .map(|i| (i as f64 * step).round() as usize)
                        .collect::<BTreeSet<_>>()
                        .into_iter()
                        .map(|i| &ae.segment_notes[i])
                        .collect()
                };
                lines.push(
                    "  per-segment timeline excerpts (use exact segment_ref strings when anchoring moments):"
                        .to_owned(),
                );
                for sn in sampled {
                    lines.push(format!(
                        "    - segment_ref={} ({}): {}",
                        sn.segment_ref, sn.time, sn.text
                    ));
                }
            } else {
                let exact_srefs: Vec<String> = ae
                    .segments
                    .iter()
                    .take(16)
                    .map(|s| format!("{}/{}/{}", ae.day, ae.stream, s))
                    .collect();
                lines.push(format!("  valid segment_refs: {}", exact_srefs.join(", ")));
            }
        }
        rendered = lines.join("\n");
        if solstone_core_tokens::count(&rendered) <= budget_tokens {
            break;
        }
    }

    while solstone_core_tokens::count(&rendered) > budget_tokens && rendered.len() > 2048 {
        let target = rendered.len() * 3 / 4;
        let cut = rendered
            .char_indices()
            .take_while(|(i, _)| *i <= target)
            .last()
            .map_or(0, |(i, _)| i);
        rendered.truncate(cut);
        rendered.push_str("\n[truncated to context floor]");
    }
    rendered
}

const LOCATION_STOPWORDS: &[&str] = &[
    "the", "and", "for", "with", "from", "into", "near", "over", "under", "area", "place", "room",
    "office", "home", "house", "building", "city", "town", "street", "road", "center", "centre",
    "hall", "shop", "store", "park", "side", "north", "south", "east", "west", "local", "nearby",
];

/// Verify that a candidate physical location is grounded in the
/// included activities' text/excerpts/captions (not leaked from facet descriptions
/// or model priors).
pub fn is_location_grounded(location: &str, grounding_corpus: &str) -> bool {
    let trimmed = location.trim();
    if trimmed.is_empty() || re_credential_or_sensitive().is_match(trimmed) {
        return false;
    }
    let corpus_low = grounding_corpus.to_lowercase();
    let loc_low = trimmed.to_lowercase();
    if corpus_low.contains(&loc_low) {
        return true;
    }
    let sig_tokens: Vec<String> = loc_low
        .split(|c: char| !c.is_alphanumeric())
        .filter(|tok| tok.len() >= 3 && !LOCATION_STOPWORDS.contains(tok))
        .map(str::to_owned)
        .collect();
    if sig_tokens.is_empty() {
        return false;
    }
    let matched = sig_tokens
        .iter()
        .filter(|tok| corpus_low.contains(tok.as_str()))
        .count();
    let longest = sig_tokens.iter().max_by_key(|t| t.len()).unwrap();
    corpus_low.contains(longest.as_str()) && matched * 4 >= sig_tokens.len() * 3
}

/// Parse `YYYY-MM-DD HH:MM` or `YYYY-MM-DDTHH:MM(:SS)?` into epoch minutes for clamping.
fn parse_wall_minutes(raw: &str) -> Option<i64> {
    let trimmed = raw.trim();
    if trimmed.len() < 16 {
        return None;
    }
    let date_part = trimmed.get(0..10)?;
    let day_compact = date_part.replace('-', "");
    let date = solstone_core_time::parse_day(&day_compact)?;
    let time_part = trimmed.get(11..16)?;
    let mut parts = time_part.split(':');
    let hh: i64 = parts.next()?.parse().ok()?;
    let mm: i64 = parts.next()?.parse().ok()?;
    if !(0..=23).contains(&hh) || !(0..=59).contains(&mm) {
        return None;
    }
    let epoch = jiff::civil::date(1970, 1, 1);
    let days_since = date.duration_since(epoch).as_hours() / 24;
    Some(days_since * 1440 + hh * 60 + mm)
}

fn day_secs_to_wall_minutes(day: &str, secs: i64) -> Option<i64> {
    let date = solstone_core_time::parse_day(day)?;
    let epoch = jiff::civil::date(1970, 1, 1);
    let days_since = date.duration_since(epoch).as_hours() / 24;
    Some(days_since * 1440 + secs / 60)
}

fn format_wall_minutes(mins: i64) -> String {
    let day_offset = mins.div_euclid(1440);
    let min_of_day = mins.rem_euclid(1440);
    let epoch = jiff::civil::date(1970, 1, 1);
    let date = epoch
        .checked_add(jiff::Span::new().days(day_offset))
        .unwrap_or(epoch);
    let hh = min_of_day / 60;
    let mm = min_of_day % 60;
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}",
        date.year(),
        date.month(),
        date.day(),
        hh,
        mm
    )
}

/// Compute exact `[min_start_min, max_end_min]` for one moment from
/// its valid `segment_refs`, falling back to its valid `activity_refs`, then
/// the Experience's overall activity span.
fn moment_reference_bounds_minutes(
    srefs: &[String],
    arefs: &[ActivityRef],
    packet: &ComposePacket,
) -> Option<(i64, i64)> {
    let mut min_m = i64::MAX;
    let mut max_m = i64::MIN;
    let mut found = false;

    for sr in srefs {
        let mut parts = sr.split('/');
        if let (Some(day), Some(_stream), Some(seg)) = (parts.next(), parts.next(), parts.next())
            && let Some((ss, se)) = parse_seg_bounds(seg)
            && let (Some(ms), Some(me)) = (
                day_secs_to_wall_minutes(day, ss),
                day_secs_to_wall_minutes(day, se),
            )
        {
            min_m = min_m.min(ms);
            max_m = max_m.max(me);
            found = true;
        }
    }
    if found {
        return Some((min_m, max_m.max(min_m)));
    }

    for ar in arefs {
        if let Some(ae) = packet
            .act_entries
            .iter()
            .find(|e| e.day == ar.day && e.stream == ar.stream && e.activity_ref.id == ar.id)
            && let (Some(ms), Some(me)) = (
                day_secs_to_wall_minutes(&ae.day, ae.start_s),
                day_secs_to_wall_minutes(&ae.day, ae.end_s),
            )
        {
            min_m = min_m.min(ms);
            max_m = max_m.max(me);
            found = true;
        }
    }
    if found {
        return Some((min_m, max_m.max(min_m)));
    }

    for ae in &packet.act_entries {
        if let (Some(ms), Some(me)) = (
            day_secs_to_wall_minutes(&ae.day, ae.start_s),
            day_secs_to_wall_minutes(&ae.day, ae.end_s),
        ) {
            min_m = min_m.min(ms);
            max_m = max_m.max(me);
            found = true;
        }
    }
    found.then(|| (min_m, max_m.max(min_m)))
}

/// Post-process raw composer JSON output, enforcing location grounding,
/// moment timestamp clamping, channel/screen hygiene, and hard deterministic
/// prose/ref invariants.
pub fn postprocess_compose_output(
    raw_output: &Value,
    packet: &ComposePacket,
) -> (ComposedExperienceUpdate, ComposeAudit) {
    let mut audit = ComposeAudit::default();

    let title = raw_output
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_owned();
    let summary = raw_output
        .get("summary")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_owned();

    let valid_people: BTreeSet<&str> = packet
        .corroborated_people
        .iter()
        .map(|p| p.entity_id.as_str())
        .collect();
    let name_to_eid: BTreeMap<String, String> = packet
        .corroborated_people
        .iter()
        .map(|p| (p.name.trim().to_lowercase(), p.entity_id.clone()))
        .collect();

    let mut clean_people = Vec::new();
    if let Some(arr) = raw_output.get("people_refs").and_then(Value::as_array) {
        for item in arr {
            let raw = item.as_str().unwrap_or("").trim();
            let resolved = if valid_people.contains(raw) {
                Some(raw.to_owned())
            } else {
                name_to_eid.get(&raw.to_lowercase()).cloned()
            };
            if let Some(eid) = resolved
                && eid != packet.owner.entity_id
                && !clean_people.contains(&eid)
            {
                clean_people.push(eid);
            } else {
                audit.dropped_people += 1;
            }
        }
    }

    // Deterministically drop ungrounded or sensitive locations.
    let mut clean_locations = Vec::new();
    if let Some(arr) = raw_output.get("locations").and_then(Value::as_array) {
        for item in arr {
            let loc = item.as_str().unwrap_or("").trim();
            if loc.is_empty() {
                continue;
            }
            if is_location_grounded(loc, &packet.grounding_corpus) {
                if !clean_locations
                    .iter()
                    .any(|e: &String| e.eq_ignore_ascii_case(loc))
                {
                    clean_locations.push(loc.to_owned());
                }
            } else {
                audit.ungrounded_locations_dropped += 1;
            }
        }
    }
    clean_locations.truncate(4);

    let mut all_prose = vec![title.clone(), summary.clone()];
    let mut clean_moments = Vec::new();
    if let Some(arr) = raw_output.get("moments").and_then(Value::as_array) {
        for (idx, m) in arr.iter().enumerate() {
            let msum = m
                .get("summary")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .to_owned();
            if msum.is_empty() {
                continue;
            }
            all_prose.push(msum.clone());

            let mut arefs: Vec<ActivityRef> = Vec::new();
            let mut seen_a = BTreeSet::new();
            if let Some(handles) = m.get("activity_handles").and_then(Value::as_array) {
                for h in handles {
                    let hs = h.as_str().unwrap_or("").trim();
                    if let Some(aref) = packet.valid_act_handles.get(hs) {
                        let key = (aref.day.clone(), aref.stream.clone(), aref.id.clone());
                        if seen_a.insert(key) {
                            arefs.push(aref.clone());
                        }
                    } else {
                        audit.invalid_act_refs += 1;
                    }
                }
            }

            let mut srefs: Vec<String> = Vec::new();
            if let Some(arr_s) = m.get("segment_refs").and_then(Value::as_array) {
                for sv in arr_s {
                    let s = sv.as_str().unwrap_or("").trim();
                    if packet.valid_seg_refs.contains(s) {
                        if !srefs.iter().any(|e| e == s) {
                            srefs.push(s.to_owned());
                        }
                    } else {
                        audit.invalid_seg_refs += 1;
                    }
                }
            }

            let mut mrefs: Vec<String> = Vec::new();
            if let Some(arr_m) = m.get("media_refs").and_then(Value::as_array) {
                for mv in arr_m {
                    let mr = mv.as_str().unwrap_or("").trim();
                    if packet.valid_media_refs.contains(mr) {
                        if !mrefs.iter().any(|e| e == mr) {
                            mrefs.push(mr.to_owned());
                        }
                    } else {
                        audit.invalid_media_refs += 1;
                    }
                }
            }

            // If activity_refs is empty, recover from segment_refs.
            if arefs.is_empty() {
                for sr in &srefs {
                    let mut parts = sr.split('/');
                    if let (Some(day), Some(stream), Some(seg)) =
                        (parts.next(), parts.next(), parts.next())
                    {
                        for ae in &packet.act_entries {
                            if ae.day == day
                                && ae.stream == stream
                                && ae.segments.iter().any(|s| s == seg)
                            {
                                let key = (
                                    ae.activity_ref.day.clone(),
                                    ae.activity_ref.stream.clone(),
                                    ae.activity_ref.id.clone(),
                                );
                                if seen_a.insert(key) {
                                    arefs.push(ae.activity_ref.clone());
                                }
                                break;
                            }
                        }
                    }
                }
            }

            // Deterministically clamp moment started_at..ended_at to
            // referenced segment_refs / activity_refs bounds.
            let raw_start = m
                .get("started_at")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim();
            let raw_end = m
                .get("ended_at")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim();
            let (started_at, ended_at) = if let Some((min_b, max_b)) =
                moment_reference_bounds_minutes(&srefs, &arefs, packet)
            {
                let ps = parse_wall_minutes(raw_start);
                let pe = parse_wall_minutes(raw_end);
                let cs = ps.map_or(min_b, |v| v.clamp(min_b, max_b));
                let ce = pe.map_or(max_b, |v| v.clamp(cs, max_b));
                if ps != Some(cs) || pe != Some(ce) {
                    audit.clamped_moment_timestamps += 1;
                }
                (format_wall_minutes(cs), format_wall_minutes(ce))
            } else {
                (raw_start.to_owned(), raw_end.to_owned())
            };

            let mid = m
                .get("id")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .unwrap_or_else(|| format!("m{}", idx + 1));

            clean_moments.push(ExperienceMoment {
                id: mid,
                started_at,
                ended_at,
                summary: msum,
                activity_refs: arefs,
                segment_refs: srefs,
                media_refs: mrefs,
            });
        }
    }

    let joined = all_prose.join("\n");
    let violations = inspect_prose_violations(&joined);
    let dup_title = title_duplicates_summary_opening(&title, &summary);

    let mut retry_reasons = Vec::new();
    if !violations.second_person.is_empty() {
        let uniq: BTreeSet<&str> = violations
            .second_person
            .iter()
            .map(String::as_str)
            .collect();
        retry_reasons.push(format!(
            "unquoted 2nd-person pronouns: {}",
            uniq.into_iter().collect::<Vec<_>>().join(", ")
        ));
    }
    if !violations.first_person.is_empty() {
        let uniq: BTreeSet<&str> = violations.first_person.iter().map(String::as_str).collect();
        retry_reasons.push(format!(
            "unquoted 1st-person pronouns: {}",
            uniq.into_iter().collect::<Vec<_>>().join(", ")
        ));
    }
    if !violations.speaker_literal.is_empty() {
        retry_reasons.push(format!(
            "literal Speaker N tokens: {}",
            violations.speaker_literal.join(", ")
        ));
    }
    if !violations.verbatim_dump.is_empty() {
        retry_reasons.push("verbatim multi-speaker transcript block".to_owned());
    }
    if !violations.sensitive_leak.is_empty() {
        retry_reasons.push("sensitive credential, code, address, or order ID".to_owned());
    }
    if !violations.raw_stream_tokens.is_empty() {
        let uniq: BTreeSet<&str> = violations
            .raw_stream_tokens
            .iter()
            .map(String::as_str)
            .collect();
        retry_reasons.push(format!(
            "internal channel token leaked into prose: {}",
            uniq.into_iter().collect::<Vec<_>>().join(", ")
        ));
    }
    if dup_title {
        retry_reasons.push("title repeats summary opening sentence".to_owned());
    }

    audit.violations = violations;
    audit.title_duplicates_summary = dup_title;
    audit.needs_retry = !retry_reasons.is_empty();
    audit.retry_reasons = retry_reasons;

    let composed = ComposedExperienceUpdate {
        title,
        summary,
        locations: clean_locations,
        entity_refs: clean_people,
        moments: clean_moments,
        started_at: None,
        ended_at: None,
    };
    (composed, audit)
}

/// Build a concise 1-shot corrective retry instruction when post-hooks fire.
pub fn build_corrective_retry_suffix(audit: &ComposeAudit, owner_preferred: &str) -> String {
    format!(
        "\n\nCORRECTION REQUIRED — your previous JSON draft violated mandatory rules:\n- {}\n\nRegenerate the complete JSON object fixing every issue above. Write strictly in third person using '{}' (no unquoted you/your/I/my/we/our), no Speaker N labels, no verbatim '>' transcript dumps, no raw internal stream names (use natural words like notes/photo/terminal/screenshot), no sensitive codes/addresses/order IDs, and ensure `title` is a short heading distinct from `summary`'s opening sentence.",
        audit.retry_reasons.join("\n- "),
        owner_preferred
    )
}

/// Final safe deterministic repair applied ONLY if a 1-shot retry still leaves
/// mechanical leaks (raw internal channel tokens, sensitive patterns, or
/// literal `Speaker N`).
pub fn apply_deterministic_repair(composed: &mut ComposedExperienceUpdate) {
    let clean_text = |input: &str| -> String {
        let s1 = re_credential_or_sensitive().replace_all(input, "[redacted]");
        let s2 = re_speaker_literal().replace_all(&s1, "an attendee");
        let s3 = re_raw_stream_token().replace_all(&s2, |caps: &regex::Captures<'_>| {
            humanize_stream_label(&caps[1])
        });
        collapse_whitespace(&s3)
    };
    composed.title = clean_text(&composed.title);
    composed.summary = clean_text(&composed.summary);
    for m in &mut composed.moments {
        m.summary = clean_text(&m.summary);
    }
    if title_duplicates_summary_opening(&composed.title, &composed.summary) {
        let words: Vec<&str> = composed.title.split_whitespace().take(6).collect();
        if words.len() >= 3 {
            composed.title = words.join(" ");
        }
    }
}

// ---------------------------------------------------------------------------
// Event-driven hook stage wiring (`experience_compose`)
// ---------------------------------------------------------------------------

pub fn build_compose(
    prepared: &mut PreparedTalent,
    context: &ExecutionContext,
) -> Result<PrePostState, RuntimeOutcome> {
    let day = prepared
        .config
        .get("day")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            RuntimeOutcome::StageFailed(stage_error(
                "build",
                "experience_compose",
                prepared,
                "missing anchor day",
            ))
        })?;
    let facet = prepared
        .config
        .get("facet")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            RuntimeOutcome::StageFailed(stage_error(
                "build",
                "experience_compose",
                prepared,
                "missing facet",
            ))
        })?;
    let exp_id = prepared
        .config
        .get("experience_id")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            RuntimeOutcome::StageFailed(stage_error(
                "build",
                "experience_compose",
                prepared,
                "missing experience_id",
            ))
        })?;
    let Some(experience) =
        solstone_core_facets::get_experience(&context.journal, facet, day, exp_id).map_err(
            |e| {
                RuntimeOutcome::StageFailed(stage_error(
                    "build",
                    "experience_compose",
                    prepared,
                    e.to_string(),
                ))
            },
        )?
    else {
        return Err(RuntimeOutcome::Skipped {
            stage: "build".to_owned(),
            talent: prepared.name.clone(),
            reason: "no_candidates".to_owned(),
        });
    };
    if experience.hidden || experience.activities.is_empty() {
        return Err(RuntimeOutcome::Skipped {
            stage: "build".to_owned(),
            talent: prepared.name.clone(),
            reason: "no_candidates".to_owned(),
        });
    }
    let max_output = prepared
        .config
        .get("max_output_tokens")
        .and_then(Value::as_u64)
        .and_then(|v| u32::try_from(v).ok())
        .unwrap_or(2048);
    let overhead = estimate_prompt_overhead(&prepared.config);
    let packet = build_composer_packet(&context.journal, &experience, max_output, overhead)
        .map_err(|e| {
            RuntimeOutcome::StageFailed(stage_error("build", "experience_compose", prepared, e))
        })?;
    if packet.act_entries.is_empty() {
        return Err(RuntimeOutcome::Skipped {
            stage: "build".to_owned(),
            talent: prepared.name.clone(),
            reason: "no_candidates".to_owned(),
        });
    }
    Ok(PrePostState::ExperienceCompose(Box::new(ComposePreState {
        packet,
    })))
}

pub fn apply_prompt_override_compose(
    prepared: &mut PreparedTalent,
    state: &PrePostState,
) -> Result<(), StageError> {
    let PrePostState::ExperienceCompose(st) = state else {
        return Err(stage_error(
            "prompt_override",
            "experience_compose",
            prepared,
            "missing compose state",
        ));
    };
    apply_template_vars(
        &mut prepared.config,
        &Map::from_iter([(
            "experience_packet".to_owned(),
            Value::String(st.packet.prompt_text.clone()),
        )]),
    );
    Ok(())
}

pub fn parse_compose(
    output: &str,
    prepared: &PreparedTalent,
    _state: &PrePostState,
) -> Result<ParsedOutput, StageError> {
    let val: Value = serde_json::from_str(output).map_err(|e| {
        stage_error(
            "parse",
            "experience_compose",
            prepared,
            format!("invalid compose JSON: {e}"),
        )
    })?;
    if !val.get("title").is_some_and(Value::is_string)
        || !val.get("summary").is_some_and(Value::is_string)
        || !val.get("moments").is_some_and(Value::is_array)
    {
        return Err(stage_error(
            "parse",
            "experience_compose",
            prepared,
            "compose JSON missing required fields",
        ));
    }
    Ok(ParsedOutput::Json(val))
}

pub fn commit_compose(
    parsed: ParsedOutput,
    prepared: &PreparedTalent,
    state: &PrePostState,
) -> Result<CommitPlan, StageError> {
    let ParsedOutput::Json(val) = parsed else {
        return Err(stage_error(
            "commit",
            "experience_compose",
            prepared,
            "expected JSON output for compose",
        ));
    };
    let PrePostState::ExperienceCompose(st) = state else {
        return Err(stage_error(
            "commit",
            "experience_compose",
            prepared,
            "missing compose state",
        ));
    };
    let (mut composed, audit) = postprocess_compose_output(&val, &st.packet);
    if audit.needs_retry {
        apply_deterministic_repair(&mut composed);
    }
    Ok(CommitPlan::Write(WriteIntent::ExperienceCompose {
        facet: st.packet.primary_facet.clone(),
        anchor_day: st.packet.anchor_day.clone(),
        experience_id: st.packet.experience_id.clone(),
        composed: Box::new(composed),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn locations_grounding_drops_ungrounded_venue_and_keeps_grounded_place() {
        let corpus = "Alex met Maya for lunch at Riverside Cafe and walked along River Park.";
        assert!(is_location_grounded("Riverside Cafe", corpus));
        assert!(is_location_grounded("River Park", corpus));
        assert!(!is_location_grounded("Downtown Theater", corpus));
        assert!(!is_location_grounded("1234 Oak Street", corpus));
    }

    #[test]
    fn moment_timestamps_clamp_to_referenced_segment_bounds() {
        let packet = ComposePacket {
            experience_id: "exp_20260304_demo".to_owned(),
            primary_facet: "personal".to_owned(),
            anchor_day: "20260304".to_owned(),
            owner: OwnerIdentity {
                full_name: "Alex Rivera".to_owned(),
                preferred: "Alex".to_owned(),
                entity_id: "alex".to_owned(),
                aliases: vec!["alex".to_owned(), "alex rivera".to_owned()],
            },
            act_entries: vec![ComposerActivityEntry {
                handle: "ACT1".to_owned(),
                activity_ref: ActivityRef {
                    day: "20260304".to_owned(),
                    stream: "watch".to_owned(),
                    id: "social_180000_600".to_owned(),
                    facet: None,
                },
                day: "20260304".to_owned(),
                start_s: 18 * 3600,
                end_s: 18 * 3600 + 600,
                start_hm: "18:00".to_owned(),
                end_hm: "18:10".to_owned(),
                duration_min: 10,
                stream: "watch".to_owned(),
                channel_label: "audio".to_owned(),
                activity: "social".to_owned(),
                segments: vec!["180000_600".to_owned()],
                description: "Dinner at Riverside Cafe.".to_owned(),
                has_story: true,
                secondary_screen_context: false,
                story_or_fallback: "Alex met Maya at Riverside Cafe.".to_owned(),
                topics: vec!["dinner".to_owned()],
                segment_notes: vec![],
                media_items: vec![],
            }],
            valid_act_handles: BTreeMap::from([(
                "ACT1".to_owned(),
                ActivityRef {
                    day: "20260304".to_owned(),
                    stream: "watch".to_owned(),
                    id: "social_180000_600".to_owned(),
                    facet: None,
                },
            )]),
            valid_seg_refs: BTreeSet::from(["20260304/watch/180000_600".to_owned()]),
            valid_media_refs: BTreeSet::new(),
            corroborated_people: vec![CorroboratedPerson {
                entity_id: "maya".to_owned(),
                name: "Maya".to_owned(),
                roles: vec!["attendee".to_owned()],
                sources: vec!["voice".to_owned()],
                mentions: 1,
            }],
            grounding_corpus: "Alex met Maya at Riverside Cafe.".to_owned(),
            prompt_text: String::new(),
        };

        let raw = json!({
            "title": "Dinner with Maya",
            "summary": "Alex caught up with Maya at Riverside Cafe.",
            "moments": [{
                "id": "m1",
                "started_at": "2026-03-04 17:15",
                "ended_at": "2026-03-04 20:30",
                "summary": "Alex arrived and spoke with Maya at Riverside Cafe.",
                "activity_handles": ["ACT1"],
                "segment_refs": ["20260304/watch/180000_600"],
                "media_refs": []
            }],
            "people_refs": ["maya", "alex", "ghost"],
            "locations": ["Riverside Cafe", "Unrelated Hall"]
        });

        let (composed, audit) = postprocess_compose_output(&raw, &packet);
        assert_eq!(composed.moments.len(), 1);
        assert_eq!(composed.moments[0].started_at, "2026-03-04 18:00");
        assert_eq!(composed.moments[0].ended_at, "2026-03-04 18:10");
        assert_eq!(audit.clamped_moment_timestamps, 1);
        assert_eq!(composed.entity_refs, vec!["maya"]);
        assert_eq!(composed.locations, vec!["Riverside Cafe"]);
        assert_eq!(audit.ungrounded_locations_dropped, 1);
        assert!(!audit.needs_retry);
    }

    #[test]
    fn hard_posthooks_flag_pronouns_stream_tokens_and_repair() {
        let violations = inspect_prose_violations(
            "You checked obsidian while Speaker 1 told you gate code 4829.",
        );
        assert!(!violations.second_person.is_empty());
        assert!(!violations.speaker_literal.is_empty());
        assert!(!violations.sensitive_leak.is_empty());
        assert_eq!(violations.raw_stream_tokens, vec!["obsidian"]);

        let mut comp = ComposedExperienceUpdate {
            title: "Notes check".to_owned(),
            summary: "Alex checked obsidian notes with Speaker 1 using gate code 4829.".to_owned(),
            locations: vec![],
            entity_refs: vec![],
            moments: vec![],
            started_at: None,
            ended_at: None,
        };
        apply_deterministic_repair(&mut comp);
        assert!(!comp.summary.contains("obsidian"));
        assert!(comp.summary.contains("notes"));
        assert!(!comp.summary.contains("Speaker 1"));
        assert!(!comp.summary.contains("4829"));
    }
}
