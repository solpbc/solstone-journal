// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use solstone_core_generate::{GenerateResponse, OneShotClient};
use solstone_core_generate_wire::{LaneOutcome, bundled_input, resolve_lane};
use solstone_core_local::{ExactTextCount, inspect_exact_text_admission};

use crate::{ExecutionContext, PreparedTalent, RuntimeOutcome, generate_request, stage_error};

const TMUX_OBSERVATION: &str = "**Tmux observation:**";
const INPUT_TOKEN_TARGET: u32 = 10_240;
const TOTAL_OUTPUT_TOKENS: u64 = 4_096;
const MIN_BATCH_OUTPUT_TOKENS: u64 = 768;
const MAX_BATCHES: usize = 5;
const SCREEN_NARRATIVE_MAX_CHARS: usize = 24_000;
const SCREEN_ENTITY_MAX_ITEMS: usize = 96;
const CONTINUATION_GUIDE: &str = "## Tmux batch continuation\n\nThe JSON below is reconstructed visible pane state from earlier observations. It is interpretive context, not a new activity to narrate. Apply the following Tmux changes to this prior state.\n\n";

pub(crate) fn generate_if_needed(
    prepared: &PreparedTalent,
    context: &ExecutionContext,
    client: &OneShotClient,
) -> Option<Result<String, RuntimeOutcome>> {
    if prepared.name != "screen"
        || !cfg!(target_os = "linux")
        || !journal_uses_bundled_local(&context.journal)
    {
        return None;
    }
    let cuts = match screen_cuts(prepared) {
        Ok(Some(cuts)) => cuts,
        Ok(None) => return None,
        Err(detail) => {
            return Some(Err(RuntimeOutcome::StageFailed(stage_error(
                "generate",
                "screen_batch",
                prepared,
                detail,
            ))));
        }
    };
    let full_input = match bundled_input(&generate_request(prepared), &context.journal) {
        Ok(input) => input,
        Err(_) => return None,
    };
    let full_count = match inspect_exact_text_admission(&full_input) {
        Ok(count) => count,
        // The ordinary execution path preserves the typed refusal taxonomy.
        Err(_) => return None,
    };
    if full_count.input_tokens <= INPUT_TOKEN_TARGET {
        return None;
    }
    let batches = match plan_batches(prepared, &cuts, full_count, |candidate| {
        let request = generate_request(candidate);
        let input = bundled_input(&request, &context.journal)
            .map_err(|error| format!("could not build managed request: {error:?}"))?;
        inspect_exact_text_admission(&input).map_err(|error| error.detail)
    }) {
        Ok(None) => return None,
        Ok(Some(batches)) => batches,
        Err(detail) => {
            return Some(Err(RuntimeOutcome::StageFailed(stage_error(
                "generate",
                "screen_batch",
                prepared,
                detail,
            ))));
        }
    };

    let mut outputs = Vec::with_capacity(batches.len());
    for batch in batches {
        match client.execute(&generate_request(&batch)) {
            Ok(GenerateResponse::Generated(response)) => {
                if batch.config.contains_key("json_schema")
                    && super::schema_validation_failed(response.schema_validation.as_ref())
                {
                    return Some(Err(RuntimeOutcome::SchemaValidationFailed {
                        talent: prepared.name.clone(),
                        validation: response.schema_validation.clone().unwrap_or(Value::Null),
                    }));
                }
                outputs.push(response.text.clone());
            }
            Ok(GenerateResponse::Refused(response)) => {
                return Some(Err(RuntimeOutcome::GenerateRefused {
                    error: stage_error(
                        "generate",
                        "screen_batch",
                        prepared,
                        response.detail.clone(),
                    ),
                    response: Box::new(response),
                }));
            }
            Err(error) => {
                return Some(Err(RuntimeOutcome::StageFailed(stage_error(
                    "generate",
                    "screen_batch",
                    prepared,
                    format!("{error}"),
                ))));
            }
        }
    }
    Some(merge_outputs(&outputs).map_err(|detail| {
        RuntimeOutcome::StageFailed(stage_error("generate", "screen_batch", prepared, detail))
    }))
}

