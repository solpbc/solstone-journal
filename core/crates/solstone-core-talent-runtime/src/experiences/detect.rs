// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Two-stage daily Experience detector (`experiences_detect_facet` Stage 1A +
//! `experiences_detect` Stage 1B) with deterministic `D1` solo pre-gate,
//! `D2` `>= 90m` + shift splitter, `B1` single-facet enforcement, and `A1`
//! cross-facet segment-overlap arbitration.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use solstone_core_facets::ActivityRef;
use solstone_core_journal_io::contained_path;
use solstone_core_journal_io::durability::ArtifactId;
use solstone_core_local::generate::compute_input_budget;
use solstone_core_local::plan::LOCAL_MIN_CONTEXT_TOKENS;

use crate::contract::{CommitPlan, ParsedOutput, PrePostState};
use crate::writers::WriteIntent;
use crate::{
    ExecutionContext, PreparedTalent, RuntimeOutcome, StageError, apply_template_vars, stage_error,
};

use super::prefilter::{
    DayPool, OpenPrevExperience, PoolItem, format_iso_day, humanize_stream_label,
    load_day_multi_facet_pool, truncate_chars,
};

/// Path under `chronicle/<day>/health/experience-candidates/<facet>.json`.
pub fn stage1a_candidate_path(journal: &Path, day: &str, facet: &str) -> Result<PathBuf, String> {
    let _artifact = ArtifactId::ExperienceCandidates;
    contained_path(
        journal,
        &format!("chronicle/{day}/health/experience-candidates/{facet}.json"),
    )
    .map_err(|e| e.to_string())
}

/// Directory holding Stage 1A per-facet candidate JSON files for `day`.
pub fn stage1a_candidates_dir(journal: &Path, day: &str) -> Result<PathBuf, String> {
    let _artifact = ArtifactId::ExperienceCandidates;
    contained_path(
        journal,
        &format!("chronicle/{day}/health/experience-candidates"),
    )
    .map_err(|e| e.to_string())
}

/// One Stage 1A candidate Experience persisted for Stage 1B arbitration.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DetectedCandidate {
    #[serde(default)]
    pub cand_id: String,
    pub primary_facet: String,
    pub anchor_day: String,
    pub topic_anchor: String,
    pub title_hint: String,
    pub category: String,
    #[serde(default)]
    pub extends_experience_id: Option<String>,
    pub started_at: String,
    pub ended_at: String,
    pub activities: Vec<ActivityRef>,
    pub handles: Vec<String>,
    pub items: Vec<PoolItem>,
}

/// Persisted Stage 1A per-facet file envelope.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stage1ACandidateFile {
    pub version: u32,
    pub day: String,
    pub facet: String,
    pub candidates: Vec<DetectedCandidate>,
}

/// Frozen pre-state for `experiences_detect_facet` (Stage 1A).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stage1APreState {
    pub day: String,
    pub facet: String,
    pub facet_digest: String,
    pub pool: DayPool,
    pub candidate_path: String,
}

/// Frozen pre-state for `experiences_detect` (Stage 1B).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stage1BPreState {
    pub day: String,
    pub candidates_packet: String,
    pub candidates: Vec<DetectedCandidate>,
}

/// Deterministic audit counts for Stage 1A post-processing.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stage1AAudit {
    pub raw_count: usize,
    pub invalid_handles: usize,
    pub cross_facet_handles_b1: usize,
    pub duplicate_handles: usize,
    pub prev_day_false_merge: usize,
    pub d1_solo_dropped: usize,
    pub d2_splits: usize,
    pub standalone_media_dropped: usize,
}

/// Deterministic audit counts for Stage 1B arbitration.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stage1BAudit {
    pub total_candidates: usize,
    pub kept: usize,
    pub dropped_duplicate_cross_facet: usize,
    pub dropped_passive_spectacle: usize,
    pub dropped_routine_or_trivial: usize,
    pub rescued_non_overlapping_dup: usize,
    pub rescued_mutual_dup_pair: usize,
    pub suppressed_a1_backstop: usize,
}

