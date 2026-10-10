// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Weekly reflection reads each activity day's briefing directly from
//! `chronicle/<D>/talents/morning_briefing.json`. It does not use the
//! presentation-day helper.
//!
//! `assess_day` classifies a day in this order. A missing day directory
//! (`NotFound`) or a directory whose `iter_segments` is empty is
//! `NothingShared`: absent, not unreadable, and a briefing file does not
//! override the empty-segment case. Any other metadata error, a
//! non-directory, an `iter_segments` error, a briefing read error other
//! than `NotFound`, bad JSON, a non-object, or a missing or non-array
//! `yesterday` is `Unreadable`. Segments exist and the briefing is
//! `NotFound`: `NotReady`. A `yesterday` array stays `NotOnPage`, with
//! candidates when extraction returned any. `Memory` is applied only in
//! `assemble_reflection`, for a day whose candidate is shown.
//!
//! Empty slots call `finish_unavailable("zero_slots")` before prompt
//! override and generate. That writes `selection.status` `none` and emits
//! no `unavailable_selection` use-log event. The other reasons that reach
//! `unavailable_commit` are `schema_exhausted`, `refused:<wire>`,
//! `transport`, and `clipped`; each writes a fallback page and emits the
//! event. Any other stage failure returns `StageFailed` and writes neither
//! file.
//!
//! `write_page` writes `reflections/weekly/<start>.md`, then `<start>.json`.
//! An index failure records `index_warning` and still returns `Ok`.
//!
//! `apply_prompt_override` inserts the packet JSON as `transcript` and
//! removes `prompt`. It does not call `apply_template_vars`, so the packet
//! stays unsubstituted.
//! `said` is built by code, never by the model, from naturally quoted passages
//! in the story body that match the owner's recognized-voice lines.

#[cfg(test)]
use std::cell::Cell;
use std::collections::BTreeSet;
use std::fs;
use std::path::Path;
use std::sync::OnceLock;

use chrono::{DateTime, Datelike, Duration, NaiveDate, Utc};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use solstone_core_facets::load_activity_records;
use solstone_core_home::{HomeContext, readers::enabled_facet_names};
use solstone_core_indexer_store::scan::{RescanFileStatus, rescan_file};
use solstone_core_journal_io::iter_segments;
use solstone_core_segment::owner_deleted;

use crate::contract::{CommitPlan, ParsedOutput, PrePostState};
use crate::writers::{WriteIntent, index_warning};
use crate::{ExecutionContext, PreparedTalent, RuntimeOutcome, StageError, stage_error};

#[cfg(test)]
thread_local! {
    static PINNED_STAGE_NOW: Cell<Option<DateTime<Utc>>> = const { Cell::new(None) };
}

#[cfg(test)]
pub(crate) fn set_pinned_stage_now(now: Option<DateTime<Utc>>) {
    PINNED_STAGE_NOW.with(|cell| cell.set(now));
}

#[cfg(test)]
fn pinned_or_utc_now() -> DateTime<Utc> {
    PINNED_STAGE_NOW
        .with(|cell| cell.get())
        .unwrap_or_else(Utc::now)
}

#[cfg(not(test))]
fn pinned_or_utc_now() -> DateTime<Utc> {
    Utc::now()
}

static PLACEHOLDER_RE: OnceLock<Regex> = OnceLock::new();

fn placeholder_regex() -> &'static Regex {
    PLACEHOLDER_RE.get_or_init(|| {
        Regex::new(r"(?i)\b(?:speaker[ _-]*[0-9]+|unknown speaker|unidentified speaker)\b")
            .expect("valid regex")
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    pub day: String,
    pub position: usize,
    pub text: String,
    pub placeholder: bool,
    pub word_count: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Slot {
    pub slot_id: String,
    pub coordinate: String,
    pub candidates: Vec<Candidate>,
    pub memory_ids: Vec<String>,
    pub placeholder_only: bool,
    pub oversize: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DayClassification {
    NothingShared,
    NotReady,
    NotOnPage,
    Memory,
    Unreadable,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DayReport {
    pub day: String,
    pub classification: DayClassification,
}

/// Something the owner said, by their recognized voice, on one of the week's days.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Said {
    pub day: String,
    pub facet: String,
    pub record_id: String,
    pub quote: String,
}

/// The most `said` entries one day contributes.
const SAID_PER_DAY: usize = 3;

#[derive(Clone, Debug, PartialEq)]
pub struct WeeklyReflectionState {
    pub start_day: String,
    pub end_day: String,
    pub today: NaiveDate,
    pub generated_at: String,
    pub day_reports: Vec<DayReport>,
    pub slots: Vec<Slot>,
    pub all_candidates: Vec<(String, Candidate)>, // (memory_id, candidate)
    pub said: Vec<Said>,
}

pub fn build(
    prepared: &mut PreparedTalent,
    context: &ExecutionContext,
) -> Result<PrePostState, RuntimeOutcome> {
    let utc = pinned_or_utc_now();
    let zoned = utc.with_timezone(&solstone_core_journal_config::owner_zone(&context.journal));
    let today = zoned.date_naive();
    let generated_at = zoned.to_rfc3339_opts(chrono::SecondsFormat::Secs, true);

    let day_str = configured_day(prepared);
    let start_date = match NaiveDate::parse_from_str(&day_str, "%Y%m%d") {
        Ok(date) => date,
        Err(_) => {
            return Err(RuntimeOutcome::StageFailed(stage_error(
                "build",
                "weekly_reflection",
                prepared,
                format!("invalid or unparseable day: '{day_str}'"),
            )));
        }
    };

    if start_date.weekday() != chrono::Weekday::Sun {
        return Err(RuntimeOutcome::StageFailed(stage_error(
            "build",
            "weekly_reflection",
            prepared,
            format!("request day {day_str} is not a Sunday"),
        )));
    }

    let end_date = start_date + Duration::days(6);
    if end_date >= today {
        return Err(RuntimeOutcome::StageFailed(stage_error(
            "build",
            "weekly_reflection",
            prepared,
            format!("week ending {end_date} has not completed before today {today}"),
        )));
    }

    let start_day = start_date.format("%Y%m%d").to_string();
    let end_day = end_date.format("%Y%m%d").to_string();

    let mut day_reports = Vec::new();
    let mut slots = Vec::new();
    let mut all_candidates = Vec::new();
    let mut said = Vec::new();
    let mut slot_counter = 0usize;
    let mut memory_counter = 0usize;
    let facets = enabled_facet_names(&HomeContext::new(&context.journal, utc));

    let mut curr = start_date;
    while curr <= end_date {
        let curr_str = curr.format("%Y%m%d").to_string();
        let (classif, candidates) = assess_day(&context.journal, &curr_str);
        said.extend(said_on_day(&context.journal, &facets, &curr_str));
        day_reports.push(DayReport {
            day: curr_str.clone(),
            classification: classif,
        });

        if !candidates.is_empty() {
            let slot_id = format!("S{:02}", slot_counter);
            slot_counter += 1;

            let placeholder_only = candidates.iter().all(|c| c.placeholder);
            let oversize = candidates.iter().all(|c| c.word_count > 90);

            let mut memory_ids = Vec::new();
            for candidate in &candidates {
                let mem_id = format!("M{:03}", memory_counter);
                memory_counter += 1;
                memory_ids.push(mem_id.clone());
                all_candidates.push((mem_id, candidate.clone()));
            }

            slots.push(Slot {
                slot_id,
                coordinate: curr_str,
                candidates,
                memory_ids,
                placeholder_only,
                oversize,
            });
        }

        curr += Duration::days(1);
    }

    let state = WeeklyReflectionState {
        start_day,
        end_day,
        today,
        generated_at,
        day_reports,
        slots,
        all_candidates,
        said,
    };

    Ok(PrePostState::WeeklyReflection(state))
}

pub fn unavailable_before_generate(state: &PrePostState) -> Option<&'static str> {
    match state {
        PrePostState::WeeklyReflection(state) if state.slots.is_empty() => Some("zero_slots"),
        _ => None,
    }
}

pub fn apply_prompt_override(
    prepared: &mut PreparedTalent,
    state: &PrePostState,
) -> Result<(), StageError> {
    let PrePostState::WeeklyReflection(state) = state else {
        return Err(stage_error(
            "prompt_override",
            "weekly_reflection",
            prepared,
            "mismatched pre-state for weekly_reflection",
        ));
    };

    if state.slots.is_empty() {
        return Ok(());
    }

    let mut slots_json = Vec::new();
    let mut memories_json = Vec::new();

    for slot in &state.slots {
        slots_json.push(json!({
            "slot": slot.slot_id,
            "coordinate": slot.coordinate,
            "ids": slot.memory_ids,
            "placeholder_only": slot.placeholder_only,
            "oversize": slot.oversize,
        }));
    }

    for (mem_id, cand) in &state.all_candidates {
        memories_json.push(json!({
            "id": mem_id,
            "day": cand.day,
            "text": cand.text,
            "placeholder": cand.placeholder,
        }));
    }

    let packet = json!({
        "week": [state.start_day, state.end_day],
        "slots": slots_json,
        "memories": memories_json,
    });

    let packet_str = serde_json::to_string(&packet).map_err(|e| {
        stage_error(
            "prompt_override",
            "weekly_reflection",
            prepared,
            e.to_string(),
        )
    })?;

    prepared
        .config
        .insert("transcript".to_owned(), Value::String(packet_str));
    prepared.config.remove("prompt");

    let mut slot_props = Map::new();
    let mut slot_required = Vec::new();

    for slot in &state.slots {
        slot_required.push(Value::String(slot.slot_id.clone()));
        let id_enums = slot
            .memory_ids
            .iter()
            .map(|id| Value::String(id.clone()))
            .collect::<Vec<_>>();
        slot_props.insert(
            slot.slot_id.clone(),
            json!({
                "type": "string",
                "enum": id_enums,
            }),
        );
    }

    let schema = json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["selections"],
        "properties": {
            "selections": {
                "type": "object",
                "additionalProperties": false,
                "required": slot_required,
                "properties": slot_props,
            }
        }
    });

    prepared.config.insert("json_schema".to_owned(), schema);

    Ok(())
}

pub fn parse(
    response: &str,
    prepared: &PreparedTalent,
    state: &PrePostState,
) -> Result<ParsedOutput, StageError> {
    let PrePostState::WeeklyReflection(state) = state else {
        return Err(stage_error(
            "parse",
            "weekly_reflection",
            prepared,
            "mismatched pre-state for weekly_reflection",
        ));
    };

    let value: Value = serde_json::from_str(response).map_err(|e| {
        stage_error(
            "parse",
            "weekly_reflection",
            prepared,
            format!("response is not valid JSON: {e}"),
        )
    })?;

    let selections_obj = value
        .get("selections")
        .and_then(Value::as_object)
        .ok_or_else(|| {
            stage_error(
                "parse",
                "weekly_reflection",
                prepared,
                "response missing 'selections' object",
            )
        })?;

    if selections_obj.len() != state.slots.len() {
        return Err(stage_error(
            "parse",
            "weekly_reflection",
            prepared,
            format!(
                "selections count {} does not match slots count {}",
                selections_obj.len(),
                state.slots.len()
            ),
        ));
    }

    let mut chosen_ids = Map::new();
    for slot in &state.slots {
        let mem_id = selections_obj
            .get(&slot.slot_id)
            .and_then(Value::as_str)
            .ok_or_else(|| {
                stage_error(
                    "parse",
                    "weekly_reflection",
                    prepared,
                    format!("missing selection for slot '{}'", slot.slot_id),
                )
            })?;
        if !slot.memory_ids.iter().any(|id| id == mem_id) {
            return Err(stage_error(
                "parse",
                "weekly_reflection",
                prepared,
                format!(
                    "selection '{mem_id}' is not in valid IDs for slot '{}'",
                    slot.slot_id
                ),
            ));
        }
        chosen_ids.insert(slot.slot_id.clone(), Value::String(mem_id.to_owned()));
    }

    Ok(ParsedOutput::Json(Value::Object(chosen_ids)))
}