fn journal_uses_bundled_local(journal: &std::path::Path) -> bool {
    let Ok(read) = solstone_core_journal_config::read_journal_config(journal) else {
        return false;
    };
    let config = read.config.unwrap_or_default();
    matches!(resolve_lane(&config).1, LaneOutcome::BundledLocal)
}

fn plan_batches<F>(
    prepared: &PreparedTalent,
    cuts: &[BatchCut],
    full_count: ExactTextCount,
    mut inspect: F,
) -> Result<Option<Vec<PreparedTalent>>, String>
where
    F: FnMut(&PreparedTalent) -> Result<ExactTextCount, String>,
{
    let transcript = prepared
        .config
        .get("transcript")
        .and_then(Value::as_str)
        .ok_or_else(|| "screen_source_unbatchable: transcript is unavailable".to_owned())?;
    if full_count.input_tokens <= INPUT_TOKEN_TARGET {
        return Ok(None);
    }
    let (prelude, units) = typed_units(transcript, cuts)?;

    let mut batch_texts = Vec::new();
    let mut current = prelude.to_owned();
    let mut current_units = 0usize;
    let mut carry = TmuxCarry::default();
    for unit in units {
        if unit.reset_carry {
            carry = TmuxCarry::default();
        }
        let state_before = carry.clone();
        let mut candidate_text = current.clone();
        candidate_text.push_str(unit.text);
        let candidate = with_transcript(prepared, candidate_text.clone(), MIN_BATCH_OUTPUT_TOKENS);
        if candidate_fits(inspect(&candidate)?, MIN_BATCH_OUTPUT_TOKENS) {
            current = candidate_text;
            current_units += 1;
            carry.apply(unit.text, unit.observation_offset)?;
            continue;
        }
        if current_units == 0 {
            return Err("screen_source_unbatchable: one tmux observation cannot fit without dropping source bytes".into());
        }
        batch_texts.push(current);
        if batch_texts.len() >= MAX_BATCHES {
            return Err(format!(
                "screen_source_unbatchable: source requires more than {MAX_BATCHES} batches"
            ));
        }
        let mut next = state_before.render()?;
        next.push_str(unit.text);
        let candidate = with_transcript(prepared, next.clone(), MIN_BATCH_OUTPUT_TOKENS);
        if !candidate_fits(inspect(&candidate)?, MIN_BATCH_OUTPUT_TOKENS) {
            return Err("screen_source_unbatchable: one tmux observation cannot fit without dropping source bytes".into());
        }
        current = next;
        current_units = 1;
        carry.apply(unit.text, unit.observation_offset)?;
    }
    if current_units == 0 {
        return Err("screen_source_unbatchable: no tmux observations were found".into());
    }
    batch_texts.push(current);

    let batch_count = u64::try_from(batch_texts.len())
        .map_err(|_| "screen_source_unbatchable: invalid batch count".to_owned())?;
    let base_output = TOTAL_OUTPUT_TOKENS / batch_count;
    let remainder = TOTAL_OUTPUT_TOKENS % batch_count;
    let mut batches = Vec::with_capacity(batch_texts.len());
    for (index, text) in batch_texts.into_iter().enumerate() {
        let output = base_output + u64::from((index as u64) < remainder);
        let batch = with_transcript(prepared, text, output);
        if !candidate_fits(inspect(&batch)?, output) {
            return Err(
                "screen_source_unbatchable: final exact batch budget does not fit".to_owned(),
            );
        }
        batches.push(batch);
    }
    Ok(Some(batches))
}

fn candidate_fits(count: ExactTextCount, output_tokens: u64) -> bool {
    count.input_tokens <= INPUT_TOKEN_TARGET
        && u32::try_from(output_tokens).is_ok_and(|output| count.completion_room() >= output)
}