/// Render a single-facet Stage 1A digest generically fitted to the 32k floor budget.
pub fn render_stage1a_facet_digest(
    pool: &DayPool,
    facet: &str,
    open_prev: &[OpenPrevExperience],
    max_output_tokens: u32,
    overhead_tokens: u32,
) -> Option<(String, Vec<PoolItem>)> {
    let fmeta = pool.facets.get(facet)?;
    let cur_items: Vec<&PoolItem> = pool
        .items
        .iter()
        .filter(|it| it.facet == facet && !it.is_prev_evening)
        .collect();
    if cur_items.is_empty() {
        return None;
    }
    let has_early_cur = cur_items.iter().any(|it| it.start_s < 3 * 3600);
    let prev_items: Vec<&PoolItem> = if has_early_cur {
        pool.items
            .iter()
            .filter(|it| it.facet == facet && it.is_prev_evening)
            .collect()
    } else {
        Vec::new()
    };

    const TARGET_STAGE1A_DIGEST_TOKENS: u32 = 5_500;
    let budget = compute_input_budget(max_output_tokens, LOCAL_MIN_CONTEXT_TOKENS)
        .saturating_sub(overhead_tokens)
        .clamp(2048, TARGET_STAGE1A_DIGEST_TOKENS);

    // Progressively shrink per-item summary caps until the digest fits the generic 32k floor.
    let total_rows = prev_items.len() + cur_items.len();
    let initial_cap = if total_rows <= 25 {
        260
    } else if total_rows <= 45 {
        190
    } else {
        140
    };
    let caps = [initial_cap, 120, 85, 55, 35];
    let mut rendered = String::new();

    for &base_cap in &caps {
        let mut lines = vec![
            format!(
                "Target day: {} | Facet: {} — {}",
                format_iso_day(&pool.day),
                facet,
                truncate_chars(&fmeta.description, 140)
            ),
            format!("Owner: {} ({})", pool.owner.preferred, pool.owner.full_name),
        ];
        if !prev_items.is_empty() {
            let rel_open: Vec<&OpenPrevExperience> = open_prev
                .iter()
                .filter(|ex| ex.primary_facet == facet)
                .collect();
            if !rel_open.is_empty() {
                lines.push(
                    "Open late-evening Experiences from previous day (set extends_experience_id if 00:00-03:00 continues):"
                        .to_owned(),
                );
                for ex in rel_open {
                    lines.push(format!(
                        "  - id={} span={}..{} title={:?}",
                        ex.id, ex.started_at, ex.ended_at, ex.title
                    ));
                }
            }
        }
        let fmt_item = |it: &PoolItem| -> String {
            let mut flags = Vec::new();
            if it.is_prev_evening {
                flags.push(format!("PREV-EVE {}", format_iso_day(&it.day)));
            }
            if it.has_media {
                flags.push("photo/media".to_owned());
            }
            if !it.has_story {
                flags.push("no_story".to_owned());
            }
            if it.playback == Some(true) {
                flags.push("playback=true".to_owned());
            }
            if !it.cross_facet_twins.is_empty() {
                flags.push(format!("x-facet:{}", it.cross_facet_twins.join(",")));
            }
            let fl_str = if flags.is_empty() {
                String::new()
            } else {
                format!(" [{}]", flags.join("; "))
            };
            let ppl = if it.people.is_empty() {
                String::new()
            } else {
                format!(
                    " | ppl:{}",
                    it.people
                        .iter()
                        .take(5)
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(",")
                )
            };
            let cap = if it.segments.len() == 1 && it.duration_min <= 5 && !it.has_media {
                base_cap.min(100)
            } else {
                base_cap
            };
            let summ = truncate_chars(&it.summary, cap);
            let channel = humanize_stream_label(&it.stream);
            format!(
                "{} {}-{}({}m,{}s) {}/{}{}{}: {}",
                it.handle,
                it.start_hm,
                it.end_hm,
                it.duration_min,
                it.segments.len(),
                channel,
                it.activity,
                fl_str,
                ppl,
                summ
            )
        };
        if !prev_items.is_empty() {
            lines.push(format!(
                "\n--- Previous evening ({} >= 20:00; include ONLY if 00:00-03:00 continues it) ---",
                format_iso_day(&pool.prev_day)
            ));
            for it in &prev_items {
                lines.push(fmt_item(it));
            }
        }
        lines.push(format!(
            "\n--- Today's activities in facet '{}' ({} rows) ---",
            facet,
            cur_items.len()
        ));
        for it in &cur_items {
            lines.push(fmt_item(it));
        }
        rendered = lines.join("\n");
        if solstone_core_tokens::count(&rendered) <= budget {
            break;
        }
    }

    // Final hard floor safety bound if a day has hundreds of rows.
    while solstone_core_tokens::count(&rendered) > budget && rendered.len() > 2048 {
        let target = rendered.len() * 3 / 4;
        let cut = rendered
            .char_indices()
            .take_while(|(i, _)| *i <= target)
            .last()
            .map_or(0, |(i, _)| i);
        rendered.truncate(cut);
        rendered.push_str("\n[truncated to context floor]");
    }

    let included: Vec<PoolItem> = prev_items.into_iter().chain(cur_items).cloned().collect();
    Some((rendered, included))
}

fn abs_seconds(it: &PoolItem) -> (i64, i64) {
    let base = if it.is_prev_evening { 0 } else { 86_400 };
    (base + it.start_s, base + it.end_s)
}

fn item_participant_set(it: &PoolItem) -> BTreeSet<String> {
    if !it.corroborated_entity_ids.is_empty() {
        return it.corroborated_entity_ids.iter().cloned().collect();
    }
    it.people
        .iter()
        .filter(|p| !p.ends_with('?'))
        .map(|p| p.trim().to_lowercase())
        .collect()
}

fn channel_mode(it: &PoolItem) -> &'static str {
    let ch = humanize_stream_label(&it.stream);
    let act = it.activity.to_lowercase();
    if matches!(act.as_str(), "meeting" | "call" | "conversation" | "social") || ch == "audio" {
        "interpersonal"
    } else if matches!(
        ch.as_str(),
        "terminal" | "screen" | "browser" | "notes" | "document"
    ) {
        "computer"
    } else if matches!(ch.as_str(), "photo" | "screenshot") {
        "media"
    } else {
        "other"
    }
}

fn extract_place_tokens(it: &PoolItem) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for h in &it.location_hints {
        let low = h.trim().to_lowercase();
        if !low.is_empty() {
            out.insert(low);
        }
    }
    out
}

/// Deterministic `D2` splitter: splits across any internal gap `> 135m` AND
/// across any `>= 90m` gap paired with a participant, channel/mode, or location shift.
pub fn split_experience_by_d2_gap_and_shift(items: &[PoolItem]) -> Vec<Vec<PoolItem>> {
    if items.len() <= 1 {
        return vec![items.to_vec()];
    }
    let mut ordered = items.to_vec();
    ordered.sort_by(|a, b| {
        a.day
            .cmp(&b.day)
            .then_with(|| a.start_s.cmp(&b.start_s))
            .then_with(|| a.id.cmp(&b.id))
    });

    let mut groups: Vec<Vec<PoolItem>> = vec![vec![ordered[0].clone()]];
    let mut prev_end_abs = abs_seconds(&ordered[0]).1;

    for idx in 1..ordered.len() {
        let cur = &ordered[idx];
        let (cur_start_abs, cur_end_abs) = abs_seconds(cur);
        let gap_min = (cur_start_abs - prev_end_abs) as f64 / 60.0;

        let should_split = if gap_min > 135.0 {
            true
        } else if gap_min >= 90.0 {
            let prev_group = groups.last().unwrap();
            // Look ahead slightly (within 30m of `cur`) so a lone photo does not mask the next scene.
            let next_window: Vec<&PoolItem> = ordered[idx..]
                .iter()
                .take_while(|other| {
                    let (os, _) = abs_seconds(other);
                    os - cur_start_abs <= 30 * 60
                })
                .collect();

            let prev_people: BTreeSet<String> =
                prev_group.iter().flat_map(item_participant_set).collect();
            let next_people: BTreeSet<String> = next_window
                .iter()
                .flat_map(|it| item_participant_set(it))
                .collect();
            let participant_shift = (prev_people.is_empty() != next_people.is_empty())
                || (!prev_people.is_empty()
                    && !next_people.is_empty()
                    && prev_people.is_disjoint(&next_people));

            let prev_modes: BTreeSet<&'static str> = prev_group
                .iter()
                .map(channel_mode)
                .filter(|m| *m != "media")
                .collect();
            let next_modes: BTreeSet<&'static str> = next_window
                .iter()
                .map(|it| channel_mode(it))
                .filter(|m| *m != "media")
                .collect();
            let mode_shift = !prev_modes.is_empty()
                && !next_modes.is_empty()
                && prev_modes.is_disjoint(&next_modes);

            let prev_acts: BTreeSet<String> = prev_group
                .iter()
                .map(|it| it.activity.to_lowercase())
                .filter(|a| !a.is_empty() && a != "media")
                .collect();
            let next_acts: BTreeSet<String> = next_window
                .iter()
                .map(|it| it.activity.to_lowercase())
                .filter(|a| !a.is_empty() && a != "media")
                .collect();
            let act_shift =
                !prev_acts.is_empty() && !next_acts.is_empty() && prev_acts.is_disjoint(&next_acts);

            let prev_places: BTreeSet<String> =
                prev_group.iter().flat_map(extract_place_tokens).collect();
            let next_places: BTreeSet<String> = next_window
                .iter()
                .flat_map(|it| extract_place_tokens(it))
                .collect();
            let place_shift = !prev_places.is_empty()
                && !next_places.is_empty()
                && prev_places.is_disjoint(&next_places);

            participant_shift || mode_shift || act_shift || place_shift
        } else {
            false
        };

        if should_split {
            groups.push(vec![cur.clone()]);
            prev_end_abs = cur_end_abs;
        } else {
            groups.last_mut().unwrap().push(cur.clone());
            prev_end_abs = prev_end_abs.max(cur_end_abs);
        }
    }
    groups
}