pub fn commit(
    parsed: ParsedOutput,
    prepared: &PreparedTalent,
    state: &PrePostState,
) -> Result<CommitPlan, StageError> {
    let PrePostState::WeeklyReflection(state) = state else {
        return Err(stage_error(
            "commit",
            "weekly_reflection",
            prepared,
            "mismatched pre-state for weekly_reflection",
        ));
    };

    let ParsedOutput::Json(Value::Object(chosen_map)) = parsed else {
        return Err(stage_error(
            "commit",
            "weekly_reflection",
            prepared,
            "expected Json parsed output",
        ));
    };

    let model_str = get_model_identifier(prepared)?;

    let mut selected_candidates = Vec::new();
    for slot in &state.slots {
        let mem_id = chosen_map
            .get(&slot.slot_id)
            .and_then(Value::as_str)
            .ok_or_else(|| {
                stage_error(
                    "commit",
                    "weekly_reflection",
                    prepared,
                    format!("missing chosen slot {}", slot.slot_id),
                )
            })?;
        let (_id, cand) = state
            .all_candidates
            .iter()
            .find(|(id, _)| id == mem_id)
            .ok_or_else(|| {
                stage_error(
                    "commit",
                    "weekly_reflection",
                    prepared,
                    format!("candidate '{mem_id}' not found"),
                )
            })?;
        selected_candidates.push(cand.clone());
    }

    let (markdown, document) =
        assemble_reflection(state, "model", Some(&model_str), &selected_candidates)?;

    Ok(CommitPlan::Write(WriteIntent::WeeklyReflection {
        start: state.start_day.clone(),
        markdown,
        document,
    }))
}

pub fn unavailable_commit(
    reason: &str,
    prepared: &PreparedTalent,
    state: &PrePostState,
) -> Result<CommitPlan, StageError> {
    let PrePostState::WeeklyReflection(state) = state else {
        return Err(stage_error(
            "unavailable_commit",
            "weekly_reflection",
            prepared,
            "mismatched pre-state for weekly_reflection",
        ));
    };

    if reason == "zero_slots" {
        let (markdown, document) = assemble_reflection(state, "none", None, &[])?;
        return Ok(CommitPlan::Write(WriteIntent::WeeklyReflection {
            start: state.start_day.clone(),
            markdown,
            document,
        }));
    }

    let model_str = get_model_identifier(prepared)?;
    let mut fallback_candidates = Vec::new();
    for slot in &state.slots {
        if let Some(first) = slot.candidates.first() {
            fallback_candidates.push(first.clone());
        }
    }

    let (markdown, document) =
        assemble_reflection(state, "fallback", Some(&model_str), &fallback_candidates)?;

    Ok(CommitPlan::Write(WriteIntent::WeeklyReflection {
        start: state.start_day.clone(),
        markdown,
        document,
    }))
}

fn get_model_identifier(prepared: &PreparedTalent) -> Result<String, StageError> {
    let provider = prepared
        .config
        .get("provider")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            stage_error(
                "commit",
                "weekly_reflection",
                prepared,
                "missing provider in config",
            )
        })?;
    let model = prepared
        .config
        .get("model")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            stage_error(
                "commit",
                "weekly_reflection",
                prepared,
                "missing model in config",
            )
        })?;
    Ok(provider_model_identifier(provider, model))
}

/// Join provider and model as `<provider>/<model>`, unless the configured
/// model id already carries that provider prefix (the bundled local model is
/// configured as `local/qwen3.5-4b`).
fn provider_model_identifier(provider: &str, model: &str) -> String {
    if model
        .strip_prefix(provider)
        .is_some_and(|rest| rest.starts_with('/'))
    {
        model.to_owned()
    } else {
        format!("{provider}/{model}")
    }
}

#[derive(Serialize)]
struct JsonWeek<'a> {
    start: &'a str,
    end: &'a str,
}

#[derive(Serialize)]
struct JsonSelection<'a> {
    status: &'a str,
    model: Option<&'a str>,
}

#[derive(Serialize)]
struct JsonIntroItem<'a> {
    sentence: &'a str,
    ids: Vec<&'a str>,
}

#[derive(Serialize)]
struct JsonMemorySource<'a> {
    kind: &'static str,
    uri: String,
    briefing_day: &'a str,
    refs: Vec<String>,
}

#[derive(Serialize)]
struct JsonMemoryItem<'a> {
    id: String,
    key: String,
    day: &'a str,
    text: &'a str,
    source: JsonMemorySource<'a>,
}

#[derive(Serialize)]
struct JsonDayItem<'a> {
    day: &'a str,
    state: DayClassification,
    #[serde(skip_serializing_if = "Option::is_none")]
    memory_id: Option<String>,
}

#[derive(Serialize)]
struct JsonSaidSource {
    kind: &'static str,
    uri: String,
}

#[derive(Serialize)]
struct JsonSaidItem<'a> {
    id: String,
    key: String,
    day: &'a str,
    quote: &'a str,
    source: JsonSaidSource,
}

#[derive(Serialize)]
struct JsonDocument<'a> {
    version: u64,
    week: JsonWeek<'a>,
    generated_at: &'a str,
    selection: JsonSelection<'a>,
    intro: Vec<JsonIntroItem<'a>>,
    memories: Vec<JsonMemoryItem<'a>>,
    days: Vec<JsonDayItem<'a>>,
    said: Vec<JsonSaidItem<'a>>,
}

pub fn compute_memory_key(day: &str, position: usize, text: &str) -> String {
    let input = format!("{day}\n{position}\n{text}");
    let digest = Sha256::digest(input.as_bytes());
    let hex = format!("{digest:x}");
    hex[..16].to_owned()
}

/// A key for a `said` entry that holds across reruns while its source row is
/// unchanged. The `said` prefix keeps it apart from every memory key.
pub fn compute_said_key(said: &Said) -> String {
    let input = format!(
        "said\n{}\n{}\n{}\n{}",
        said.day, said.facet, said.record_id, said.quote
    );
    let digest = Sha256::digest(input.as_bytes());
    let hex = format!("{digest:x}");
    hex[..16].to_owned()
}

/// The owner's voice-backed Story quotes on one day, in record order; a
/// repeated quote is kept once and the day is capped.
fn said_on_day(journal: &Path, facets: &[String], day: &str) -> Vec<Said> {
    let mut said = Vec::new();
    let mut seen = BTreeSet::new();
    for facet in facets {
        let Ok(records) = load_activity_records(journal, facet, day, false) else {
            continue;
        };
        for record in records {
            let Some(record_id) = record
                .get("id")
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty())
            else {
                continue;
            };
            // The words rest on the conversation's segments; once the owner has
            // deleted every one of them, the weekly does not bring the words back.
            if !keeps_a_segment(journal, day, &record) {
                continue;
            }
            let Some(body) = record
                .get("story")
                .and_then(Value::as_object)
                .and_then(|s| s.get("body"))
                .and_then(Value::as_str)
            else {
                continue;
            };
            let Some(owner_lines) = crate::story::owner_heard(journal, day, &record) else {
                continue;
            };

            for passage in extract_passages(body) {
                if passage.split_whitespace().count() < 3 {
                    continue;
                }
                if let Some(owner_line) = owner_lines.iter().find(|line| line.contains(passage)) {
                    let words = owner_line
                        .to_lowercase()
                        .split(|c: char| !c.is_alphanumeric())
                        .filter(|w| !w.is_empty())
                        .collect::<Vec<_>>()
                        .join(" ");
                    if seen.insert(words) {
                        said.push(Said {
                            day: day.to_owned(),
                            facet: facet.clone(),
                            record_id: record_id.to_owned(),
                            quote: owner_line.clone(),
                        });
                    }
                }
            }
        }
    }
    said.truncate(SAID_PER_DAY);
    said
}

fn extract_passages(body: &str) -> Vec<&str> {
    let mut passages = Vec::new();
    let mut chars = body.char_indices().peekable();
    while let Some((start_idx, ch)) = chars.next() {
        let closing_char = match ch {
            '"' => Some('"'),
            '\u{201c}' => Some('\u{201d}'),
            '\u{2018}' => Some('\u{2019}'),
            _ => None,
        };
        if let Some(target) = closing_char {
            let inner_start = start_idx + ch.len_utf8();
            let mut found_close = None;
            for (idx, close_ch) in chars.by_ref() {
                if close_ch == target {
                    found_close = Some(idx);
                    break;
                }
            }
            if let Some(end_idx) = found_close {
                let trimmed = body[inner_start..end_idx].trim();
                if !trimmed.is_empty() {
                    passages.push(trimmed);
                }
            }
        }
    }
    passages
}

/// Whether any segment the activity names is still in the journal, looked up
/// the way the Story writer found its audio. A deleted segment's directory
/// stays behind as its tombstone, so it counts only while live; one whose
/// state cannot be read does not keep the words.
fn keeps_a_segment(journal: &Path, day: &str, record: &Map<String, Value>) -> bool {
    let day_dir = journal.join("chronicle").join(day);
    let stream = record.get("stream").and_then(Value::as_str);
    let streams = fs::read_dir(&day_dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| stream.is_none_or(|stream| entry.file_name() == stream))
        .map(|entry| entry.path())
        .collect::<Vec<_>>();
    record
        .get("segments")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .any(|segment| {
            streams
                .iter()
                .any(|dir| is_live_segment(&dir.join(segment)))
                || (stream.is_none() && is_live_segment(&day_dir.join(segment)))
        })
}

fn is_live_segment(dir: &Path) -> bool {
    dir.is_dir() && matches!(owner_deleted(dir), Ok(false))
}

pub fn extract_refs(text: &str) -> Vec<String> {
    let mut refs = Vec::new();
    let mut seen = BTreeSet::new();

    let bytes = text.as_bytes();
    let mut pos = 0;
    while let Some(sol_idx) = text[pos..].find("sol://") {
        let abs_sol = pos + sol_idx;
        let is_md_link = abs_sol >= 2 && &bytes[abs_sol - 2..abs_sol] == b"](";

        let (uri, next_pos) = if is_md_link {
            if let Some(close_idx) = text[abs_sol..].find(')') {
                let end = abs_sol + close_idx;
                (&text[abs_sol..end], end + 1)
            } else {
                (&text[abs_sol..], text.len())
            }
        } else {
            let mut end = abs_sol;
            while end < text.len() {
                let ch = text[end..].chars().next().unwrap();
                if ch.is_whitespace() || matches!(ch, ')' | ']' | '>' | '"' | '\'') {
                    break;
                }
                end += ch.len_utf8();
            }
            let raw = &text[abs_sol..end];
            let trimmed = raw.trim_end_matches(['.', ',', ';', ':', '!', '?']);
            (trimmed, end)
        };

        if !uri.is_empty() && seen.insert(uri.to_owned()) {
            refs.push(uri.to_owned());
        }

        pos = next_pos;
    }

    refs
}

pub fn normalize_markdown_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch.is_whitespace() {
            let mut run = String::new();
            run.push(ch);
            while let Some(&next) = chars.peek() {
                if next.is_whitespace() {
                    run.push(chars.next().unwrap());
                } else {
                    break;
                }
            }
            if run.contains('\n') || run.contains('\r') {
                out.push(' ');
            } else {
                out.push_str(&run);
            }
        } else {
            out.push(ch);
        }
    }

    let trimmed = out.trim_start();
    if trimmed.is_empty() {
        return String::new();
    }

    if trimmed.starts_with('#')
        || trimmed.starts_with('-')
        || trimmed.starts_with('*')
        || trimmed.starts_with('+')
        || trimmed.starts_with('>')
        || trimmed.starts_with("```")
        || trimmed.starts_with("~~~")
    {
        return format!("\\{trimmed}");
    }

    if let Some(dot_idx) = trimmed.find('.') {
        let prefix = &trimmed[..dot_idx];
        if !prefix.is_empty() && prefix.chars().all(|c| c.is_ascii_digit()) {
            let rest = &trimmed[dot_idx + 1..];
            return format!("{prefix}\\.{rest}");
        }
    }

    trimmed.to_owned()
}