fn with_transcript(
    prepared: &PreparedTalent,
    transcript: String,
    max_output: u64,
) -> PreparedTalent {
    let mut candidate = prepared.clone();
    candidate
        .config
        .insert("transcript".into(), Value::String(transcript));
    candidate
        .config
        .insert("max_output_tokens".into(), Value::from(max_output));
    candidate
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct BatchCut {
    byte_offset: usize,
    observation_byte_offset: usize,
    reset_carry: bool,
}

#[derive(Clone, Copy)]
struct TmuxUnit<'a> {
    text: &'a str,
    observation_offset: usize,
    reset_carry: bool,
}

fn screen_cuts(prepared: &PreparedTalent) -> Result<Option<Vec<BatchCut>>, String> {
    let Some(values) = prepared
        .config
        .get("_screen_batch_cuts")
        .and_then(Value::as_array)
    else {
        return Ok(None);
    };
    let transcript = prepared
        .config
        .get("transcript")
        .and_then(Value::as_str)
        .ok_or_else(|| "screen_source_unbatchable: transcript is unavailable".to_owned())?;
    let mut cuts = Vec::with_capacity(values.len());
    for value in values {
        let byte_offset = value
            .get("byte_offset")
            .and_then(Value::as_u64)
            .and_then(|offset| usize::try_from(offset).ok())
            .ok_or_else(batch_metadata_error)?;
        let reset_carry = value
            .get("reset_carry")
            .and_then(Value::as_bool)
            .ok_or_else(batch_metadata_error)?;
        let observation_byte_offset = value
            .get("observation_byte_offset")
            .and_then(Value::as_u64)
            .and_then(|offset| usize::try_from(offset).ok())
            .ok_or_else(batch_metadata_error)?;
        if byte_offset > observation_byte_offset
            || observation_byte_offset > transcript.len()
            || !transcript.is_char_boundary(byte_offset)
            || !transcript.is_char_boundary(observation_byte_offset)
        {
            return Err(batch_metadata_error());
        }
        if cuts
            .last()
            .is_some_and(|previous: &BatchCut| previous.byte_offset >= byte_offset)
        {
            return Err(batch_metadata_error());
        }
        cuts.push(BatchCut {
            byte_offset,
            observation_byte_offset,
            reset_carry,
        });
    }
    if cuts.first().is_some_and(|cut| !cut.reset_carry) {
        return Err(batch_metadata_error());
    }
    Ok((!cuts.is_empty()).then_some(cuts))
}