const DELIVERABLE_MARKERS: &[&str] = &[
    "submit",
    "submitted",
    "submission",
    "proposal",
    "published",
    "publish",
    "shipped",
    "shipping",
    "release",
    "released",
    "merged",
    "deploy",
    "deployed",
    "migration",
    "migrated",
    "pitch deck",
    "investor deck",
    "deck review",
    "manuscript",
    "drafted",
    "chapter",
    "conference talk",
    "presentation",
    "completed",
    "milestone",
    "launch",
    "launched",
    "specification",
    "article",
    "essay",
    "blog post",
    "announcement",
    "contract",
    "filing",
    "application",
    "deliverable",
    "architecture",
    "rehearsal",
    "premiere",
    "performance",
    "scene draft",
    "wrote",
    "finished",
];

const ROUTINE_SOLO_MARKERS: &[&str] = &[
    "status check",
    "agent status",
    "routine",
    "inbox triage",
    "folder review",
    "slate check",
    "cue slip",
    "cue-slip",
    "single ledger",
    "ledger entry",
    "housekeeping",
    "micro-check",
    "quick check",
    "password check",
    "login check",
    "banking check",
    "medical check",
    "optimization",
    "cleanup",
];

fn contains_phrase(haystack: &str, phrase: &str) -> bool {
    if phrase.contains(' ') || phrase.contains('-') {
        return haystack.contains(phrase);
    }
    haystack
        .split(|c: char| !c.is_ascii_alphanumeric())
        .any(|tok| tok == phrase)
}

/// Deterministic `D1` solo pre-gate: enforces a concrete deliverable/milestone bar
/// for pure solo candidates (zero corroborated humans, zero social/meeting/outing
/// activity signals) before Stage 1B.
pub fn passes_d1_solo_pregate(
    items: &[PoolItem],
    category: &str,
    title_hint: &str,
    topic_anchor: &str,
) -> bool {
    let has_human_participants = items.iter().any(|it| {
        !it.corroborated_entity_ids.is_empty() || it.people.iter().any(|p| !p.ends_with('?'))
    });
    if has_human_participants {
        return true;
    }

    let has_world_or_social_activity = items.iter().any(|it| {
        matches!(
            it.activity.to_lowercase().as_str(),
            "social"
                | "meeting"
                | "call"
                | "conversation"
                | "outing"
                | "errand"
                | "meal"
                | "dining"
                | "travel"
                | "rehearsal"
                | "performance"
        )
    });
    if has_world_or_social_activity {
        return true;
    }

    // Pure solo candidate: check duration, channel diversity, and deliverable markers.
    let total_acts = items.len();
    let total_segs: usize = items.iter().map(|it| it.segments.len()).sum();
    let total_dur: i64 = items.iter().map(|it| it.duration_min).sum();
    let channels: BTreeSet<String> = items
        .iter()
        .map(|it| humanize_stream_label(&it.stream))
        .collect();
    let has_media = items.iter().any(|it| it.has_media);

    let mut corpus = format!("{title_hint} {topic_anchor}");
    for it in items {
        corpus.push(' ');
        corpus.push_str(&it.description);
        corpus.push(' ');
        corpus.push_str(&it.summary);
        for t in &it.topics {
            corpus.push(' ');
            corpus.push_str(t);
        }
    }
    let low = corpus.to_lowercase();

    let has_deliverable = DELIVERABLE_MARKERS
        .iter()
        .any(|marker| contains_phrase(&low, marker));
    let has_routine_only = ROUTINE_SOLO_MARKERS
        .iter()
        .any(|marker| contains_phrase(&low, marker));

    // Tiny 1-segment micro-activities without a deliverable never clear the solo bar.
    if total_acts == 1 && total_segs <= 1 && total_dur <= 5 && !has_deliverable {
        return false;
    }

    // Pure computer/desk/terminal/browser solo work requires explicit deliverable evidence.
    let pure_computer = items.iter().all(|it| {
        matches!(
            humanize_stream_label(&it.stream).as_str(),
            "terminal" | "screen" | "browser" | "mobile"
        )
    });
    if pure_computer {
        if has_routine_only && !has_deliverable {
            return false;
        }
        if has_deliverable {
            return true;
        }
        return false;
    }

    // Outing/errand category with non-computer channels (e.g., audio/photo walk or errand).
    if category == "outing_or_errand" && (total_dur >= 10 || total_acts >= 2 || has_media) {
        return !has_routine_only || has_deliverable;
    }

    // Solo creative/writing/notes work: require deliverable marker OR multi-channel artifact arc.
    if has_deliverable && !has_routine_only {
        return true;
    }
    if total_acts >= 2 && channels.len() >= 2 && !has_routine_only {
        return true;
    }
    false
}

fn derive_split_title_hint(subgroup: &[PoolItem], fallback: &str) -> String {
    // Pick the most substantive non-media activity in the subgroup for a crisp hint.
    let best = subgroup
        .iter()
        .filter(|it| {
            !matches!(
                humanize_stream_label(&it.stream).as_str(),
                "photo" | "screenshot"
            )
        })
        .max_by_key(|it| (it.segments.len(), it.duration_min))
        .or_else(|| subgroup.first());
    let Some(item) = best else {
        return fallback.to_owned();
    };
    let raw = if !item.description.trim().is_empty() {
        item.description.trim().to_owned()
    } else {
        item.summary
            .trim()
            .trim_start_matches("[storyless]")
            .trim()
            .to_owned()
    };
    if raw.is_empty() {
        return fallback.to_owned();
    }
    let first_clause = raw
        .split(['.', '!', '?', '\n'])
        .next()
        .unwrap_or(&raw)
        .trim();
    let words: Vec<&str> = first_clause.split_whitespace().take(8).collect();
    if words.is_empty() {
        fallback.to_owned()
    } else {
        truncate_chars(&words.join(" "), 72)
    }
}