fn english_weekday(weekday: chrono::Weekday) -> &'static str {
    match weekday {
        chrono::Weekday::Mon => "monday",
        chrono::Weekday::Tue => "tuesday",
        chrono::Weekday::Wed => "wednesday",
        chrono::Weekday::Thu => "thursday",
        chrono::Weekday::Fri => "friday",
        chrono::Weekday::Sat => "saturday",
        chrono::Weekday::Sun => "sunday",
    }
}

fn english_month(month: u32) -> &'static str {
    match month {
        1 => "january",
        2 => "february",
        3 => "march",
        4 => "april",
        5 => "may",
        6 => "june",
        7 => "july",
        8 => "august",
        9 => "september",
        10 => "october",
        11 => "november",
        12 => "december",
        _ => "unknown",
    }
}

pub fn assemble_reflection(
    state: &WeeklyReflectionState,
    selection_status: &str,
    selection_model: Option<&str>,
    selected_candidates: &[Candidate],
) -> Result<(String, String), StageError> {
    let mut sorted_candidates = selected_candidates.to_vec();
    sorted_candidates.sort_by(|a, b| a.day.cmp(&b.day));

    let n = sorted_candidates.len();
    let sentence_storage;
    let intro_sentence: Option<&str> = if n == 0 {
        None
    } else if n == 1 {
        sentence_storage = "your week, a memory from one day.".to_owned();
        Some(&sentence_storage)
    } else {
        sentence_storage = format!("your week, a memory from each of {n} days.");
        Some(&sentence_storage)
    };

    let mut memory_items = Vec::new();
    let mut shown_contract_ids = Vec::new();

    for cand in &sorted_candidates {
        let contract_id = format!("m-{}-{}", cand.day, cand.position);
        let key = compute_memory_key(&cand.day, cand.position, &cand.text);
        let refs = extract_refs(&cand.text);
        let uri = format!("sol://chronicle/{}/talents/morning_briefing", cand.day);

        shown_contract_ids.push(contract_id.clone());
        memory_items.push(JsonMemoryItem {
            id: contract_id,
            key,
            day: &cand.day,
            text: &cand.text,
            source: JsonMemorySource {
                kind: "briefing",
                uri,
                briefing_day: &cand.day,
                refs,
            },
        });
    }

    let intro = if let Some(sentence) = intro_sentence {
        vec![JsonIntroItem {
            sentence,
            ids: shown_contract_ids.iter().map(String::as_str).collect(),
        }]
    } else {
        vec![]
    };

    let mut day_items = Vec::new();
    for report in &state.day_reports {
        if let Some(cand) = sorted_candidates.iter().find(|c| c.day == report.day) {
            let contract_id = format!("m-{}-{}", cand.day, cand.position);
            day_items.push(JsonDayItem {
                day: &report.day,
                state: DayClassification::Memory,
                memory_id: Some(contract_id),
            });
        } else {
            day_items.push(JsonDayItem {
                day: &report.day,
                state: report.classification,
                memory_id: None,
            });
        }
    }

    let mut said_items = Vec::new();
    let mut said_day = "";
    let mut said_n = 0usize;
    for said in &state.said {
        if said.day != said_day {
            said_day = &said.day;
            said_n = 0;
        }
        said_items.push(JsonSaidItem {
            id: format!("s-{}-{said_n}", said.day),
            key: compute_said_key(said),
            day: &said.day,
            quote: &said.quote,
            source: JsonSaidSource {
                kind: "activity",
                uri: said_uri(said),
            },
        });
        said_n += 1;
    }

    let doc = JsonDocument {
        version: 1,
        week: JsonWeek {
            start: &state.start_day,
            end: &state.end_day,
        },
        generated_at: &state.generated_at,
        selection: JsonSelection {
            status: selection_status,
            model: selection_model,
        },
        intro,
        memories: memory_items,
        days: day_items,
        said: said_items,
    };

    let document_str = serde_json::to_string(&doc).map_err(|e| {
        StageError::new(
            "commit",
            "weekly_reflection",
            "weekly_reflection",
            e.to_string(),
        )
    })?;

    let mut markdown_str = if n == 0 && state.said.is_empty() {
        "nothing from this week is on this page.\n".to_owned()
    } else if n == 0 {
        String::new()
    } else {
        let mut md = String::new();
        if let Some(intro_item) = doc.intro.first() {
            md.push_str(intro_item.sentence);
            md.push_str("\n\n");
        }

        for (idx, cand) in sorted_candidates.iter().enumerate() {
            if idx > 0 {
                md.push_str("\n\n");
            }
            let date = NaiveDate::parse_from_str(&cand.day, "%Y%m%d").unwrap_or(state.today);
            let weekday = english_weekday(date.weekday());
            let month = english_month(date.month());
            let day_num = date.day();

            let norm_text = normalize_markdown_text(&cand.text);
            md.push_str(&format!("{weekday}, {month} {day_num}\n{norm_text}\n[source](sol://chronicle/{}/talents/morning_briefing)", cand.day));
        }
        md.push('\n');
        md
    };

    if !state.said.is_empty() {
        if !markdown_str.is_empty() {
            markdown_str.push('\n');
        }
        markdown_str.push_str("said by you");
        let mut heading_day = "";
        for said in &state.said {
            if said.day != heading_day {
                heading_day = &said.day;
                let date = NaiveDate::parse_from_str(&said.day, "%Y%m%d").unwrap_or(state.today);
                markdown_str.push_str(&format!(
                    "\n\n{}, {} {}",
                    english_weekday(date.weekday()),
                    english_month(date.month()),
                    date.day()
                ));
            }
            let quote = normalize_markdown_text(&format!("\"{}\"", said.quote));
            markdown_str.push_str(&format!("\n{quote}\n[source]({})", said_uri(said)));
        }
        markdown_str.push('\n');
    }

    Ok((markdown_str, document_str))
}

fn said_uri(said: &Said) -> String {
    format!(
        "sol://facets/{}/activities/{}#{}",
        said.facet, said.day, said.record_id
    )
}

pub fn write_page(
    journal: &Path,
    start: &str,
    markdown: &str,
    document: &str,
) -> Result<(), StageError> {
    write_page_with_sources(journal, start, markdown, document, &[])
}

pub(crate) fn write_page_with_sources(
    journal: &Path,
    start: &str,
    markdown: &str,
    document: &str,
    sources: &[solstone_core_format::content::ConsumedOriginal],
) -> Result<(), StageError> {
    let target_dir = journal.join("reflections/weekly");
    fs::create_dir_all(&target_dir).map_err(|e| {
        StageError::new(
            "write",
            "weekly_reflection",
            "weekly_reflection",
            format!("failed to create directory {}: {e}", target_dir.display()),
        )
    })?;

    let md_path = target_dir.join(format!("{start}.md"));
    let json_path = target_dir.join(format!("{start}.json"));

    crate::writers::write_output_with_sources(md_path.clone(), markdown, sources).map_err(
        |error| {
            StageError::new(
                "write",
                "weekly_reflection",
                "weekly_reflection",
                format!(
                    "failed to write markdown sources {}: {error}",
                    md_path.display()
                ),
            )
        },
    )?;
    crate::writers::write_output_with_sources(json_path.clone(), document, sources).map_err(
        |error| {
            StageError::new(
                "write",
                "weekly_reflection",
                "weekly_reflection",
                format!(
                    "failed to write document sources {}: {error}",
                    json_path.display()
                ),
            )
        },
    )?;

    match rescan_file(journal, &md_path) {
        Ok(RescanFileStatus::Indexed { warnings }) => {
            for warning in warnings {
                index_warning(&format!(
                    "weekly_reflection index warning for {}: {warning}",
                    md_path.display()
                ));
            }
        }
        Ok(RescanFileStatus::Declined) => {}
        Err(err) => {
            index_warning(&format!(
                "weekly_reflection index error for {}: {err}",
                md_path.display()
            ));
        }
    }

    Ok(())
}

fn assess_day(journal: &Path, day: &str) -> (DayClassification, Vec<Candidate>) {
    let day_dir = journal.join("chronicle").join(day);
    let meta = match fs::metadata(&day_dir) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return (DayClassification::NothingShared, Vec::new());
        }
        Err(_) => {
            return (DayClassification::Unreadable, Vec::new());
        }
    };

    if !meta.is_dir() {
        return (DayClassification::Unreadable, Vec::new());
    }

    let has_segments = match iter_segments(journal, solstone_core_journal_io::PathOrDay::Day(day)) {
        Ok(segments) => !segments.is_empty(),
        Err(_) => return (DayClassification::Unreadable, Vec::new()),
    };

    if !has_segments {
        return (DayClassification::NothingShared, Vec::new());
    }

    let briefing_path = day_dir.join("talents/morning_briefing.json");
    let bytes = match fs::read(&briefing_path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return (DayClassification::NotReady, Vec::new());
        }
        Err(_) => {
            return (DayClassification::Unreadable, Vec::new());
        }
    };

    let val: Value = match serde_json::from_slice(&bytes) {
        Ok(v) => v,
        Err(_) => return (DayClassification::Unreadable, Vec::new()),
    };

    let obj = match val.as_object() {
        Some(o) => o,
        None => return (DayClassification::Unreadable, Vec::new()),
    };

    let yesterday_arr = match obj.get("yesterday").and_then(Value::as_array) {
        Some(a) => a,
        None => return (DayClassification::Unreadable, Vec::new()),
    };

    let candidates = extract_candidates(day, yesterday_arr);
    if candidates.is_empty() {
        (DayClassification::NotOnPage, Vec::new())
    } else {
        (DayClassification::NotOnPage, candidates)
    }
}

pub fn extract_candidates(day: &str, yesterday_arr: &[Value]) -> Vec<Candidate> {
    let mut initial: Vec<Candidate> = Vec::new();
    let mut seen_norm = BTreeSet::new();

    let re = placeholder_regex();

    for (pos, val) in yesterday_arr.iter().enumerate() {
        let Some(s) = val.as_str() else { continue };
        let trimmed = s.trim();
        if trimmed.is_empty() {
            continue;
        }

        let norm = trimmed
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase();
        if !seen_norm.insert(norm) {
            continue;
        }

        let is_placeholder = re.is_match(trimmed);
        let word_count = trimmed.split_whitespace().count();

        initial.push(Candidate {
            day: day.to_owned(),
            position: pos,
            text: s.to_owned(),
            placeholder: is_placeholder,
            word_count,
        });
    }

    if initial.is_empty() {
        return Vec::new();
    }

    initial.sort_by(|a, b| {
        a.placeholder
            .cmp(&b.placeholder)
            .then_with(|| (a.word_count > 90).cmp(&(b.word_count > 90)))
            .then_with(|| a.word_count.cmp(&b.word_count))
            .then_with(|| a.position.cmp(&b.position))
    });

    let has_non_placeholder = initial.iter().any(|c| !c.placeholder);
    if has_non_placeholder {
        initial.retain(|c| !c.placeholder);
    }

    let has_fits_word_limit = initial.iter().any(|c| c.word_count <= 90);
    if has_fits_word_limit {
        initial.retain(|c| c.word_count <= 90);
    }

    initial.truncate(3);
    initial
}

fn configured_day(prepared: &PreparedTalent) -> String {
    prepared
        .config
        .get("day")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned()
}

#[cfg(test)]
mod tests {
    #[test]
    fn provider_model_identifier_does_not_repeat_the_provider() {
        assert_eq!(
            provider_model_identifier("local", "local/qwen3.5-4b"),
            "local/qwen3.5-4b"
        );
        assert_eq!(
            provider_model_identifier("google", "gemini-custom-flash-test"),
            "google/gemini-custom-flash-test"
        );
        assert_eq!(
            provider_model_identifier("local", "localish/model"),
            "local/localish/model"
        );
    }

    use super::*;
    use serde_json::json;
    use tempfile::tempdir;

    struct StageClockGuard;
    impl Drop for StageClockGuard {
        fn drop(&mut self) {
            set_pinned_stage_now(None);
        }
    }