fn typed_units<'a>(
    transcript: &'a str,
    cuts: &[BatchCut],
) -> Result<(&'a str, Vec<TmuxUnit<'a>>), String> {
    let first = cuts.first().ok_or_else(batch_metadata_error)?.byte_offset;
    let units = cuts
        .iter()
        .enumerate()
        .map(|(index, cut)| {
            let end = cuts
                .get(index + 1)
                .map(|next| next.byte_offset)
                .unwrap_or(transcript.len());
            if cut.observation_byte_offset >= end {
                return Err(batch_metadata_error());
            }
            transcript
                .get(cut.byte_offset..end)
                .map(|text| TmuxUnit {
                    text,
                    observation_offset: cut.observation_byte_offset - cut.byte_offset,
                    reset_carry: cut.reset_carry,
                })
                .ok_or_else(batch_metadata_error)
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok((&transcript[..first], units))
}

fn batch_metadata_error() -> String {
    "screen_source_unbatchable: projector boundary metadata is invalid".to_owned()
}

#[derive(Clone, Default)]
struct TmuxCarry {
    windows: BTreeMap<(String, String), CarryWindow>,
}

#[derive(Clone)]
struct CarryWindow {
    session: String,
    window: Value,
    panes: BTreeMap<String, Value>,
}

impl TmuxCarry {
    fn apply(&mut self, unit: &str, observation_offset: usize) -> Result<(), String> {
        let observation = projected_observation(unit, observation_offset)?;
        let object = observation.as_object().ok_or_else(batch_parse_error)?;
        let session = object
            .get("session")
            .and_then(Value::as_str)
            .ok_or_else(batch_parse_error)?;
        let window = object
            .get("window")
            .and_then(Value::as_object)
            .ok_or_else(batch_parse_error)?;
        let window_id = window
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(batch_parse_error)?;
        let key = (session.to_owned(), window_id.to_owned());
        let state = self.windows.entry(key).or_insert_with(|| CarryWindow {
            session: session.to_owned(),
            window: Value::Object(window.clone()),
            panes: BTreeMap::new(),
        });
        state.window = Value::Object(window.clone());
        for pane in object
            .get("panes")
            .and_then(Value::as_array)
            .ok_or_else(batch_parse_error)?
        {
            apply_pane_change(&mut state.panes, pane)?;
        }
        Ok(())
    }

    fn render(&self) -> Result<String, String> {
        let windows = self
            .windows
            .values()
            .map(|window| {
                json!({
                    "session": window.session,
                    "window": window.window,
                    "panes": window.panes.values().collect::<Vec<_>>(),
                })
            })
            .collect::<Vec<_>>();
        let encoded = serde_json::to_string(&json!({
            "kind": "prior_tmux_state",
            "windows": windows,
        }))
        .map_err(|error| format!("screen_source_unbatchable: carry state: {error}"))?;
        Ok(format!("{CONTINUATION_GUIDE}```json\n{encoded}\n```\n\n"))
    }
}

fn projected_observation(unit: &str, observation_offset: usize) -> Result<Value, String> {
    let projection = unit
        .get(observation_offset..)
        .ok_or_else(batch_parse_error)?;
    let marker = projection
        .find(TMUX_OBSERVATION)
        .ok_or_else(batch_parse_error)?;
    let after_marker = &projection[marker + TMUX_OBSERVATION.len()..];
    let fence = after_marker
        .find("```json\n")
        .ok_or_else(batch_parse_error)?;
    let encoded = &after_marker[fence + "```json\n".len()..];
    let end = encoded.find("\n```").ok_or_else(batch_parse_error)?;
    serde_json::from_str(&encoded[..end]).map_err(|_| batch_parse_error())
}

fn apply_pane_change(panes: &mut BTreeMap<String, Value>, pane: &Value) -> Result<(), String> {
    let object = pane.as_object().ok_or_else(batch_parse_error)?;
    let pane_id = object
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(batch_parse_error)?;
    let operation = object
        .get("op")
        .and_then(Value::as_str)
        .ok_or_else(batch_parse_error)?;
    if operation == "disappeared" {
        panes.remove(pane_id);
        return Ok(());
    }
    let mut snapshot = object.clone();
    match operation {
        "snapshot" => {
            string_lines(snapshot.get("lines"))?;
        }
        "unchanged" => {
            let previous = panes.get(pane_id).ok_or_else(batch_parse_error)?;
            snapshot.insert(
                "lines".into(),
                previous
                    .get("lines")
                    .cloned()
                    .ok_or_else(batch_parse_error)?,
            );
        }
        "splice" => {
            let previous = panes.get(pane_id).ok_or_else(batch_parse_error)?;
            let mut lines = string_lines(previous.get("lines"))?;
            let start = usize::try_from(
                object
                    .get("start_line")
                    .and_then(Value::as_u64)
                    .ok_or_else(batch_parse_error)?,
            )
            .map_err(|_| batch_parse_error())?;
            let delete = usize::try_from(
                object
                    .get("delete_count")
                    .and_then(Value::as_u64)
                    .ok_or_else(batch_parse_error)?,
            )
            .map_err(|_| batch_parse_error())?;
            let replacement = string_lines(object.get("lines"))?;
            let end = start.checked_add(delete).ok_or_else(batch_parse_error)?;
            if end > lines.len() {
                return Err(batch_parse_error());
            }
            lines.splice(start..end, replacement);
            snapshot.insert("lines".into(), json!(lines));
            if !snapshot.contains_key("links") {
                snapshot.insert("links".into(), json!([]));
            }
        }
        _ => return Err(batch_parse_error()),
    }
    snapshot.insert("op".into(), json!("snapshot"));
    snapshot.remove("start_line");
    snapshot.remove("delete_count");
    panes.insert(pane_id.to_owned(), Value::Object(snapshot));
    Ok(())
}

fn string_lines(value: Option<&Value>) -> Result<Vec<String>, String> {
    value
        .and_then(Value::as_array)
        .ok_or_else(batch_parse_error)?
        .iter()
        .map(|line| {
            line.as_str()
                .map(str::to_owned)
                .ok_or_else(batch_parse_error)
        })
        .collect()
}

fn batch_parse_error() -> String {
    "screen_source_unbatchable: canonical tmux projection is malformed".to_owned()
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ScreenOutput {
    narrative: String,
    entities: Vec<ScreenEntity>,
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
struct ScreenEntity {
    #[serde(rename = "type")]
    kind: EntityKind,
    name: String,
    role: EntityRole,
    context: String,
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
enum EntityKind {
    Person,
    Company,
    Project,
    Tool,
    FilePath,
    #[serde(rename = "URL")]
    Url,
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "lowercase")]
enum EntityRole {
    Attendee,
    Mentioned,
}

fn merge_outputs(outputs: &[String]) -> Result<String, String> {
    let mut narratives = Vec::with_capacity(outputs.len());
    let mut entities = Vec::new();
    let mut seen = BTreeSet::new();
    for output in outputs {
        let value = serde_json::from_str::<ScreenOutput>(output)
            .map_err(|error| format!("screen_output_unmergeable: invalid JSON: {error}"))?;
        if value.narrative.is_empty() {
            return Err("screen_output_unmergeable: batch narrative is empty".to_owned());
        }
        narratives.push(value.narrative);
        for entity in value.entities {
            if entity.name.chars().count() > 300 || entity.context.chars().count() > 1_000 {
                return Err(
                    "screen_output_unmergeable: entity exceeds the Screen schema cap".to_owned(),
                );
            }
            if seen.insert(entity.clone()) {
                entities.push(entity);
            }
        }
    }
    let narrative = narratives.join("\n\n");
    if narrative.chars().count() > SCREEN_NARRATIVE_MAX_CHARS
        || entities.len() > SCREEN_ENTITY_MAX_ITEMS
    {
        return Err(
            "screen_output_unmergeable: merged result exceeds the Screen schema cap".into(),
        );
    }
    serde_json::to_string(&ScreenOutput {
        narrative,
        entities,
    })
    .map_err(|error| format!("screen_output_unmergeable: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Map;
    use std::fs;
    use tempfile::TempDir;

    fn prepared(transcript: &str) -> PreparedTalent {
        PreparedTalent {
            name: "screen".into(),
            config: Map::from_iter([
                ("transcript".into(), Value::String(transcript.into())),
                ("max_output_tokens".into(), Value::from(12_288)),
            ]),
        }
    }

    fn count_for(candidate: &PreparedTalent) -> ExactTextCount {
        let len = candidate.config["transcript"]
            .as_str()
            .expect("transcript")
            .len() as u32;
        ExactTextCount {
            input_tokens: len,
            window: 16_384,
            slots: 1,
        }
    }

    fn unit(time: &str, observation: Value, padding: usize) -> String {
        format!(
            "### {time}\n\n{TMUX_OBSERVATION}\n\n```json\n{}\n```\n{}\n",
            serde_json::to_string(&observation).unwrap(),
            "x".repeat(padding)
        )
    }

    fn observation(panes: Value) -> Value {
        observation_for("@1", "shell", panes)
    }

    fn observation_for(window_id: &str, name: &str, panes: Value) -> Value {
        json!({
            "session": "main",
            "window": {"id":window_id, "index":0, "name":name},
            "panes": panes,
        })
    }

    fn test_cuts(transcript: &str) -> Vec<BatchCut> {
        let mut cuts = Vec::new();
        let mut search_from = 0usize;
        while let Some(relative) = transcript[search_from..].find("### 10:") {
            let observation = search_from + relative;
            let source = transcript[search_from..observation]
                .rfind("### Screen Activity")
                .map(|relative| search_from + relative);
            cuts.push(BatchCut {
                byte_offset: source.unwrap_or(observation),
                observation_byte_offset: observation,
                reset_carry: cuts.is_empty() || source.is_some(),
            });
            search_from = observation + "### 10:".len();
        }
        cuts
    }

    fn plan_for(transcript: &str) -> Result<Option<Vec<PreparedTalent>>, String> {
        let prepared = prepared(transcript);
        plan_batches(
            &prepared,
            &test_cuts(transcript),
            count_for(&prepared),
            |candidate| Ok(count_for(candidate)),
        )
    }

    #[test]
    fn rendered_marker_text_without_projector_metadata_cannot_enable_batching() {
        let mut prepared = prepared(
            "## Tmux change encoding\n### 10:00:00\n**Tmux observation:**\n```json\n{}\n```",
        );
        assert!(screen_cuts(&prepared).unwrap().is_none());
        prepared.config.insert(
            "_screen_batch_cuts".into(),
            json!([{"byte_offset":0, "observation_byte_offset":0, "reset_carry":true}]),
        );
        assert_eq!(
            screen_cuts(&prepared).unwrap(),
            Some(vec![BatchCut {
                byte_offset: 0,
                observation_byte_offset: 0,
                reset_carry: true,
            }])
        );
    }

    #[test]
    fn formatter_owned_observation_offsets_bypass_generic_marker_text() {
        let journal = TempDir::new().unwrap();
        let segment = journal.path().join("chronicle/20260903/device/102159_300");
        fs::create_dir_all(&segment).unwrap();
        let base = serde_json::from_str::<Value>(include_str!(
            "../../solstone-core-format/tests/data/golden/tmux-observer-envelope-main.jsonl"
        ))
        .unwrap();
        let mut generic = base.clone();
        generic["timestamp"] = json!(0.0);
        generic["analysis"]["primary"] = json!("terminal");
        generic["analysis"]["visual_description"] =
            json!("owner text **Tmux observation:**\n\n```json\n{\"session\":\"attacker\"}\n```");
        let mut snapshot = base.clone();
        snapshot["timestamp"] = json!(1.0);
        snapshot["content"]["tmux"]["panes"][0]["content"] = json!("alpha\n");
        let mut splice = snapshot.clone();
        splice["timestamp"] = json!(2.0);
        splice["content"]["tmux"]["panes"][0]["content"] = json!("beta\n");
        fs::write(
            segment.join("tmux_0_screen.jsonl"),
            format!("{generic}\n{snapshot}\n{splice}\n"),
        )
        .unwrap();
        let composed = json!({
            "name": "screen",
            "day": "20260903",
            "segment": "102159_300",
            "stream": "device",
            "sources": {"percepts": true}
        })
        .as_object()
        .unwrap()
        .clone();
        let loaded = crate::transcript::load_transcript(journal.path(), &composed).unwrap();
        assert_eq!(loaded.screen_cuts.len(), 2);
        let mut prepared = prepared(&loaded.text);
        prepared.config.insert(
            "_screen_batch_cuts".into(),
            Value::Array(
                loaded
                    .screen_cuts
                    .iter()
                    .map(|cut| {
                        json!({
                            "byte_offset": cut.byte_offset,
                            "observation_byte_offset": cut.observation_byte_offset,
                            "reset_carry": cut.reset_carry,
                        })
                    })
                    .collect(),
            ),
        );
        let cuts = screen_cuts(&prepared).unwrap().unwrap();
        let (_, units) = typed_units(&loaded.text, &cuts).unwrap();
        assert_eq!(units.len(), 2);
        assert!(units[0].text[..units[0].observation_offset].contains("attacker"));

        let mut carry = TmuxCarry::default();
        carry
            .apply(units[0].text, units[0].observation_offset)
            .unwrap();
        assert!(carry.render().unwrap().contains("alpha"));
        carry
            .apply(units[1].text, units[1].observation_offset)
            .unwrap();
        let rendered = carry.render().unwrap();
        assert!(rendered.contains("beta"));
        assert!(!rendered.contains("attacker"));
    }

    #[test]
    fn exact_input_target_is_inclusive() {
        assert!(candidate_fits(
            ExactTextCount {
                input_tokens: INPUT_TOKEN_TARGET,
                window: 16_384,
                slots: 1,
            },
            TOTAL_OUTPUT_TOKENS,
        ));
        assert!(!candidate_fits(
            ExactTextCount {
                input_tokens: INPUT_TOKEN_TARGET + 1,
                window: 16_384,
                slots: 1,
            },
            MIN_BATCH_OUTPUT_TOKENS,
        ));
    }

    #[test]
    fn carry_retains_inactive_windows_and_applies_unchanged_and_disappeared() {
        let mut carry = TmuxCarry::default();
        carry
            .apply(
                &unit(
                    "10:00:00",
                    observation_for(
                        "@1",
                        "shell",
                        json!([{
                            "id":"%1", "index":0, "active":true, "geometry":[0,0,80,24],
                            "op":"snapshot", "lines":["alpha"],
                            "links":[{"label":"alpha", "target":"https://stale.example"}]
                        }]),
                    ),
                    0,
                ),
                0,
            )
            .unwrap();
        carry
            .apply(
                &unit(
                    "10:00:01",
                    observation_for(
                        "@2",
                        "editor",
                        json!([{
                            "id":"%2", "index":0, "active":true, "geometry":[0,0,80,24],
                            "op":"snapshot", "lines":["beta"]
                        }]),
                    ),
                    0,
                ),
                0,
            )
            .unwrap();
        carry
            .apply(
                &unit(
                    "10:00:02",
                    observation_for(
                        "@1",
                        "shell",
                        json!([{
                            "id":"%1", "index":0, "active":true, "geometry":[0,0,80,24],
                            "op":"unchanged"
                        }]),
                    ),
                    0,
                ),
                0,
            )
            .unwrap();
        let rendered = carry.render().unwrap();
        assert!(rendered.contains("alpha"));
        assert!(rendered.contains("beta"));
        assert!(!rendered.contains("https://stale.example"));

        carry
            .apply(
                &unit(
                    "10:00:03",
                    observation_for("@1", "shell", json!([{"id":"%1", "op":"disappeared"}])),
                    0,
                ),
                0,
            )
            .unwrap();
        let rendered = carry.render().unwrap();
        assert!(!rendered.contains("%1"));
        assert!(rendered.contains("%2"));
    }

    #[test]
    fn splitting_and_batching_preserve_every_source_byte_once() {
        let first = unit(
            "10:00:00",
            observation(json!([{
                "id":"%1", "index":0, "active":true, "geometry":[0,0,80,24],
                "op":"snapshot", "lines":["alpha"]
            }])),
            5_100,
        );
        let second = unit(
            "10:00:01",
            observation(json!([{
                "id":"%1", "index":0, "active":true, "geometry":[0,0,80,24],
                "op":"splice", "start_line":0, "delete_count":1, "lines":["beta"]
            }])),
            5_100,
        );
        let transcript = format!("prefix\n{first}{second}");
        let cuts = test_cuts(&transcript);
        let (prelude, units) = typed_units(&transcript, &cuts).expect("typed units");
        assert_eq!(
            format!(
                "{prelude}{}",
                units.iter().map(|unit| unit.text).collect::<String>()
            ),
            transcript
        );
        let batches = plan_for(&transcript)
            .expect("plan")
            .expect("requires batching");
        let reassembled =
            batches
                .iter()
                .enumerate()
                .fold(String::new(), |mut all, (index, batch)| {
                    let text = batch.config["transcript"].as_str().unwrap();
                    if index == 0 {
                        all.push_str(text);
                    } else {
                        all.push_str(&text[text.find("### ").unwrap()..]);
                    }
                    all
                });
        assert_eq!(reassembled, transcript);
        assert_eq!(batches.len(), 2);
        assert!(
            batches[1].config["transcript"]
                .as_str()
                .unwrap()
                .contains("prior_tmux_state")
        );
        assert!(
            batches[1].config["transcript"]
                .as_str()
                .unwrap()
                .contains("alpha")
        );
        assert_eq!(
            batches
                .iter()
                .map(|batch| batch.config["max_output_tokens"].as_u64().unwrap())
                .sum::<u64>(),
            TOTAL_OUTPUT_TOKENS
        );
        assert!(batches.iter().all(|batch| candidate_fits(
            count_for(batch),
            batch.config["max_output_tokens"].as_u64().unwrap()
        )));
    }

    #[test]
    fn a_new_projector_source_keeps_its_header_and_resets_carry() {
        let header =
            "### Screen Activity\n# Frame Analyses\n\n## Tmux change encoding\n\nchange guide\n";
        let first = unit(
            "10:00:00",
            observation(json!([{
                "id":"%1", "index":0, "active":true, "geometry":[0,0,80,24],
                "op":"snapshot", "lines":["alpha"]
            }])),
            5_100,
        );
        let second = unit(
            "10:05:00",
            observation(json!([{
                "id":"%1", "index":0, "active":true, "geometry":[0,0,80,24],
                "op":"snapshot", "lines":["beta"]
            }])),
            5_100,
        );
        let transcript = format!("prefix\n{header}{first}{header}{second}");
        let cuts = test_cuts(&transcript);
        let (_, units) = typed_units(&transcript, &cuts).unwrap();
        assert_eq!(units.len(), 2);
        assert!(
            units
                .iter()
                .all(|unit| unit.text.starts_with("### Screen Activity"))
        );

        let batches = plan_for(&transcript).unwrap().unwrap();
        assert_eq!(batches.len(), 2);
        let second_batch = batches[1].config["transcript"].as_str().unwrap();
        assert!(second_batch.contains("\"windows\":[]"));
        assert!(!second_batch.contains("alpha"));
        assert!(second_batch.contains("beta"));
    }

    #[test]
    fn one_oversized_observation_refuses_instead_of_truncating() {
        let transcript = format!(
            "prefix\n{}",
            unit(
                "10:00:00",
                observation(json!([{
                    "id":"%1", "index":0, "active":true, "geometry":[0,0,80,24],
                    "op":"snapshot", "lines":["alpha"]
                }])),
                11_000,
            )
        );
        let result = plan_for(&transcript);
        assert!(result.unwrap_err().contains("screen_source_unbatchable"));
    }

    #[test]
    fn more_than_five_lossless_batches_refuses_before_generation() {
        let mut transcript = "prefix\n".to_owned();
        for index in 0..6 {
            let operation = if index == 0 {
                json!({
                    "id":"%1", "index":0, "active":true, "geometry":[0,0,80,24],
                    "op":"snapshot", "lines":["alpha"]
                })
            } else {
                json!({
                    "id":"%1", "index":0, "active":true, "geometry":[0,0,80,24],
                    "op":"unchanged"
                })
            };
            transcript.push_str(&unit(
                &format!("10:00:0{index}"),
                observation(json!([operation])),
                9_000,
            ));
        }
        let result = plan_for(&transcript);
        assert!(result.unwrap_err().contains("more than 5 batches"));
    }

    #[test]
    fn merge_is_ordered_exact_deduped_and_capped() {
        let entity = json!({"type":"Tool","name":"tmux","role":"mentioned","context":"terminal"});
        let merged = merge_outputs(&[
            json!({"narrative":"first","entities":[entity.clone()]}).to_string(),
            json!({"narrative":"second","entities":[entity]}).to_string(),
        ])
        .expect("merge");
        let value: Value = serde_json::from_str(&merged).unwrap();
        assert_eq!(value["narrative"], "first\n\nsecond");
        assert_eq!(value["entities"].as_array().unwrap().len(), 1);

        let too_long = "x".repeat(SCREEN_NARRATIVE_MAX_CHARS + 1);
        assert!(
            merge_outputs(&[json!({"narrative":too_long,"entities":[]}).to_string()])
                .unwrap_err()
                .contains("screen_output_unmergeable")
        );
    }
}