/// Deterministic Stage 1A post-processor enforcing single-facet scope,
/// cross-midnight `< 03:00` continuity, no lone photo/short media,
/// gap-and-shift episode splitting (`>= 90m` + shift / `> 135m` cap), and
/// solo-work milestone pre-gating.
pub fn postprocess_stage1a_facet(
    raw_output: &Value,
    pool: &DayPool,
    facet: &str,
) -> (Vec<DetectedCandidate>, Stage1AAudit) {
    let mut audit = Stage1AAudit::default();
    let mut used: BTreeSet<(String, String, String)> = BTreeSet::new();
    let mut candidates = Vec::new();

    let experiences = raw_output
        .get("experiences")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    audit.raw_count = experiences.len();

    for exp in experiences {
        let topic_anchor = exp
            .get("topic_anchor")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_owned();
        let title_hint = exp
            .get("title_hint")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_owned();
        let category = exp
            .get("category")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_owned();
        let extends_experience_id = exp
            .get("extends_experience_id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned);

        let mut kept: Vec<PoolItem> = Vec::new();
        let mut seen_local: BTreeSet<(String, String, String)> = BTreeSet::new();
        if let Some(handles) = exp.get("activity_handles").and_then(Value::as_array) {
            for h_val in handles {
                let Some(h) = h_val.as_str().map(str::trim) else {
                    audit.invalid_handles += 1;
                    continue;
                };
                let Some(it) = pool.find_by_handle(h) else {
                    audit.invalid_handles += 1;
                    continue;
                };
                if it.facet != facet {
                    audit.cross_facet_handles_b1 += 1;
                    continue;
                }
                let k_full = (
                    it.day.clone(),
                    facet.to_owned(),
                    it.stream.clone(),
                    it.id.clone(),
                );
                let k_short = (it.day.clone(), it.stream.clone(), it.id.clone());
                if pool.excluded_keys.contains(&k_full) || used.contains(&k_short) {
                    audit.duplicate_handles += 1;
                    continue;
                }
                if !seen_local.insert(k_short) {
                    continue;
                }
                kept.push(it.clone());
            }
        }
        if kept.is_empty() {
            continue;
        }

        // Cross-midnight guard: prev_evening rows only allowed if target day has row < 03:00
        // within <= 120 minutes of prev_evening end.
        let cur_part: Vec<PoolItem> = kept
            .iter()
            .filter(|it| !it.is_prev_evening)
            .cloned()
            .collect();
        let prev_part: Vec<PoolItem> = kept
            .iter()
            .filter(|it| it.is_prev_evening)
            .cloned()
            .collect();
        if cur_part.is_empty() {
            continue;
        }
        if !prev_part.is_empty() {
            let early_cur: Vec<&PoolItem> =
                cur_part.iter().filter(|it| it.start_s < 3 * 3600).collect();
            if early_cur.is_empty() {
                audit.prev_day_false_merge += prev_part.len();
                kept = cur_part;
            } else {
                let min_early = early_cur.iter().map(|it| it.start_s).min().unwrap() + 86_400;
                let max_prev_end = prev_part.iter().map(|it| it.end_s).max().unwrap();
                if min_early - max_prev_end > 120 * 60 {
                    audit.prev_day_false_merge += prev_part.len();
                    kept = cur_part;
                }
            }
        }

        // Deterministic gap-and-shift splitter (>= 90m + shift, or > 135m hard cap)
        let subgroups = split_experience_by_d2_gap_and_shift(&kept);
        let was_split = subgroups.len() > 1;
        if was_split {
            audit.d2_splits += subgroups.len() - 1;
        }

        for (sg_idx, mut sg) in subgroups.into_iter().enumerate() {
            if sg.iter().all(|it| it.is_prev_evening) {
                continue;
            }
            if sg.len() == 1 {
                let only = &sg[0];
                if matches!(only.stream.as_str(), "import.image" | "ios_screenshots")
                    || (only.activity == "media" && only.segments.len() <= 3)
                {
                    audit.standalone_media_dropped += 1;
                    continue;
                }
            }
            let effective_title = if was_split {
                derive_split_title_hint(&sg, &title_hint)
            } else if !title_hint.is_empty() {
                title_hint.clone()
            } else {
                derive_split_title_hint(&sg, &topic_anchor)
            };

            // Deterministic solo-work milestone pre-gate
            if !passes_d1_solo_pregate(&sg, &category, &effective_title, &topic_anchor) {
                audit.d1_solo_dropped += 1;
                continue;
            }

            sg.sort_by(|a, b| {
                a.day
                    .cmp(&b.day)
                    .then_with(|| a.start_s.cmp(&b.start_s))
                    .then_with(|| a.id.cmp(&b.id))
            });
            for it in &sg {
                used.insert((it.day.clone(), it.stream.clone(), it.id.clone()));
            }
            let first = &sg[0];
            let last = &sg[sg.len() - 1];
            candidates.push(DetectedCandidate {
                cand_id: String::new(),
                primary_facet: facet.to_owned(),
                anchor_day: first.day.clone(),
                topic_anchor: topic_anchor.clone(),
                title_hint: effective_title,
                category: category.clone(),
                extends_experience_id: if sg_idx == 0 {
                    extends_experience_id.clone()
                } else {
                    None
                },
                started_at: format!("{}T{}:00", format_iso_day(&first.day), first.start_hm),
                ended_at: format!("{}T{}:00", format_iso_day(&last.day), last.end_hm),
                activities: sg
                    .iter()
                    .map(|it| ActivityRef {
                        day: it.day.clone(),
                        stream: it.stream.clone(),
                        id: it.id.clone(),
                        facet: None,
                    })
                    .collect(),
                handles: sg.iter().map(|it| it.handle.clone()).collect(),
                items: sg,
            });
        }
    }

    (candidates, audit)
}

/// Render Stage 1B cross-facet arbitration packet fitted generically to the 32k floor.
pub fn render_stage1b_arbitration_packet(
    pool: &DayPool,
    candidates: &mut [DetectedCandidate],
    max_output_tokens: u32,
    overhead_tokens: u32,
) -> String {
    let mut seg_sets: BTreeMap<String, BTreeSet<(String, String, String)>> = BTreeMap::new();
    for (idx, c) in candidates.iter_mut().enumerate() {
        let cid = format!("C{:02}", idx + 1);
        c.cand_id = cid.clone();
        let set: BTreeSet<(String, String, String)> = c
            .items
            .iter()
            .flat_map(|it| {
                it.segments
                    .iter()
                    .map(move |s| (it.day.clone(), it.stream.clone(), s.clone()))
            })
            .collect();
        seg_sets.insert(cid, set);
    }

    const TARGET_STAGE1B_PACKET_TOKENS: u32 = 5_500;
    let budget = compute_input_budget(max_output_tokens, LOCAL_MIN_CONTEXT_TOKENS)
        .saturating_sub(overhead_tokens)
        .clamp(2048, TARGET_STAGE1B_PACKET_TOKENS);

    let mut rendered = String::new();
    for excerpt_cap in [130usize, 85, 50] {
        let mut lines = vec![
            format!(
                "Target day: {} | Owner: {} ({})",
                format_iso_day(&pool.day),
                pool.owner.preferred,
                pool.owner.full_name
            ),
            "Enabled facets:".to_owned(),
        ];
        for (fn_name, fm) in &pool.facets {
            lines.push(format!(
                "  - {}: {}",
                fn_name,
                truncate_chars(&fm.description, 110)
            ));
        }
        lines.push(format!(
            "\nProposed Experiences across all facets ({} candidates — issue ONE verdict per cand_id):",
            candidates.len()
        ));

        for c in candidates.iter() {
            let cid = &c.cand_id;
            let tot_segs: usize = c.items.iter().map(|it| it.segments.len()).sum();
            let tot_dur: i64 = c.items.iter().map(|it| it.duration_min).sum();
            let channels: Vec<String> = c
                .items
                .iter()
                .map(|it| humanize_stream_label(&it.stream))
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect();
            let act_types: Vec<String> = c
                .items
                .iter()
                .map(|it| it.activity.clone())
                .filter(|a| !a.is_empty())
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect();
            let mut ppl = Vec::new();
            for it in &c.items {
                for p in &it.people {
                    if !ppl.contains(p) {
                        ppl.push(p.clone());
                    }
                }
            }
            let my_segs = seg_sets.get(cid).unwrap();
            let mut ov_notes = Vec::new();
            for other in candidates.iter() {
                if other.cand_id == *cid || other.primary_facet == c.primary_facet {
                    continue;
                }
                let o_segs = seg_sets.get(&other.cand_id).unwrap();
                let seg_ov = my_segs.intersection(o_segs).count();
                if seg_ov > 0 {
                    let pct =
                        ((100 * seg_ov) as f64 / my_segs.len().max(1) as f64).round() as usize;
                    ov_notes.push(format!(
                        "{}({}: {} shared segs, {}%)",
                        other.cand_id, other.primary_facet, seg_ov, pct
                    ));
                }
            }
            let ov_str = if ov_notes.is_empty() {
                String::new()
            } else {
                format!(" | SHARED_SEGS_WITH: {}", ov_notes.join(", "))
            };
            let ppl_str = if ppl.is_empty() {
                String::new()
            } else {
                format!(
                    " | ppl:{}",
                    ppl.iter().take(5).cloned().collect::<Vec<_>>().join(",")
                )
            };
            let mut excerpts: Vec<String> = c
                .items
                .iter()
                .take(3)
                .map(|it| truncate_chars(&it.summary, excerpt_cap))
                .collect();
            if c.items.len() > 3 {
                excerpts.push(format!("(+{} more)", c.items.len() - 3));
            }
            let start_hm = c.started_at.get(11..16).unwrap_or("00:00");
            let end_hm = c.ended_at.get(11..16).unwrap_or("00:00");
            lines.push(format!(
                "\n{} | facet={} | {}-{} ({} acts, {}s, ~{}m) | cat={} | channels={} | types={}{}{}",
                cid,
                c.primary_facet,
                start_hm,
                end_hm,
                c.items.len(),
                tot_segs,
                tot_dur,
                c.category,
                channels.join(","),
                act_types.join(","),
                ppl_str,
                ov_str
            ));
            lines.push(format!("  title_hint: {}", c.title_hint));
            lines.push(format!("  substance: {}", excerpts.join(" // ")));
        }
        rendered = lines.join("\n");
        if solstone_core_tokens::count(&rendered) <= budget {
            break;
        }
    }
    rendered
}

/// Apply Stage 1B verdicts with deterministic A1 guards (non-overlapping rescue,
/// mutual-duplicate rescue, and `>= 60%` segment-overlap backstop).
pub fn postprocess_stage1b_verdicts(
    raw_output: &Value,
    candidates: &[DetectedCandidate],
) -> (Vec<DetectedCandidate>, Stage1BAudit) {
    let mut audit = Stage1BAudit {
        total_candidates: candidates.len(),
        ..Stage1BAudit::default()
    };

    let seg_sets: BTreeMap<&str, BTreeSet<(String, String, String)>> = candidates
        .iter()
        .map(|c| {
            let set = c
                .items
                .iter()
                .flat_map(|it| {
                    it.segments
                        .iter()
                        .map(move |s| (it.day.clone(), it.stream.clone(), s.clone()))
                })
                .collect();
            (c.cand_id.as_str(), set)
        })
        .collect();

    let has_shared_cross_facet = |cid: &str, facet: &str| -> bool {
        let Some(my) = seg_sets.get(cid) else {
            return false;
        };
        candidates.iter().any(|other| {
            other.cand_id != cid
                && other.primary_facet != facet
                && seg_sets
                    .get(other.cand_id.as_str())
                    .is_some_and(|os| my.intersection(os).next().is_some())
        })
    };

    let mut decisions: BTreeMap<String, (String, Option<String>)> = BTreeMap::new();
    if let Some(verdicts) = raw_output.get("verdicts").and_then(Value::as_array) {
        for v in verdicts {
            let Some(cid) = v.get("cand_id").and_then(Value::as_str).map(str::trim) else {
                continue;
            };
            let dec = v
                .get("decision")
                .and_then(Value::as_str)
                .unwrap_or("keep")
                .trim()
                .to_owned();
            let dup_of = v
                .get("duplicate_of")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned);
            decisions.insert(cid.to_owned(), (dec, dup_of));
        }
    }

    // Safety guard #1: never drop as cross-facet duplicate when no shared segments exist.
    for c in candidates {
        if let Some((dec, dup_of)) = decisions.get_mut(&c.cand_id)
            && dec == "drop_duplicate_cross_facet"
            && !has_shared_cross_facet(&c.cand_id, &c.primary_facet)
        {
            *dec = "keep".to_owned();
            *dup_of = None;
            audit.rescued_non_overlapping_dup += 1;
        }
    }

    // Safety guard #2: if a candidate was marked duplicate_of another candidate that
    // was also dropped as duplicate_cross_facet, rescue the strongest member of the pair.
    let mut rescue_ids: BTreeSet<String> = BTreeSet::new();
    for c in candidates {
        if let Some((dec, Some(target_id))) = decisions.get(&c.cand_id)
            && dec == "drop_duplicate_cross_facet"
            && decisions
                .get(target_id)
                .is_some_and(|(tdec, _)| tdec == "drop_duplicate_cross_facet")
        {
            let c_segs = seg_sets.get(c.cand_id.as_str()).map_or(0, BTreeSet::len);
            let t_segs = seg_sets.get(target_id.as_str()).map_or(0, BTreeSet::len);
            let winner = if t_segs > c_segs
                || (t_segs == c_segs && target_id.as_str() < c.cand_id.as_str())
            {
                target_id.clone()
            } else {
                c.cand_id.clone()
            };
            rescue_ids.insert(winner);
        }
    }
    for wid in rescue_ids {
        if let Some((dec, dup_of)) = decisions.get_mut(&wid) {
            *dec = "keep".to_owned();
            *dup_of = None;
            audit.rescued_mutual_dup_pair += 1;
        }
    }

    let mut survivors: Vec<&DetectedCandidate> = Vec::new();
    for c in candidates {
        let dec = decisions
            .get(&c.cand_id)
            .map(|(d, _)| d.as_str())
            .unwrap_or("keep");
        match dec {
            "drop_duplicate_cross_facet" => audit.dropped_duplicate_cross_facet += 1,
            "drop_passive_spectacle" => audit.dropped_passive_spectacle += 1,
            "drop_routine_or_trivial" => audit.dropped_routine_or_trivial += 1,
            _ => survivors.push(c),
        }
    }

    // Prefer richer / domain-specific candidates first for the deterministic A1 backstop.
    survivors.sort_by(|a, b| {
        let segs_a = seg_sets.get(a.cand_id.as_str()).map_or(0, BTreeSet::len);
        let segs_b = seg_sets.get(b.cand_id.as_str()).map_or(0, BTreeSet::len);
        let personal_a = usize::from(a.primary_facet == "personal");
        let personal_b = usize::from(b.primary_facet == "personal");
        segs_b
            .cmp(&segs_a)
            .then_with(|| personal_a.cmp(&personal_b))
            .then_with(|| a.started_at.cmp(&b.started_at))
            .then_with(|| a.cand_id.cmp(&b.cand_id))
    });

    let mut accepted: Vec<DetectedCandidate> = Vec::new();
    type SegKeySet = BTreeSet<(String, String, String)>;
    let mut accepted_segs: Vec<(String, SegKeySet)> = Vec::new();
    for c in survivors {
        let cur = seg_sets
            .get(c.cand_id.as_str())
            .cloned()
            .unwrap_or_default();
        let suppressed = accepted_segs.iter().any(|(pf, prev)| {
            if pf == &c.primary_facet || cur.is_empty() {
                return false;
            }
            let ov = cur.intersection(prev).count();
            let unique = cur.difference(prev).count();
            (ov as f64 / cur.len() as f64) >= 0.60 && unique < 2
        });
        if suppressed {
            audit.suppressed_a1_backstop += 1;
            continue;
        }
        accepted_segs.push((c.primary_facet.clone(), cur));
        accepted.push(c.clone());
    }

    accepted.sort_by(|a, b| {
        a.started_at
            .cmp(&b.started_at)
            .then_with(|| a.primary_facet.cmp(&b.primary_facet))
            .then_with(|| a.cand_id.cmp(&b.cand_id))
    });
    audit.kept = accepted.len();
    (accepted, audit)
}