    fn pin_stage_now(dt_str: &str) -> StageClockGuard {
        set_pinned_stage_now(Some(
            DateTime::parse_from_rfc3339(dt_str)
                .unwrap()
                .with_timezone(&Utc),
        ));
        StageClockGuard
    }

    fn write_denver_journal_config(journal: &Path) {
        let cfg_dir = journal.join("config");
        fs::create_dir_all(&cfg_dir).unwrap();
        fs::write(
            cfg_dir.join("journal.json"),
            r#"{"identity":{"timezone":"America/Denver"},"providers":{"active":{"provider":"google","model":"gemini"}}}"#,
        )
        .unwrap();
    }

    #[test]
    fn test_compute_memory_key() {
        let key1 = compute_memory_key("20260308", 0, "Juliet presented the Verona platform.");
        assert_eq!(key1, "5af81e9aa81d98a1");

        let key2 = compute_memory_key("20260920", 1, "see ([n](sol://facets/work/news/20260326)).");
        assert_eq!(key2, "328313e3c1c3de17");

        let key3 = compute_memory_key("20260920", 0, "Speaker_2 joined the call.");
        assert_eq!(key3, "7f3b46a4e97d4c3c");

        assert_ne!(
            compute_memory_key("20260309", 0, "Juliet presented the Verona platform."),
            "5af81e9aa81d98a1"
        );
        assert_ne!(
            compute_memory_key("20260308", 1, "Juliet presented the Verona platform."),
            "5af81e9aa81d98a1"
        );
        assert_ne!(
            compute_memory_key("20260308", 0, "Other text"),
            "5af81e9aa81d98a1"
        );
    }

    #[test]
    fn test_extract_refs() {
        let text = "see ([n](sol://facets/work/news/20260326)).";
        let refs = extract_refs(text);
        assert_eq!(refs, vec!["sol://facets/work/news/20260326"]);

        let repeated = "sol://facets/work/news/20260326 and sol://facets/work/news/20260326";
        assert_eq!(
            extract_refs(repeated),
            vec!["sol://facets/work/news/20260326"]
        );

        let bare = "Visit sol://facets/work/news/20260326. Also sol://other/path, check it.";
        assert_eq!(
            extract_refs(bare),
            vec![
                "sol://facets/work/news/20260326".to_owned(),
                "sol://other/path".to_owned()
            ]
        );
    }

    #[test]
    fn test_normalize_markdown_text() {
        assert_eq!(normalize_markdown_text("hello \n world"), "hello world");
        assert_eq!(normalize_markdown_text("line1\n\n\nline2"), "line1 line2");
        assert_eq!(
            normalize_markdown_text("   hello \r\n  world   "),
            "hello world   "
        );
        assert_eq!(normalize_markdown_text("# Heading"), "\\# Heading");
        assert_eq!(normalize_markdown_text("- Bullet"), "\\- Bullet");
        assert_eq!(normalize_markdown_text("* Bullet"), "\\* Bullet");
        assert_eq!(normalize_markdown_text("+ Bullet"), "\\+ Bullet");
        assert_eq!(normalize_markdown_text("> Quote"), "\\> Quote");
        assert_eq!(normalize_markdown_text("```code"), "\\```code");
        assert_eq!(normalize_markdown_text("~~~code"), "\\~~~code");
        assert_eq!(normalize_markdown_text("1. Numbered"), "1\\. Numbered");
        assert_eq!(normalize_markdown_text("12. Item"), "12\\. Item");
        assert_eq!(normalize_markdown_text("Plain text."), "Plain text.");

        let raw_memory_text = "First part of memory.\nSecond part of memory.";
        let norm = normalize_markdown_text(raw_memory_text);
        assert_eq!(norm, "First part of memory. Second part of memory.");
        assert!(raw_memory_text.contains('\n'));
    }