/// Typed commit payload for one detected Experience persisted to `solstone-core-facets`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DetectedExperienceCommit {
    pub primary_facet: String,
    pub anchor_day: String,
    pub title_hint: String,
    pub category: Option<String>,
    pub started_at: String,
    pub ended_at: String,
    pub activities: Vec<ActivityRef>,
    pub extends_experience_id: Option<String>,
}

/// Convert accepted Stage 1B candidates into durable commit inputs for `solstone-core-facets`.
pub fn candidates_to_upsert_inputs(
    candidates: &[DetectedCandidate],
) -> Vec<DetectedExperienceCommit> {
    candidates
        .iter()
        .map(|c| DetectedExperienceCommit {
            primary_facet: c.primary_facet.clone(),
            anchor_day: c.anchor_day.clone(),
            title_hint: c.title_hint.clone(),
            category: (!c.category.is_empty()).then(|| c.category.clone()),
            started_at: c.started_at.clone(),
            ended_at: c.ended_at.clone(),
            activities: c.activities.clone(),
            extends_experience_id: c.extends_experience_id.clone(),
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Daily hook stage wiring (`experiences_detect_facet` + `experiences_detect`)
// ---------------------------------------------------------------------------

pub fn build_stage1a(
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
                "experiences_detect_facet",
                prepared,
                "missing day",
            ))
        })?
        .to_owned();
    let facet = prepared
        .config
        .get("facet")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            RuntimeOutcome::StageFailed(stage_error(
                "build",
                "experiences_detect_facet",
                prepared,
                "missing facet",
            ))
        })?
        .to_owned();
    let pool = load_day_multi_facet_pool(&context.journal, &day, None).map_err(|e| {
        RuntimeOutcome::StageFailed(stage_error(
            "build",
            "experiences_detect_facet",
            prepared,
            e,
        ))
    })?;
    let max_output = prepared
        .config
        .get("max_output_tokens")
        .and_then(Value::as_u64)
        .and_then(|v| u32::try_from(v).ok())
        .unwrap_or(2048);
    let overhead = estimate_prompt_overhead(&prepared.config);
    let Some((digest, _)) = render_stage1a_facet_digest(
        &pool,
        &facet,
        &pool.open_prev_experiences,
        max_output,
        overhead,
    ) else {
        return Err(RuntimeOutcome::Skipped {
            stage: "build".to_owned(),
            talent: prepared.name.clone(),
            reason: "no_candidates".to_owned(),
        });
    };
    let cand_path = stage1a_candidate_path(&context.journal, &day, &facet).map_err(|e| {
        RuntimeOutcome::StageFailed(stage_error(
            "build",
            "experiences_detect_facet",
            prepared,
            e,
        ))
    })?;
    Ok(PrePostState::ExperiencesDetectFacet(Box::new(
        Stage1APreState {
            day,
            facet,
            facet_digest: digest,
            pool,
            candidate_path: cand_path.to_string_lossy().into_owned(),
        },
    )))
}

pub fn apply_prompt_override_stage1a(
    prepared: &mut PreparedTalent,
    state: &PrePostState,
) -> Result<(), StageError> {
    let PrePostState::ExperiencesDetectFacet(st) = state else {
        return Err(stage_error(
            "prompt_override",
            "experiences_detect_facet",
            prepared,
            "missing Stage 1A state",
        ));
    };
    apply_template_vars(
        &mut prepared.config,
        &Map::from_iter([(
            "facet_digest".to_owned(),
            Value::String(st.facet_digest.clone()),
        )]),
    );
    Ok(())
}

pub fn parse_stage1a(
    output: &str,
    prepared: &PreparedTalent,
    _state: &PrePostState,
) -> Result<ParsedOutput, StageError> {
    let val: Value = serde_json::from_str(output).map_err(|e| {
        stage_error(
            "parse",
            "experiences_detect_facet",
            prepared,
            format!("invalid Stage 1A JSON: {e}"),
        )
    })?;
    if !val.get("experiences").is_some_and(Value::is_array) {
        return Err(stage_error(
            "parse",
            "experiences_detect_facet",
            prepared,
            "Stage 1A JSON missing experiences array",
        ));
    }
    Ok(ParsedOutput::Json(val))
}

pub fn commit_stage1a(
    parsed: ParsedOutput,
    prepared: &PreparedTalent,
    state: &PrePostState,
) -> Result<CommitPlan, StageError> {
    let ParsedOutput::Json(val) = parsed else {
        return Err(stage_error(
            "commit",
            "experiences_detect_facet",
            prepared,
            "expected parsed JSON for Stage 1A",
        ));
    };
    let PrePostState::ExperiencesDetectFacet(st) = state else {
        return Err(stage_error(
            "commit",
            "experiences_detect_facet",
            prepared,
            "missing Stage 1A state",
        ));
    };
    let (candidates, _audit) = postprocess_stage1a_facet(&val, &st.pool, &st.facet);
    let envelope = Stage1ACandidateFile {
        version: 1,
        day: st.day.clone(),
        facet: st.facet.clone(),
        candidates,
    };
    let serialized = serde_json::to_string_pretty(&envelope).map_err(|e| {
        stage_error(
            "commit",
            "experiences_detect_facet",
            prepared,
            format!("serialize Stage 1A candidates: {e}"),
        )
    })?;
    Ok(CommitPlan::Write(WriteIntent::ExperienceCandidates {
        day: st.day.clone(),
        facet: st.facet.clone(),
        output_path: st.candidate_path.clone(),
        output: format!("{serialized}\n"),
    }))
}