    #[test]
    fn test_day_states() {
        let journal = tempdir().unwrap();

        let (st1, c1) = assess_day(journal.path(), "20260308");
        assert_eq!(st1, DayClassification::NothingShared);
        assert!(c1.is_empty());

        let day_dir = journal.path().join("chronicle/20260308");
        fs::create_dir_all(day_dir.join("talents")).unwrap();
        fs::write(
            day_dir.join("talents/morning_briefing.json"),
            r#"{"yesterday":[]}"#,
        )
        .unwrap();
        let (st2, c2) = assess_day(journal.path(), "20260308");
        assert_eq!(st2, DayClassification::NothingShared);
        assert!(c2.is_empty());

        fs::write(
            day_dir.join("talents/morning_briefing.json"),
            r#"{"yesterday":["Usable bullet text."]}"#,
        )
        .unwrap();
        let (st3, c3) = assess_day(journal.path(), "20260308");
        assert_eq!(st3, DayClassification::NothingShared);
        assert!(c3.is_empty());

        fs::create_dir_all(day_dir.join("default/120000_60")).unwrap();

        fs::write(
            day_dir.join("talents/morning_briefing.json"),
            r#"{"yesterday":[]}"#,
        )
        .unwrap();
        let (st4, c4) = assess_day(journal.path(), "20260308");
        assert_eq!(st4, DayClassification::NotOnPage);
        assert!(c4.is_empty());

        fs::remove_file(day_dir.join("talents/morning_briefing.json")).unwrap();
        let (st5, c5) = assess_day(journal.path(), "20260308");
        assert_eq!(st5, DayClassification::NotReady);
        assert!(c5.is_empty());

        fs::write(
            day_dir.join("talents/morning_briefing.json"),
            "invalid json",
        )
        .unwrap();
        let (st6, c6) = assess_day(journal.path(), "20260308");
        assert_eq!(st6, DayClassification::Unreadable);
        assert!(c6.is_empty());

        fs::write(
            day_dir.join("talents/morning_briefing.json"),
            r#"{"metadata":{}}"#,
        )
        .unwrap();
        let (st7, c7) = assess_day(journal.path(), "20260308");
        assert_eq!(st7, DayClassification::Unreadable);
        assert!(c7.is_empty());

        fs::write(
            day_dir.join("talents/morning_briefing.json"),
            r#"{"yesterday":"string"}"#,
        )
        .unwrap();
        let (st8, c8) = assess_day(journal.path(), "20260308");
        assert_eq!(st8, DayClassification::Unreadable);
        assert!(c8.is_empty());

        fs::write(
            day_dir.join("talents/morning_briefing.json"),
            r#"{"yesterday":["Usable bullet text."]}"#,
        )
        .unwrap();
        let (st9, c9) = assess_day(journal.path(), "20260308");
        assert_eq!(st9, DayClassification::NotOnPage);
        assert_eq!(c9.len(), 1);

        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            let loop_journal = tempdir().unwrap();
            let loop_day_dir = loop_journal.path().join("chronicle/20260308");
            fs::create_dir_all(loop_day_dir.join("default/120000_60")).unwrap();
            fs::create_dir_all(loop_day_dir.join("talents")).unwrap();
            let link_target = loop_day_dir.join("talents/morning_briefing.json");
            symlink(&link_target, &link_target).unwrap();
            let (st_loop, _) = assess_day(loop_journal.path(), "20260308");
            assert_eq!(st_loop, DayClassification::Unreadable);

            let loop_day_root = loop_journal.path().join("chronicle/20260309");
            symlink(&loop_day_root, &loop_day_root).unwrap();
            let (st_day_loop, _) = assess_day(loop_journal.path(), "20260309");
            assert_eq!(st_day_loop, DayClassification::Unreadable);
        }
    }

    #[test]
    fn test_candidate_selection_and_dedupe() {
        let raw = vec![
            Value::String("  Duplicate Bullet Text  ".to_owned()),
            Value::String("duplicate bullet text".to_owned()),
            Value::String("DUPLICATE   BULLET   TEXT".to_owned()),
            Value::String("Unique bullet".to_owned()),
        ];
        let candidates = extract_candidates("20260308", &raw);
        assert_eq!(candidates.len(), 2);
        assert_eq!(candidates[0].text, "Unique bullet");
        assert_eq!(candidates[1].text, "  Duplicate Bullet Text  ");

        let day1_cands = extract_candidates("20260308", &[Value::String("Same text".to_owned())]);
        let day2_cands = extract_candidates("20260309", &[Value::String("Same text".to_owned())]);
        assert_eq!(day1_cands.len(), 1);
        assert_eq!(day2_cands.len(), 1);

        let placeholder_samples = vec![
            Value::String("Speaker_2: notes".to_owned()),
            Value::String("speaker2 did something".to_owned()),
            Value::String("SPEAKER 3 spoke".to_owned()),
            Value::String("loudspeaker 3 announcement".to_owned()),
        ];
        let cands_ph = extract_candidates("20260308", &placeholder_samples);
        assert_eq!(cands_ph.len(), 1);
        assert_eq!(cands_ph[0].text, "loudspeaker 3 announcement");
        assert!(!cands_ph[0].placeholder);

        let mixed = vec![
            Value::Number(42.into()),
            Value::Bool(true),
            Value::String("   ".to_owned()),
            Value::String("".to_owned()),
            Value::String("Valid 1".to_owned()),
            Value::String("Valid 2".to_owned()),
            Value::String("Valid 3".to_owned()),
            Value::String("Valid 4".to_owned()),
        ];
        let cands_mixed = extract_candidates("20260308", &mixed);
        assert_eq!(cands_mixed.len(), 3);

        let short = Value::String("A short sentence under ninety words.".to_owned());
        let long_words = (0..100)
            .map(|i| format!("w{i}"))
            .collect::<Vec<_>>()
            .join(" ");
        let long = Value::String(long_words.clone());
        let cands_words = extract_candidates("20260308", &[short, long.clone()]);
        assert_eq!(cands_words.len(), 1);
        assert!(cands_words[0].word_count <= 90);

        let sole_oversize = extract_candidates("20260308", &[long]);
        assert_eq!(sole_oversize.len(), 1);
        assert!(sole_oversize[0].word_count > 90);
    }

    #[test]
    fn test_prompt_override() {
        let _guard = pin_stage_now("2026-03-16T18:00:00Z");
        let journal = tempdir().unwrap();
        write_denver_journal_config(journal.path());
        let context = ExecutionContext {
            journal: journal.path().to_owned(),
        };
        let day_dir = journal.path().join("chronicle/20260308");
        fs::create_dir_all(day_dir.join("default/120000_60")).unwrap();
        fs::create_dir_all(day_dir.join("talents")).unwrap();
        fs::write(
            day_dir.join("talents/morning_briefing.json"),
            r#"{"yesterday":["Memory with $name and $$ symbols."]}"#,
        )
        .unwrap();

        let mut prepared = PreparedTalent {
            name: "weekly_reflection".to_owned(),
            config: Map::from_iter([
                ("max_output_tokens".to_owned(), json!(1024)),
                ("day".to_owned(), Value::String("20260308".to_owned())),
                ("today".to_owned(), Value::String("20260316".to_owned())),
                ("prompt".to_owned(), Value::String("old prompt".to_owned())),
            ]),
        };

        let state = build(&mut prepared, &context).unwrap();

        apply_prompt_override(&mut prepared, &state).unwrap();
        assert!(!prepared.config.contains_key("prompt"));

        let transcript: Value = serde_json::from_str(
            prepared
                .config
                .get("transcript")
                .and_then(Value::as_str)
                .unwrap(),
        )
        .unwrap();
        assert_eq!(transcript["week"], json!(["20260308", "20260314"]));
        let memory_text = transcript["memories"][0]["text"].as_str().unwrap();
        assert!(memory_text.contains("$name"));
        assert!(memory_text.contains("$$"));

        let schema = prepared.config.get("json_schema").unwrap();
        assert_eq!(schema["type"], "object");
        assert_eq!(schema["properties"]["selections"]["type"], "object");
        assert_eq!(
            schema["properties"]["selections"]["required"],
            json!(["S00"])
        );
        assert_eq!(
            solstone_core_generate_wire::anthropic_schema_violations(schema),
            Vec::<String>::new()
        );
    }

    #[test]
    fn test_week_guard() {
        let _guard = pin_stage_now("2026-09-30T18:00:00Z");
        let journal = tempdir().unwrap();
        write_denver_journal_config(journal.path());
        let context = ExecutionContext {
            journal: journal.path().to_owned(),
        };

        let check_fail = |day_val: Option<&str>| {
            let mut config = Map::new();
            if let Some(d) = day_val {
                config.insert("day".to_owned(), Value::String(d.to_owned()));
            }
            let mut prepared = PreparedTalent {
                name: "weekly_reflection".to_owned(),
                config,
            };
            let result = build(&mut prepared, &context);
            assert!(matches!(result, Err(RuntimeOutcome::StageFailed(_))));
            assert!(!journal.path().join("reflections/weekly").exists());
        };

        check_fail(None);
        check_fail(Some("20260930"));
        check_fail(Some("20990104"));
        check_fail(Some("20260927"));

        let mut prep_ok = PreparedTalent {
            name: "weekly_reflection".to_owned(),
            config: Map::from_iter([
                ("max_output_tokens".to_owned(), json!(1024)),
                ("day".to_owned(), Value::String("20200105".to_owned())),
            ]),
        };
        let res_ok = build(&mut prep_ok, &context);
        assert!(res_ok.is_ok());
    }

    #[test]
    fn test_attribution() {
        let _guard = pin_stage_now("2026-09-30T18:00:00Z");
        let journal = tempdir().unwrap();
        write_denver_journal_config(journal.path());
        let context = ExecutionContext {
            journal: journal.path().to_owned(),
        };

        let day1 = journal.path().join("chronicle/20260920");
        fs::create_dir_all(day1.join("default/120000_60")).unwrap();
        fs::create_dir_all(day1.join("talents")).unwrap();
        fs::write(
            day1.join("talents/morning_briefing.json"),
            r#"{"yesterday":["Target memory bullet."]}"#,
        )
        .unwrap();

        let day2 = journal.path().join("chronicle/20260921");
        fs::create_dir_all(day2.join("default/120000_60")).unwrap();
        fs::create_dir_all(day2.join("talents")).unwrap();
        fs::write(
            day2.join("talents/morning_briefing.json"),
            r#"{"yesterday":["Decoy memory bullet."]}"#,
        )
        .unwrap();

        let mut prepared = PreparedTalent {
            name: "weekly_reflection".to_owned(),
            config: Map::from_iter([
                ("max_output_tokens".to_owned(), json!(1024)),
                ("day".to_owned(), Value::String("20260920".to_owned())),
                ("provider".to_owned(), Value::String("google".to_owned())),
                ("model".to_owned(), Value::String("gemini".to_owned())),
            ]),
        };

        let pre_state = build(&mut prepared, &context).unwrap();
        let PrePostState::WeeklyReflection(ref state) = pre_state else {
            panic!("expected weekly reflection state");
        };
        assert_eq!(state.slots.len(), 2);

        let parsed_both = ParsedOutput::Json(json!({"S00": "M000", "S01": "M001"}));
        let plan = commit(parsed_both, &prepared, &pre_state).unwrap();
        let CommitPlan::Write(WriteIntent::WeeklyReflection { document, .. }) = plan else {
            panic!("expected Write plan");
        };

        let doc: Value = serde_json::from_str(&document).unwrap();
        let m0 = &doc["memories"][0];
        assert_eq!(m0["day"], "20260920");
        assert_eq!(m0["source"]["briefing_day"], "20260920");
        assert_eq!(m0["text"], "Target memory bullet.");
    }

    #[test]
    fn test_intro() {
        let state = WeeklyReflectionState {
            start_day: "20260308".to_owned(),
            end_day: "20260314".to_owned(),
            today: NaiveDate::from_ymd_opt(2026, 3, 16).unwrap(),
            generated_at: "2026-03-15T18:00:00Z".to_owned(),
            day_reports: vec![],
            slots: vec![],
            all_candidates: vec![],
            said: vec![],
        };

        let (md0, doc0_str) = assemble_reflection(&state, "none", None, &[]).unwrap();
        assert_eq!(md0, "nothing from this week is on this page.\n");
        let doc0: Value = serde_json::from_str(&doc0_str).unwrap();
        assert_eq!(doc0["intro"], json!([]));

        let cand1 = Candidate {
            day: "20260308".to_owned(),
            position: 0,
            text: "Single memory".to_owned(),
            placeholder: false,
            word_count: 2,
        };
        let (md1, doc1_str) =
            assemble_reflection(&state, "model", Some("model"), std::slice::from_ref(&cand1))
                .unwrap();
        assert!(md1.starts_with("your week, a memory from one day.\n\n"));
        let doc1: Value = serde_json::from_str(&doc1_str).unwrap();
        assert_eq!(
            doc1["intro"][0],
            json!({
                "sentence": "your week, a memory from one day.",
                "ids": ["m-20260308-0"]
            })
        );

        let cand2 = Candidate {
            day: "20260309".to_owned(),
            position: 0,
            text: "Second memory".to_owned(),
            placeholder: false,
            word_count: 2,
        };
        let cand3 = Candidate {
            day: "20260310".to_owned(),
            position: 0,
            text: "Third memory".to_owned(),
            placeholder: false,
            word_count: 2,
        };
        let (md3, doc3_str) =
            assemble_reflection(&state, "model", Some("model"), &[cand1, cand2, cand3]).unwrap();
        assert!(md3.starts_with("your week, a memory from each of 3 days.\n\n"));
        let doc3: Value = serde_json::from_str(&doc3_str).unwrap();
        assert_eq!(
            doc3["intro"][0],
            json!({
                "sentence": "your week, a memory from each of 3 days.",
                "ids": ["m-20260308-0", "m-20260309-0", "m-20260310-0"]
            })
        );
    }

    #[test]
    fn said_keeps_only_the_owners_voice_backed_quotes() {
        let root = tempfile::tempdir().unwrap();
        solstone_core_facets::create_facet(root.path(), "work", "Work", "", "", "", None).unwrap();
        let write = |path: &str, text: String| {
            let p = root.path().join(path);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, text).unwrap();
        };
        write(
            "config/journal.json",
            json!({"identity":{"name":"Jordan Rivers"}}).to_string(),
        );
        write(
            "entities/jordan/entity.json",
            json!({"id":"jordan","name":"Jordan Rivers","type":"Person","is_principal":true})
                .to_string(),
        );

        let path = root.path().join("facets/work/activities/20260309.jsonl");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let rows = [
            json!({
                "id":"meeting_1",
                "activity":"meeting",
                "stream":"phone",
                "segments":["090000_300"],
                "story":{"body":"You promised \"I will get you the deck by friday\" before leaving."}
            }),
            json!({
                "id":"gone",
                "activity":"meeting",
                "segments":["100000_300"],
                "story":{"body":"You said \"never shown anywhere at all\"."}
            }),
            json!({
                "id":"meeting_pat",
                "activity":"meeting",
                "stream":"default",
                "segments":["110000_300"],
                "story":{"body":"Pat stated \"we can book the room later\"."}
            }),
            json!({
                "id":"chat_1",
                "activity":"chat",
                "story":{"body":"You typed \"we should meet tomorrow morning\"."}
            }),
        ];
        fs::write(
            &path,
            rows.iter()
                .map(|row| format!("{row}\n"))
                .collect::<String>(),
        )
        .unwrap();

        let day_dir = root.path().join("chronicle/20260309");
        fs::create_dir_all(day_dir.join("phone/090000_300")).unwrap();
        fs::create_dir_all(day_dir.join("default/110000_300")).unwrap();
        // The same key under another stream does not keep a stream-bound record.
        fs::create_dir_all(day_dir.join("default/100000_300")).unwrap();
        let mut bound: Map<String, Value> =
            serde_json::from_value(json!({"stream":"phone","segments":["100000_300"]})).unwrap();
        assert!(!keeps_a_segment(root.path(), "20260309", &bound));
        bound.remove("stream");
        assert!(keeps_a_segment(root.path(), "20260309", &bound));
        // A deleted segment leaves its directory behind as a tombstone.
        fs::write(day_dir.join("default/100000_300/tombstone.json"), "{}").unwrap();
        assert!(!keeps_a_segment(root.path(), "20260309", &bound));

        write(
            "chronicle/20260309/phone/090000_300/audio.jsonl",
            r#"{"start":"00:00:01","speaker":1,"sentence_id":1,"text":"I will get you the deck by friday"}"#
                .into(),
        );
        write(
            "chronicle/20260309/phone/090000_300/talents/speaker_labels.json",
            json!({"labels":[{"sentence_id":1,"speaker":"jordan","method":"user_confirmed"}]})
                .to_string(),
        );

        write(
            "chronicle/20260309/default/110000_300/audio.jsonl",
            r#"{"start":"00:00:01","speaker":2,"sentence_id":1,"text":"we can book the room later"}"#
                .into(),
        );
        write(
            "chronicle/20260309/default/110000_300/talents/speaker_labels.json",
            json!({"labels":[{"sentence_id":1,"speaker":"pat","method":"voice"}]}).to_string(),
        );

        let said = said_on_day(root.path(), &["work".to_owned()], "20260309");
        assert_eq!(said.len(), 1);
        assert_eq!(said[0].quote, "I will get you the deck by friday");
        assert_eq!(said[0].day, "20260309");
        assert_eq!(said[0].facet, "work");
        assert_eq!(said[0].record_id, "meeting_1");
        assert_eq!(
            said_uri(&said[0]),
            "sol://facets/work/activities/20260309#meeting_1"
        );
        assert!(said_on_day(root.path(), &["work".to_owned()], "20260310").is_empty());
    }

    #[test]
    fn said_renders_the_quote_and_never_the_action() {
        let mut state = WeeklyReflectionState {
            start_day: "20260308".to_owned(),
            end_day: "20260314".to_owned(),
            today: NaiveDate::from_ymd_opt(2026, 3, 16).unwrap(),
            generated_at: "2026-03-15T18:00:00Z".to_owned(),
            day_reports: vec![],
            slots: vec![],
            all_candidates: vec![],
            said: vec![Said {
                day: "20260309".to_owned(),
                facet: "work".to_owned(),
                record_id: "meeting_1".to_owned(),
                quote: "I'll get you the deck by friday".to_owned(),
            }],
        };

        let (md, doc) = assemble_reflection(&state, "none", None, &[]).unwrap();
        assert_eq!(
            md,
            "said by you\n\nmonday, march 9\n\"I'll get you the deck by friday\"\n[source](sol://facets/work/activities/20260309#meeting_1)\n"
        );
        let mut two = state.clone();
        two.said.push(Said {
            quote: "let's go with blue then".to_owned(),
            ..state.said[0].clone()
        });
        let (md_two, doc_two) = assemble_reflection(&two, "none", None, &[]).unwrap();
        // One heading per day, with that day's quotes under it.
        assert_eq!(md_two.matches("monday, march 9").count(), 1);
        assert!(md_two.ends_with("#meeting_1)\n\"let's go with blue then\"\n[source](sol://facets/work/activities/20260309#meeting_1)\n"));
        let doc_two: Value = serde_json::from_str(&doc_two).unwrap();
        assert_eq!(doc_two["said"][1]["id"], "s-20260309-1");
        assert_ne!(doc_two["said"][0]["key"], doc_two["said"][1]["key"]);
        let doc: Value = serde_json::from_str(&doc).unwrap();
        assert_eq!(doc["version"], 1);
        assert_eq!(doc["intro"], json!([]));
        let entry = &doc["said"][0];
        assert_eq!(entry["id"], "s-20260309-0");
        assert_eq!(entry["quote"], "I'll get you the deck by friday");
        assert_eq!(entry["source"]["kind"], "activity");
        assert_eq!(
            entry["source"]["uri"],
            "sol://facets/work/activities/20260309#meeting_1"
        );
        assert_eq!(entry["key"], compute_said_key(&state.said[0]));
        assert!(entry.get("action").is_none());

        let memory = Candidate {
            day: "20260308".to_owned(),
            position: 0,
            text: "A memory".to_owned(),
            placeholder: false,
            word_count: 2,
        };
        let (md, _) = assemble_reflection(
            &state,
            "model",
            Some("model"),
            std::slice::from_ref(&memory),
        )
        .unwrap();
        assert!(md.starts_with("your week, a memory from one day.\n\nsunday, march 8\nA memory\n[source](sol://chronicle/20260308/talents/morning_briefing)\n\nsaid by you\n\nmonday, march 9\n"));

        state.said.clear();
        let (md, doc) = assemble_reflection(&state, "none", None, &[]).unwrap();
        assert_eq!(md, "nothing from this week is on this page.\n");
        assert_eq!(
            serde_json::from_str::<Value>(&doc).unwrap()["said"],
            json!([])
        );
    }

    #[test]
    fn test_write_page() {
        let journal = tempdir().unwrap();
        let target_dir = journal.path().join("reflections/weekly");
        fs::create_dir_all(&target_dir).unwrap();

        let json_path = target_dir.join("20260308.json");
        fs::write(&json_path, "pre-existing json").unwrap();
        let md_dir = target_dir.join("20260308.md");
        fs::create_dir(&md_dir).unwrap();

        let err1 = write_page(journal.path(), "20260308", "new md", "new json");
        assert!(err1.is_err());
        assert_eq!(fs::read_to_string(&json_path).unwrap(), "pre-existing json");

        fs::remove_dir(&md_dir).unwrap();

        fs::remove_file(&json_path).unwrap();
        fs::create_dir(&json_path).unwrap();

        let err2 = write_page(journal.path(), "20260308", "new md content\n", "new json");
        assert!(err2.is_err());
        assert_eq!(
            fs::read_to_string(target_dir.join("20260308.md")).unwrap(),
            "new md content\n"
        );
    }

    #[test]
    fn test_fixture_journal_assembly_and_byte_equality() {
        let _guard = pin_stage_now("2026-03-15T18:00:00Z");
        let fixture_journal = Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(3)
            .expect("workspace root")
            .join("tests/fixtures/journal");

        let context = ExecutionContext {
            journal: fixture_journal.clone(),
        };
        let mut prepared = PreparedTalent {
            name: "weekly_reflection".to_owned(),
            config: Map::from_iter([
                ("max_output_tokens".to_owned(), json!(1024)),
                ("day".to_owned(), Value::String("20260308".to_owned())),
                ("today".to_owned(), Value::String("20260316".to_owned())),
                ("provider".to_owned(), Value::String("google".to_owned())),
                (
                    "model".to_owned(),
                    Value::String("gemini-custom-flash-test".to_owned()),
                ),
            ]),
        };

        let pre_state = build(&mut prepared, &context).expect("build against fixtures journal");
        let PrePostState::WeeklyReflection(ref state) = pre_state else {
            panic!("expected weekly reflection pre state");
        };

        assert_eq!(state.slots.len(), 3);
        assert_eq!(state.day_reports.len(), 7);

        let parsed = ParsedOutput::Json(json!({
            "S00": "M000",
            "S01": "M001",
            "S02": "M002",
        }));

        let plan = commit(parsed, &prepared, &pre_state).expect("commit against fixtures journal");
        let CommitPlan::Write(WriteIntent::WeeklyReflection {
            start,
            markdown,
            document,
        }) = plan
        else {
            panic!("expected Write plan");
        };
        assert_eq!(start, "20260308");

        let fixture_md_path = fixture_journal.join("reflections/weekly/20260308.md");
        let fixture_json_path = fixture_journal.join("reflections/weekly/20260308.json");

        let loaded_md = fs::read_to_string(&fixture_md_path).unwrap();
        let loaded_json = fs::read_to_string(&fixture_json_path).unwrap();

        assert_eq!(markdown, loaded_md);
        assert_eq!(document, loaded_json);
    }

    #[cfg(all(test, feature = "full-tests"))]
    mod full_tests {
        use super::*;
        use crate::test_support::{
            generated_response_value, refused_response_value, sequenced_one_shot_stub,
        };
        use crate::{OneShotClient, execute_request};
        use serde_json::json;
        use std::fs;
        use std::path::{Path, PathBuf};
        use tempfile::tempdir;

        fn test_fixture(
            day: &str,
            bullets: &[&str],
            with_segments: bool,
        ) -> (
            tempfile::TempDir,
            crate::prepare::RuntimePaths,
            ExecutionContext,
        ) {
            let root = tempdir().unwrap();
            let talent_root = root.path().join("talent");
            let apps_root = root.path().join("apps");
            let templates_dir = root.path().join("templates");
            fs::create_dir_all(&talent_root).unwrap();
            fs::create_dir_all(&apps_root).unwrap();
            fs::create_dir_all(&templates_dir).unwrap();

            fs::write(
                talent_root.join("weekly_reflection.md"),
                r#"{
  "type": "generate", "max_output_tokens": 1024,
  "schedule": "weekly",
  "hook": {
    "pre": "weekly_reflection",
    "post": "weekly_reflection"
  }
}
Weekly prompt
"#,
            )
            .unwrap();

            let paths = crate::prepare::RuntimePaths {
                talent_root,
                apps_root,
                templates_dir,
            };
            let journal = root.path().join("journal");
            let context = ExecutionContext {
                journal: journal.clone(),
            };
            fs::create_dir_all(&journal).unwrap();
            write_denver_journal_config(&journal);

            let day_dir = journal.join("chronicle").join(day);
            if with_segments {
                fs::create_dir_all(day_dir.join("default/080000_300")).unwrap();
            }
            if !bullets.is_empty() {
                fs::create_dir_all(day_dir.join("talents")).unwrap();
                let doc = json!({
                    "version": 1,
                    "generated_at": "2026-03-09T07:00:00-06:00",
                    "today": {
                        "date": day,
                        "events": [],
                        "reminders": [],
                        "schedule_summary": "Quiet day."
                    },
                    "yesterday": bullets,
                });
                fs::write(
                    day_dir.join("talents/morning_briefing.json"),
                    serde_json::to_string(&doc).unwrap(),
                )
                .unwrap();
            }

            (root, paths, context)
        }

        fn parse_events(output: &[u8]) -> Vec<Value> {
            String::from_utf8_lossy(output)
                .lines()
                .filter_map(|line| serde_json::from_str(line).ok())
                .collect()
        }

        fn stub_invocation_count(stub: &Path) -> usize {
            let count_file = PathBuf::from(format!("{}.count", stub.display()));
            fs::read_to_string(count_file)
                .map(|s| s.trim().parse::<usize>().unwrap_or(0))
                .unwrap_or(0)
        }

        #[test]
        fn test_full_zero_slots_no_segments() {
            let _guard = pin_stage_now("2026-03-15T18:00:00Z");
            let (root, paths, context) = test_fixture("20260308", &[], false);

            let stub = sequenced_one_shot_stub(root.path(), &[]);
            let generate = OneShotClient::at_path(&stub);
            let mut output = Vec::new();

            let outcome = execute_request(
                json!({
                    "name": "weekly_reflection",
                    "day": "20260308",
                    "today": "20260316",
                })
                .as_object()
                .unwrap()
                .clone(),
                &paths,
                &context,
                &generate,
                &mut output,
            );

            assert!(
                matches!(outcome, RuntimeOutcome::Finished { .. }),
                "{outcome:?}"
            );
            assert_eq!(stub_invocation_count(&stub), 0);

            let events = parse_events(&output);
            assert_eq!(
                events
                    .iter()
                    .filter(|e| e["event"] == "unavailable_selection")
                    .count(),
                0
            );

            let md_path = context.journal.join("reflections/weekly/20260308.md");
            let json_path = context.journal.join("reflections/weekly/20260308.json");
            assert_eq!(
                fs::read_to_string(&md_path).unwrap(),
                "nothing from this week is on this page.\n"
            );
            let doc: Value =
                serde_json::from_str(&fs::read_to_string(&json_path).unwrap()).unwrap();
            assert_eq!(doc["selection"]["status"], "none");
            assert!(doc["selection"]["model"].is_null());
            assert!(doc["intro"].as_array().unwrap().is_empty());
            assert!(doc["memories"].as_array().unwrap().is_empty());
            assert_eq!(doc["days"].as_array().unwrap().len(), 7);
        }

        #[test]
        fn test_full_zero_slots_segments_no_briefing() {
            let _guard = pin_stage_now("2026-03-15T18:00:00Z");
            let (root, paths, context) = test_fixture("20260308", &[], false);

            // Create segments on 20260308..=20260314, but no briefing files
            for d in [
                "20260308", "20260309", "20260310", "20260311", "20260312", "20260313", "20260314",
            ] {
                fs::create_dir_all(
                    context
                        .journal
                        .join("chronicle")
                        .join(d)
                        .join("default/080000_300"),
                )
                .unwrap();
            }

            let stub = sequenced_one_shot_stub(root.path(), &[]);
            let generate = OneShotClient::at_path(&stub);
            let mut output = Vec::new();

            let outcome = execute_request(
                json!({
                    "name": "weekly_reflection",
                    "day": "20260308",
                    "today": "20260316",
                })
                .as_object()
                .unwrap()
                .clone(),
                &paths,
                &context,
                &generate,
                &mut output,
            );

            assert!(
                matches!(outcome, RuntimeOutcome::Finished { .. }),
                "{outcome:?}"
            );
            assert_eq!(stub_invocation_count(&stub), 0);

            let json_path = context.journal.join("reflections/weekly/20260308.json");
            let doc: Value =
                serde_json::from_str(&fs::read_to_string(&json_path).unwrap()).unwrap();
            assert_eq!(doc["selection"]["status"], "none");
            assert!(doc["intro"].as_array().unwrap().is_empty());
            assert!(doc["memories"].as_array().unwrap().is_empty());
            let days = doc["days"].as_array().unwrap();
            assert_eq!(days.len(), 7);
            for day in days {
                assert_eq!(day["state"], "not_ready");
            }
        }

        #[test]
        fn test_full_no_segments_usable_yesterday_bullet() {
            let _guard = pin_stage_now("2026-03-15T18:00:00Z");
            let (root, paths, context) = test_fixture("20260308", &["Usable bullet text."], false);

            let stub = sequenced_one_shot_stub(root.path(), &[]);
            let generate = OneShotClient::at_path(&stub);
            let mut output = Vec::new();

            let outcome = execute_request(
                json!({
                    "name": "weekly_reflection",
                    "day": "20260308",
                    "today": "20260316",
                })
                .as_object()
                .unwrap()
                .clone(),
                &paths,
                &context,
                &generate,
                &mut output,
            );

            assert!(
                matches!(outcome, RuntimeOutcome::Finished { .. }),
                "{outcome:?}"
            );
            assert_eq!(stub_invocation_count(&stub), 0);

            let md_path = context.journal.join("reflections/weekly/20260308.md");
            let json_path = context.journal.join("reflections/weekly/20260308.json");
            let md = fs::read_to_string(&md_path).unwrap();
            let json_str = fs::read_to_string(&json_path).unwrap();

            assert!(!md.contains("Usable bullet text."));
            assert!(!json_str.contains("Usable bullet text."));

            let doc: Value = serde_json::from_str(&json_str).unwrap();
            assert_eq!(doc["selection"]["status"], "none");
            assert!(doc["intro"].as_array().unwrap().is_empty());
            assert!(doc["memories"].as_array().unwrap().is_empty());
        }

        #[test]
        fn test_full_schema_invalid_twice() {
            let _guard = pin_stage_now("2026-03-15T18:00:00Z");
            let (root, paths, context) = test_fixture("20260308", &["Alpha memory."], true);

            let stub = sequenced_one_shot_stub(
                root.path(),
                &[
                    generated_response_value(
                        "bad json 1",
                        json!({"valid": false, "errors": ["err1"]}),
                    ),
                    generated_response_value(
                        "bad json 2",
                        json!({"valid": false, "errors": ["err2"]}),
                    ),
                ],
            );
            let generate = OneShotClient::at_path(&stub);
            let mut output = Vec::new();

            let outcome = execute_request(
                json!({
                    "name": "weekly_reflection",
                    "day": "20260308",
                    "today": "20260316",
                })
                .as_object()
                .unwrap()
                .clone(),
                &paths,
                &context,
                &generate,
                &mut output,
            );

            assert!(
                matches!(outcome, RuntimeOutcome::Finished { .. }),
                "{outcome:?}"
            );
            assert_eq!(stub_invocation_count(&stub), 2);

            let events = parse_events(&output);
            let unavail: Vec<_> = events
                .iter()
                .filter(|e| e["event"] == "unavailable_selection")
                .collect();
            assert_eq!(unavail.len(), 1);
            assert_eq!(unavail[0]["trigger"], "schema_exhausted");

            let md_path = context.journal.join("reflections/weekly/20260308.md");
            let json_path = context.journal.join("reflections/weekly/20260308.json");
            let md = fs::read_to_string(&md_path).unwrap();
            assert!(md.contains("Alpha memory."));

            let doc: Value =
                serde_json::from_str(&fs::read_to_string(&json_path).unwrap()).unwrap();
            assert_eq!(doc["selection"]["status"], "fallback");
            assert_eq!(doc["selection"]["model"], "google/gemini");
            assert_eq!(doc["memories"].as_array().unwrap().len(), 1);
        }

        #[test]
        fn test_full_refusal_context_budget_exceeded() {
            let _guard = pin_stage_now("2026-03-15T18:00:00Z");
            let (root, paths, context) = test_fixture("20260308", &["Alpha memory."], true);

            let stub = sequenced_one_shot_stub(
                root.path(),
                &[refused_response_value(
                    Some("context_budget_exceeded"),
                    false,
                    true,
                    "google",
                    "context budget exceeded",
                )],
            );
            let generate = OneShotClient::at_path(&stub);
            let mut output = Vec::new();

            let outcome = execute_request(
                json!({
                    "name": "weekly_reflection",
                    "day": "20260308",
                    "today": "20260316",
                })
                .as_object()
                .unwrap()
                .clone(),
                &paths,
                &context,
                &generate,
                &mut output,
            );

            assert!(
                matches!(outcome, RuntimeOutcome::Finished { .. }),
                "{outcome:?}"
            );
            assert_eq!(stub_invocation_count(&stub), 1);

            let events = parse_events(&output);
            let unavail: Vec<_> = events
                .iter()
                .filter(|e| e["event"] == "unavailable_selection")
                .collect();
            assert_eq!(unavail.len(), 1);
            assert_eq!(unavail[0]["trigger"], "refused:context_budget_exceeded");

            let json_path = context.journal.join("reflections/weekly/20260308.json");
            let doc: Value =
                serde_json::from_str(&fs::read_to_string(&json_path).unwrap()).unwrap();
            assert_eq!(doc["selection"]["status"], "fallback");
        }

        #[test]
        fn test_full_refusal_incomplete_json_length_twice() {
            let _guard = pin_stage_now("2026-03-15T18:00:00Z");
            let (root, paths, context) = test_fixture("20260308", &["Alpha memory."], true);

            let stub = sequenced_one_shot_stub(
                root.path(),
                &[
                    refused_response_value(
                        Some("incomplete_json_length"),
                        true,
                        true,
                        "google",
                        "cut off 1",
                    ),
                    refused_response_value(
                        Some("incomplete_json_length"),
                        true,
                        true,
                        "google",
                        "cut off 2",
                    ),
                ],
            );
            let generate = OneShotClient::at_path(&stub);
            let mut output = Vec::new();

            let outcome = execute_request(
                json!({
                    "name": "weekly_reflection",
                    "day": "20260308",
                    "today": "20260316",
                })
                .as_object()
                .unwrap()
                .clone(),
                &paths,
                &context,
                &generate,
                &mut output,
            );

            assert!(
                matches!(outcome, RuntimeOutcome::Finished { .. }),
                "{outcome:?}"
            );
            assert_eq!(stub_invocation_count(&stub), 2);

            let events = parse_events(&output);
            let unavail: Vec<_> = events
                .iter()
                .filter(|e| e["event"] == "unavailable_selection")
                .collect();
            assert_eq!(unavail.len(), 1);
            assert_eq!(unavail[0]["trigger"], "refused:incomplete_json_length");

            let json_path = context.journal.join("reflections/weekly/20260308.json");
            let doc: Value =
                serde_json::from_str(&fs::read_to_string(&json_path).unwrap()).unwrap();
            assert_eq!(doc["selection"]["status"], "fallback");
        }

        #[test]
        fn test_full_transport_error() {
            let _guard = pin_stage_now("2026-03-15T18:00:00Z");
            let (root, paths, context) = test_fixture("20260308", &["Alpha memory."], true);

            let stub = sequenced_one_shot_stub(root.path(), &[]);
            let generate = OneShotClient::at_path(&stub);
            let mut output = Vec::new();

            let outcome = execute_request(
                json!({
                    "name": "weekly_reflection",
                    "day": "20260308",
                    "today": "20260316",
                })
                .as_object()
                .unwrap()
                .clone(),
                &paths,
                &context,
                &generate,
                &mut output,
            );

            assert!(
                matches!(outcome, RuntimeOutcome::Finished { .. }),
                "{outcome:?}"
            );
            assert_eq!(stub_invocation_count(&stub), 1);

            let events = parse_events(&output);
            let unavail: Vec<_> = events
                .iter()
                .filter(|e| e["event"] == "unavailable_selection")
                .collect();
            assert_eq!(unavail.len(), 1);
            assert_eq!(unavail[0]["trigger"], "transport");

            let json_path = context.journal.join("reflections/weekly/20260308.json");
            let doc: Value =
                serde_json::from_str(&fs::read_to_string(&json_path).unwrap()).unwrap();
            assert_eq!(doc["selection"]["status"], "fallback");
            assert_eq!(doc["selection"]["model"], "google/gemini");
        }

        #[test]
        fn test_full_clipped_budget_two_candidates() {
            let _guard = pin_stage_now("2026-03-15T18:00:00Z");
            let (root, paths, context) =
                test_fixture("20260308", &["Alpha memory.", "Beta memory."], true);

            let mut resp =
                generated_response_value(r#"{"selections":{"S00":"M001"}}"#, Value::Null);
            resp["input_budget"] = json!({"clipped": true});
            let stub = sequenced_one_shot_stub(root.path(), &[resp]);
            let generate = OneShotClient::at_path(&stub);
            let mut output = Vec::new();

            let outcome = execute_request(
                json!({
                    "name": "weekly_reflection",
                    "day": "20260308",
                    "today": "20260316",
                })
                .as_object()
                .unwrap()
                .clone(),
                &paths,
                &context,
                &generate,
                &mut output,
            );

            assert!(
                matches!(outcome, RuntimeOutcome::Finished { .. }),
                "{outcome:?}"
            );
            assert_eq!(stub_invocation_count(&stub), 1);

            let events = parse_events(&output);
            let unavail: Vec<_> = events
                .iter()
                .filter(|e| e["event"] == "unavailable_selection")
                .collect();
            assert_eq!(unavail.len(), 1);
            assert_eq!(unavail[0]["trigger"], "clipped");

            let md_path = context.journal.join("reflections/weekly/20260308.md");
            let json_path = context.journal.join("reflections/weekly/20260308.json");
            let md = fs::read_to_string(&md_path).unwrap();
            assert!(!md.contains("selections"));
            assert!(md.contains("Alpha memory."));

            let doc: Value =
                serde_json::from_str(&fs::read_to_string(&json_path).unwrap()).unwrap();
            assert_eq!(doc["selection"]["status"], "fallback");
            assert_eq!(doc["memories"][0]["text"], "Alpha memory.");
        }

        #[test]
        fn test_full_valid_unclipped_selection_non_first() {
            let _guard = pin_stage_now("2026-03-15T18:00:00Z");
            let (root, paths, context) =
                test_fixture("20260308", &["Alpha memory.", "Beta memory."], true);

            let resp = generated_response_value(r#"{"selections":{"S00":"M001"}}"#, Value::Null);
            let stub = sequenced_one_shot_stub(root.path(), &[resp]);
            let generate = OneShotClient::at_path(&stub);
            let mut output = Vec::new();

            let outcome = execute_request(
                json!({
                    "name": "weekly_reflection",
                    "day": "20260308",
                    "today": "20260316",
                })
                .as_object()
                .unwrap()
                .clone(),
                &paths,
                &context,
                &generate,
                &mut output,
            );

            assert!(
                matches!(outcome, RuntimeOutcome::Finished { .. }),
                "{outcome:?}"
            );
            assert_eq!(stub_invocation_count(&stub), 1);

            let events = parse_events(&output);
            assert_eq!(
                events
                    .iter()
                    .filter(|e| e["event"] == "unavailable_selection")
                    .count(),
                0
            );
            assert!(
                events
                    .iter()
                    .find(|e| e["event"] == "finish" && e.get("day").is_some())
                    .is_none()
            );

            let json_path = context.journal.join("reflections/weekly/20260308.json");
            let doc: Value =
                serde_json::from_str(&fs::read_to_string(&json_path).unwrap()).unwrap();
            assert_eq!(doc["selection"]["status"], "model");
            assert_eq!(doc["memories"][0]["text"], "Beta memory.");
        }

        #[test]
        fn test_full_parse_failure_null_schema_validation() {
            let _guard = pin_stage_now("2026-03-15T18:00:00Z");
            let (root, paths, context) = test_fixture("20260308", &["Alpha memory."], true);

            let stub = sequenced_one_shot_stub(
                root.path(),
                &[generated_response_value(
                    "not valid json at all",
                    Value::Null,
                )],
            );
            let generate = OneShotClient::at_path(&stub);
            let mut output = Vec::new();

            let outcome = execute_request(
                json!({
                    "name": "weekly_reflection",
                    "day": "20260308",
                    "today": "20260316",
                })
                .as_object()
                .unwrap()
                .clone(),
                &paths,
                &context,
                &generate,
                &mut output,
            );

            assert!(
                matches!(outcome, RuntimeOutcome::StageFailed(_)),
                "{outcome:?}"
            );
            assert!(
                !context
                    .journal
                    .join("reflections/weekly/20260308.md")
                    .exists()
            );
            assert!(
                !context
                    .journal
                    .join("reflections/weekly/20260308.json")
                    .exists()
            );

            let events = parse_events(&output);
            assert_eq!(
                events
                    .iter()
                    .filter(|e| e["event"] == "unavailable_selection")
                    .count(),
                0
            );
        }

        #[test]
        fn test_full_build_failure_future_day_denver_pin() {
            let _guard = pin_stage_now("2026-03-15T18:00:00Z");
            let (root, paths, context) = test_fixture("20990104", &["Alpha memory."], true);

            let stub = sequenced_one_shot_stub(root.path(), &[]);
            let generate = OneShotClient::at_path(&stub);
            let mut output = Vec::new();

            let outcome = execute_request(
                json!({
                    "name": "weekly_reflection",
                    "day": "20990104",
                    "today": "20990112",
                })
                .as_object()
                .unwrap()
                .clone(),
                &paths,
                &context,
                &generate,
                &mut output,
            );

            assert!(
                matches!(outcome, RuntimeOutcome::StageFailed(_)),
                "{outcome:?}"
            );
            assert!(
                !context
                    .journal
                    .join("reflections/weekly/20990104.md")
                    .exists()
            );
            assert!(
                !context
                    .journal
                    .join("reflections/weekly/20990104.json")
                    .exists()
            );

            let events = parse_events(&output);
            assert_eq!(
                events
                    .iter()
                    .filter(|e| e["event"] == "unavailable_selection")
                    .count(),
                0
            );
        }

        #[test]
        fn test_apply_prompt_override_with_none_state_returns_err() {
            let mut prepared = PreparedTalent {
                name: "weekly_reflection".to_owned(),
                config: Map::from_iter([
                    ("max_output_tokens".to_owned(), json!(1024)),
                    (
                        "prompt".to_owned(),
                        Value::String("Some prompt template".to_owned()),
                    ),
                ]),
            };
            let res = apply_prompt_override(&mut prepared, &PrePostState::None);
            assert!(res.is_err());
        }

        #[test]
        fn test_full_template_variables_preserved_in_request() {
            let _guard = pin_stage_now("2026-03-15T18:00:00Z");
            let (root, paths, context) =
                test_fixture("20260308", &["Memory with $name and $$ dollars."], true);

            let resp = generated_response_value(r#"{"selections":{"S00":"M000"}}"#, Value::Null);
            let stub = sequenced_one_shot_stub(root.path(), &[resp]);
            let generate = OneShotClient::at_path(&stub);
            let mut output = Vec::new();

            let outcome = execute_request(
                json!({
                    "name": "weekly_reflection",
                    "day": "20260308",
                    "today": "20260316",
                })
                .as_object()
                .unwrap()
                .clone(),
                &paths,
                &context,
                &generate,
                &mut output,
            );

            assert!(
                matches!(outcome, RuntimeOutcome::Finished { .. }),
                "{outcome:?}"
            );
            let req_path = PathBuf::from(format!("{}.request_1", stub.display()));
            let req_content = fs::read_to_string(req_path).unwrap();
            assert!(req_content.contains("Memory with $name and $$ dollars."));
        }

        #[test]
        fn test_full_weekly_reflection_ignores_forged_request_keys() {
            let _guard = pin_stage_now("2026-03-15T18:00:00Z");
            let forged_cases = [
                json!({
                    "name": "weekly_reflection",
                    "day": "20260308",
                    "today": "20260316",
                    "unavailable_selection": "zero_slots",
                }),
                json!({
                    "name": "weekly_reflection",
                    "day": "20260308",
                    "today": "20260316",
                    "unavailable_selection": "schema_exhausted",
                }),
                json!({
                    "name": "weekly_reflection",
                    "day": "20260308",
                    "today": "20260316",
                    "input_budget": {"clipped": true},
                }),
            ];
            for req in forged_cases {
                let (root, paths, context) = test_fixture("20260308", &["Alpha memory."], true);
                let resp =
                    generated_response_value(r#"{"selections":{"S00":"M000"}}"#, Value::Null);
                let stub = sequenced_one_shot_stub(root.path(), &[resp]);
                let generate = OneShotClient::at_path(&stub);
                let mut output = Vec::new();

                let outcome = execute_request(
                    req.as_object().unwrap().clone(),
                    &paths,
                    &context,
                    &generate,
                    &mut output,
                );

                assert!(
                    matches!(outcome, RuntimeOutcome::Finished { .. }),
                    "{outcome:?}"
                );
                assert_eq!(stub_invocation_count(&stub), 1);

                let json_path = context.journal.join("reflections/weekly/20260308.json");
                let doc: Value =
                    serde_json::from_str(&fs::read_to_string(&json_path).unwrap()).unwrap();
                assert_eq!(doc["selection"]["status"], "model");

                let events = parse_events(&output);
                assert_eq!(
                    events
                        .iter()
                        .filter(|e| e["event"] == "unavailable_selection")
                        .count(),
                    0
                );
            }
        }

        #[test]
        fn test_full_zero_slots_with_forged_unavailable_key() {
            let _guard = pin_stage_now("2026-03-15T18:00:00Z");
            let (root, paths, context) = test_fixture("20260308", &[], false);

            let stub = sequenced_one_shot_stub(root.path(), &[]);
            let generate = OneShotClient::at_path(&stub);
            let mut output = Vec::new();

            let outcome = execute_request(
                json!({
                    "name": "weekly_reflection",
                    "day": "20260308",
                    "today": "20260316",
                    "unavailable_selection": "zero_slots",
                })
                .as_object()
                .unwrap()
                .clone(),
                &paths,
                &context,
                &generate,
                &mut output,
            );

            assert!(
                matches!(outcome, RuntimeOutcome::Finished { .. }),
                "{outcome:?}"
            );
            assert_eq!(stub_invocation_count(&stub), 0);

            let events = parse_events(&output);
            assert_eq!(
                events
                    .iter()
                    .filter(|e| e["event"] == "unavailable_selection")
                    .count(),
                0
            );

            let md_path = context.journal.join("reflections/weekly/20260308.md");
            let json_path = context.journal.join("reflections/weekly/20260308.json");
            assert_eq!(
                fs::read_to_string(&md_path).unwrap(),
                "nothing from this week is on this page.\n"
            );
            let doc: Value =
                serde_json::from_str(&fs::read_to_string(&json_path).unwrap()).unwrap();
            assert_eq!(doc["selection"]["status"], "none");
            assert!(doc["selection"]["model"].is_null());
            assert!(doc["intro"].as_array().unwrap().is_empty());
            assert!(doc["memories"].as_array().unwrap().is_empty());
            assert_eq!(doc["days"].as_array().unwrap().len(), 7);
        }

        #[test]
        fn test_full_weekly_reflection_candidate_id_mapping() {
            let _guard = pin_stage_now("2026-03-15T18:00:00Z");
            let longer_bullet = "A productive planning session with the team discussing architecture and quarterly deliverables.";
            let (root, paths, context) =
                test_fixture("20260308", &["", longer_bullet, "Meet Sam."], true);

            let resp = generated_response_value(r#"{"selections":{"S00":"M000"}}"#, Value::Null);
            let stub = sequenced_one_shot_stub(root.path(), &[resp]);
            let generate = OneShotClient::at_path(&stub);
            let mut output = Vec::new();

            let outcome = execute_request(
                json!({
                    "name": "weekly_reflection",
                    "day": "20260308",
                    "today": "20260316",
                })
                .as_object()
                .unwrap()
                .clone(),
                &paths,
                &context,
                &generate,
                &mut output,
            );

            assert!(
                matches!(outcome, RuntimeOutcome::Finished { .. }),
                "{outcome:?}"
            );

            let req_path = PathBuf::from(format!("{}.request_1", stub.display()));
            let req_raw = fs::read_to_string(req_path).unwrap();
            let req_val: Value = serde_json::from_str(&req_raw).unwrap();
            let transcript_str = req_val["contents"][0]["text"]
                .as_str()
                .or_else(|| req_val["transcript"].as_str())
                .unwrap();
            let transcript_val: Value = serde_json::from_str(transcript_str).unwrap();

            assert_eq!(transcript_val["memories"][0]["id"], "M000");
            assert_eq!(transcript_val["memories"][0]["text"], "Meet Sam.");
            assert_eq!(transcript_val["memories"][1]["id"], "M001");
            assert_eq!(transcript_val["memories"][1]["text"], longer_bullet);

            let json_path = context.journal.join("reflections/weekly/20260308.json");
            let doc: Value =
                serde_json::from_str(&fs::read_to_string(&json_path).unwrap()).unwrap();
            assert_eq!(doc["memories"][0]["id"], "m-20260308-2");
            assert_eq!(doc["memories"][0]["text"], "Meet Sam.");

            let day_entry = doc["days"]
                .as_array()
                .unwrap()
                .iter()
                .find(|d| d["day"] == "20260308")
                .unwrap();
            assert_eq!(day_entry["memory_id"], "m-20260308-2");
        }

        #[test]
        fn test_full_weekly_reflection_placeholder_only_slot() {
            let _guard = pin_stage_now("2026-03-15T18:00:00Z");
            let placeholder_bullet = "speaker 1 arrived early";
            let (root, paths, context) = test_fixture("20260308", &[placeholder_bullet], true);

            let resp = generated_response_value(r#"{"selections":{"S00":"M000"}}"#, Value::Null);
            let stub = sequenced_one_shot_stub(root.path(), &[resp]);
            let generate = OneShotClient::at_path(&stub);
            let mut output = Vec::new();

            let outcome = execute_request(
                json!({
                    "name": "weekly_reflection",
                    "day": "20260308",
                    "today": "20260316",
                })
                .as_object()
                .unwrap()
                .clone(),
                &paths,
                &context,
                &generate,
                &mut output,
            );

            assert!(
                matches!(outcome, RuntimeOutcome::Finished { .. }),
                "{outcome:?}"
            );

            let req_path = PathBuf::from(format!("{}.request_1", stub.display()));
            let req_raw = fs::read_to_string(req_path).unwrap();
            let req_val: Value = serde_json::from_str(&req_raw).unwrap();
            let transcript_str = req_val["contents"][0]["text"]
                .as_str()
                .or_else(|| req_val["transcript"].as_str())
                .unwrap();
            let transcript_val: Value = serde_json::from_str(transcript_str).unwrap();

            assert_eq!(transcript_val["slots"][0]["placeholder_only"], true);
            assert_eq!(transcript_val["memories"][0]["text"], placeholder_bullet);

            let md_path = context.journal.join("reflections/weekly/20260308.md");
            let json_path = context.journal.join("reflections/weekly/20260308.json");
            assert!(
                fs::read_to_string(&md_path)
                    .unwrap()
                    .contains(placeholder_bullet)
            );

            let doc: Value =
                serde_json::from_str(&fs::read_to_string(&json_path).unwrap()).unwrap();
            assert_eq!(doc["selection"]["status"], "model");
        }

        #[test]
        fn test_full_weekly_reflection_index_success() {
            let _guard = pin_stage_now("2026-03-15T18:00:00Z");
            crate::writers::reset_index_warning_count();
            let (root, paths, context) = test_fixture("20260308", &["Alpha memory."], true);

            let resp = generated_response_value(r#"{"selections":{"S00":"M000"}}"#, Value::Null);
            let stub = sequenced_one_shot_stub(root.path(), &[resp]);
            let generate = OneShotClient::at_path(&stub);
            let mut output = Vec::new();

            let outcome = execute_request(
                json!({
                    "name": "weekly_reflection",
                    "day": "20260308",
                    "today": "20260316",
                })
                .as_object()
                .unwrap()
                .clone(),
                &paths,
                &context,
                &generate,
                &mut output,
            );

            assert!(
                matches!(outcome, RuntimeOutcome::Finished { .. }),
                "{outcome:?}"
            );

            let conn = solstone_core_indexer_store::db::open_index(&context.journal).unwrap();
            let mut stmt = conn
                .prepare("SELECT DISTINCT agent, day FROM chunks WHERE path = 'reflections/weekly/20260308.md'")
                .unwrap();
            let rows: Vec<(String, String)> = stmt
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
                .unwrap()
                .map(Result::unwrap)
                .collect();

            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].0, "reflection");
            assert_eq!(rows[0].1, "20260308");
            assert_eq!(crate::writers::index_warning_count(), 0);
        }

        #[test]
        fn test_full_weekly_reflection_index_error_retains_output_and_warns() {
            let _guard = pin_stage_now("2026-03-15T18:00:00Z");
            crate::writers::reset_index_warning_count();
            let (root, paths, context) = test_fixture("20260308", &["Alpha memory."], true);

            // Block directory creation by making indexer a regular file
            fs::write(context.journal.join("indexer"), b"not a directory").unwrap();

            let resp = generated_response_value(r#"{"selections":{"S00":"M000"}}"#, Value::Null);
            let stub = sequenced_one_shot_stub(root.path(), &[resp]);
            let generate = OneShotClient::at_path(&stub);
            let mut output = Vec::new();

            let outcome = execute_request(
                json!({
                    "name": "weekly_reflection",
                    "day": "20260308",
                    "today": "20260316",
                })
                .as_object()
                .unwrap()
                .clone(),
                &paths,
                &context,
                &generate,
                &mut output,
            );

            assert!(
                matches!(outcome, RuntimeOutcome::Finished { .. }),
                "{outcome:?}"
            );

            let md_path = context.journal.join("reflections/weekly/20260308.md");
            let json_path = context.journal.join("reflections/weekly/20260308.json");
            assert!(md_path.exists());
            assert!(json_path.exists());
            assert!(crate::writers::index_warning_count() >= 1);
        }
    }
}