/// Read all persisted Stage 1A candidate files for `day` across active facets.
pub fn load_stage1a_candidates(
    journal: &Path,
    day: &str,
    active_facets: &BTreeMap<String, super::prefilter::ActiveFacetInfo>,
) -> Result<Vec<DetectedCandidate>, String> {
    let dir = stage1a_candidates_dir(journal, day)?;
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut all = Vec::new();
    for facet in active_facets.keys() {
        let path = stage1a_candidate_path(journal, day, facet)?;
        let Ok(raw) = fs::read_to_string(&path) else {
            continue;
        };
        let Ok(file) = serde_json::from_str::<Stage1ACandidateFile>(&raw) else {
            continue;
        };
        if file.day == day && file.facet == *facet {
            all.extend(file.candidates);
        }
    }
    all.sort_by(|a, b| {
        a.started_at
            .cmp(&b.started_at)
            .then_with(|| a.primary_facet.cmp(&b.primary_facet))
            .then_with(|| a.title_hint.cmp(&b.title_hint))
    });
    Ok(all)
}

pub fn build_stage1b(
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
                "experiences_detect",
                prepared,
                "missing day",
            ))
        })?
        .to_owned();
    let pool = load_day_multi_facet_pool(&context.journal, &day, None).map_err(|e| {
        RuntimeOutcome::StageFailed(stage_error("build", "experiences_detect", prepared, e))
    })?;
    let mut candidates =
        load_stage1a_candidates(&context.journal, &day, &pool.facets).map_err(|e| {
            RuntimeOutcome::StageFailed(stage_error("build", "experiences_detect", prepared, e))
        })?;
    if candidates.is_empty() {
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
        .unwrap_or(1536);
    let overhead = estimate_prompt_overhead(&prepared.config);
    let packet = render_stage1b_arbitration_packet(&pool, &mut candidates, max_output, overhead);
    Ok(PrePostState::ExperiencesDetect(Box::new(Stage1BPreState {
        day,
        candidates_packet: packet,
        candidates,
    })))
}

pub fn apply_prompt_override_stage1b(
    prepared: &mut PreparedTalent,
    state: &PrePostState,
) -> Result<(), StageError> {
    let PrePostState::ExperiencesDetect(st) = state else {
        return Err(stage_error(
            "prompt_override",
            "experiences_detect",
            prepared,
            "missing Stage 1B state",
        ));
    };
    apply_template_vars(
        &mut prepared.config,
        &Map::from_iter([(
            "candidates_packet".to_owned(),
            Value::String(st.candidates_packet.clone()),
        )]),
    );
    Ok(())
}

pub fn parse_stage1b(
    output: &str,
    prepared: &PreparedTalent,
    _state: &PrePostState,
) -> Result<ParsedOutput, StageError> {
    let val: Value = serde_json::from_str(output).map_err(|e| {
        stage_error(
            "parse",
            "experiences_detect",
            prepared,
            format!("invalid Stage 1B JSON: {e}"),
        )
    })?;
    if !val.get("verdicts").is_some_and(Value::is_array) {
        return Err(stage_error(
            "parse",
            "experiences_detect",
            prepared,
            "Stage 1B JSON missing verdicts array",
        ));
    }
    Ok(ParsedOutput::Json(val))
}

pub fn commit_stage1b(
    parsed: ParsedOutput,
    prepared: &PreparedTalent,
    state: &PrePostState,
) -> Result<CommitPlan, StageError> {
    let ParsedOutput::Json(val) = parsed else {
        return Err(stage_error(
            "commit",
            "experiences_detect",
            prepared,
            "expected parsed JSON for Stage 1B",
        ));
    };
    let PrePostState::ExperiencesDetect(st) = state else {
        return Err(stage_error(
            "commit",
            "experiences_detect",
            prepared,
            "missing Stage 1B state",
        ));
    };
    let (accepted, _audit) = postprocess_stage1b_verdicts(&val, &st.candidates);
    let experiences = candidates_to_upsert_inputs(&accepted);
    if experiences.is_empty() {
        return Ok(CommitPlan::NoOutput);
    }
    Ok(CommitPlan::Write(WriteIntent::ExperiencesCommit {
        day: st.day.clone(),
        experiences,
    }))
}

/// Commit accepted detected Experiences to `facets/<facet>/experiences/<anchor_day>.jsonl`
/// and enqueue `experience_compose` via `OwnerAsk`.
pub fn apply_detected_experiences(
    journal: &Path,
    inputs: &[DetectedExperienceCommit],
    now_iso: &str,
) -> Result<Vec<(String, String, String, String)>, String> {
    let mut committed = Vec::new();
    for input in inputs {
        let Some(record) = solstone_core_facets::upsert_detected_experience(
            journal,
            &input.primary_facet,
            &input.anchor_day,
            input.extends_experience_id.as_deref(),
            &input.title_hint,
            input.category.as_deref(),
            input.activities.clone(),
            &input.started_at,
            &input.ended_at,
            now_iso,
        )
        .map_err(|e| e.to_string())?
        else {
            continue;
        };
        let path = format!(
            "facets/{}/experiences/{}.jsonl",
            record.primary_facet, record.anchor_day
        );
        let _ = solstone_core_system::owner_ask::ask_experience_compose(
            journal,
            &record.primary_facet,
            &record.anchor_day,
            &record.id,
            Some("detect"),
        );
        committed.push((record.primary_facet, record.anchor_day, record.id, path));
    }
    Ok(committed)
}

pub(crate) fn estimate_prompt_overhead(config: &Map<String, Value>) -> u32 {
    let prompt_tokens = config
        .get("prompt")
        .and_then(Value::as_str)
        .map(solstone_core_tokens::count)
        .unwrap_or(0);
    let schema_tokens = config
        .get("json_schema")
        .map(|v| solstone_core_tokens::count(&v.to_string()))
        .unwrap_or(0);
    prompt_tokens
        .saturating_add(schema_tokens)
        .saturating_add(512)
}

/// Helper to build a JSON value from `Stage1ACandidateFile` cleanly for tests and runners.
pub fn serialize_stage1a_file(day: &str, facet: &str, candidates: Vec<DetectedCandidate>) -> Value {
    json!(Stage1ACandidateFile {
        version: 1,
        day: day.to_owned(),
        facet: facet.to_owned(),
        candidates,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[allow(clippy::too_many_arguments)]
    fn sample_item(
        handle: &str,
        facet: &str,
        stream: &str,
        activity: &str,
        start_s: i64,
        end_s: i64,
        people: Vec<&str>,
        desc: &str,
    ) -> PoolItem {
        PoolItem {
            handle: handle.to_owned(),
            day: "20260304".to_owned(),
            is_prev_evening: false,
            facet: facet.to_owned(),
            stream: stream.to_owned(),
            id: format!("id_{handle}"),
            activity: activity.to_owned(),
            start_s,
            end_s,
            start_hm: super::super::prefilter::format_hm(start_s),
            end_hm: super::super::prefilter::format_hm(end_s),
            duration_min: (end_s - start_s) / 60,
            segments: vec![format!(
                "{:02}{:02}00_300",
                start_s / 3600,
                (start_s % 3600) / 60
            )],
            has_story: true,
            has_media: false,
            playback: None,
            cross_facet_twins: Vec::new(),
            people: people.iter().map(|s| (*s).to_owned()).collect(),
            corroborated_entity_ids: people.iter().map(|s| (*s).to_lowercase()).collect(),
            location_hints: Vec::new(),
            topics: Vec::new(),
            description: desc.to_owned(),
            summary: desc.to_owned(),
        }
    }

    #[test]
    fn solo_pregate_drops_routine_solo_terminal_and_keeps_milestone() {
        let routine = vec![sample_item(
            "WO01",
            "work",
            "tmux",
            "coding",
            9 * 3600,
            10 * 3600 + 1800,
            vec![],
            "Routine tmux folder review and agent status check.",
        )];
        assert!(!passes_d1_solo_pregate(
            &routine,
            "solo_milestone",
            "Morning terminal session",
            "folder review"
        ));

        let milestone = vec![sample_item(
            "WO02",
            "work",
            "obsidian",
            "writing",
            11 * 3600,
            12 * 3600 + 1800,
            vec![],
            "Drafted and submitted the conference talk proposal.",
        )];
        assert!(passes_d1_solo_pregate(
            &milestone,
            "solo_milestone",
            "Submitted conference talk proposal",
            "talk proposal submission"
        ));
    }

    #[test]
    fn gap_shift_splitter_splits_at_90m_with_participant_shift() {
        let morning = sample_item(
            "PE01",
            "personal",
            "watch",
            "social",
            9 * 3600,
            10 * 3600,
            vec!["Maya"],
            "Morning coffee with Maya.",
        );
        let afternoon = sample_item(
            "PE02",
            "personal",
            "watch",
            "social",
            11 * 3600 + 45 * 60, // 105m gap
            12 * 3600 + 45 * 60,
            vec!["Devon"],
            "Lunch with Devon.",
        );
        let groups = split_experience_by_d2_gap_and_shift(&[morning, afternoon]);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0][0].handle, "PE01");
        assert_eq!(groups[1][0].handle, "PE02");
    }

    #[test]
    fn gap_shift_splitter_keeps_continuous_same_people_event_under_135m() {
        let part1 = sample_item(
            "PE01",
            "personal",
            "watch",
            "social",
            14 * 3600,
            15 * 3600,
            vec!["Maya"],
            "Afternoon walk with Maya.",
        );
        let part2 = sample_item(
            "PE02",
            "personal",
            "watch",
            "social",
            16 * 3600 + 35 * 60, // 95m gap, same person, same mode
            17 * 3600 + 30 * 60,
            vec!["Maya"],
            "Continued dinner with Maya.",
        );
        let groups = split_experience_by_d2_gap_and_shift(&[part1, part2]);
        assert_eq!(groups.len(), 1);
    }
}
