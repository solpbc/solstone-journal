// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Entity-observer hook.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use serde_json::{Map, Value, json};

use crate::contract::{CommitPlan, GateDecision, ParsedOutput, PrePostState};
use crate::writers::WriteIntent;
use crate::{
    ExecutionContext, PreparedTalent, RuntimeOutcome, StageError, apply_template_vars, stage_error,
};

#[derive(Clone, Debug, PartialEq)]
pub struct ObserverState {
    context: Option<String>,
}

type Counts = BTreeMap<&'static str, usize>;

const MAX_ACTIVE_ENTITIES: usize = 6;
const MAX_SOURCE_SEGMENTS: usize = 3;
const MAX_SEGMENT_CONTEXT_CHARS: usize = 800;
const MAX_ENTITY_CONTEXT_CHARS: usize = 3_800;
const MAX_OBSERVER_CONTEXT_CHARS: usize = 24_000;

fn empty_counts() -> Counts {
    BTreeMap::from([
        ("update", 0),
        ("add", 0),
        ("drop", 0),
        ("keep", 0),
        ("skipped", 0),
        ("relation_unresolved", 0),
    ])
}

fn write_outcome(
    journal: &Path,
    facet: &str,
    day: &str,
    counts: &Counts,
    error: Option<&str>,
) -> Result<(), String> {
    let path = journal
        .join("facets")
        .join(facet)
        .join("entities")
        .join(format!("{day}_observer_outcome.json"));
    let mut payload = Map::from_iter(
        counts
            .iter()
            .map(|(name, count)| ((*name).to_owned(), Value::from(*count))),
    );
    payload.insert(
        "error".to_owned(),
        error.map_or(Value::Null, |error| Value::String(error.to_owned())),
    );
    payload.insert(
        "ts".to_owned(),
        Value::from(chrono::Utc::now().timestamp_millis()),
    );
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|write_error| write_error.to_string())?;
    }
    fs::write(path, format!("{}\n", Value::Object(payload)))
        .map_err(|write_error| write_error.to_string())
}

fn target_index(value: Option<&Value>) -> Option<i64> {
    value
        .and_then(Value::as_i64)
        .filter(|_| !value.is_some_and(Value::is_boolean))
}

fn target_quote(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn attached_entities(
    journal: &Path,
    facet: &str,
) -> Result<
    (
        Vec<String>,
        Vec<solstone_core_entity::EntityResolutionEntity>,
    ),
    String,
> {
    let scoped = solstone_core_facets::list_scoped_facet_entities(journal, facet, false, false)
        .map_err(|error| error.to_string())?;
    let ids = scoped
        .iter()
        .map(|entity| entity.entity_id.clone())
        .collect::<Vec<_>>();
    let entities = scoped
        .into_iter()
        .map(|entity| solstone_core_entity::EntityResolutionEntity {
            id: Some(entity.entity_id),
            name: entity
                .identity
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            aka: entity
                .identity
                .get("aka")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect(),
            emails: Vec::new(),
            blocked: entity.blocked,
        })
        .collect();
    Ok((ids, entities))
}

fn clean_relation(
    value: Option<&Value>,
    op: &str,
    entities: &[solstone_core_entity::EntityResolutionEntity],
    journal: &Path,
    facet: &str,
    day: &str,
    entity_id: &str,
) -> Result<(Option<Value>, Option<&'static str>), String> {
    if value.is_none() || value.is_some_and(Value::is_null) || matches!(op, "drop" | "keep") {
        return Ok((None, None));
    }
    let Some(value) = value.and_then(Value::as_object) else {
        return Ok((None, Some("skipped")));
    };
    let (Some(kind), Some(target_name), Some(note)) = (
        value.get("kind").and_then(Value::as_str),
        value.get("target_name").and_then(Value::as_str),
        value.get("note").and_then(Value::as_str),
    ) else {
        return Ok((None, Some("skipped")));
    };
    if !crate::story::RELATIONS.contains(&kind) || (kind == "other" && note.trim().is_empty()) {
        return Ok((None, Some("skipped")));
    }
    let resolution = solstone_core_entity::record_entity_resolution(
        journal,
        target_name,
        entities,
        json!({"kind":"facet","facet":facet}),
        json!({"lane":"apps.entities.entity_observer","facet":facet,"day":day,"record_id":entity_id,"field":"relation.target_name"}),
        90.0,
        false,
    )
    .map_err(|error| error.to_string())?;
    let target_entity_id = (resolution.outcome
        == solstone_core_entity::EntityResolutionOutcome::Resolved)
        .then(|| {
            resolution
                .entity_index
                .and_then(|index| entities[index].id.clone())
        })
        .flatten();
    let status = target_entity_id.is_none().then_some("relation_unresolved");
    Ok((
        Some(
            json!({"kind":kind,"target_entity_id":target_entity_id,"target_name":target_name,"note":note}),
        ),
        status,
    ))
}

struct OperationContext<'a> {
    journal: &'a Path,
    facet: &'a str,
    day: &'a str,
    entity_id: &'a str,
    entities: &'a [solstone_core_entity::EntityResolutionEntity],
}

fn clean_operation(
    item: &Value,
    seen_indexes: &mut Vec<i64>,
    context: &OperationContext<'_>,
) -> Result<(Option<Value>, Option<&'static str>), String> {
    let Some(item) = item.as_object() else {
        return Ok((None, Some("skipped")));
    };
    let Some(op) = item.get("op").and_then(Value::as_str) else {
        return Ok((None, Some("skipped")));
    };
    if op == "add" {
        let Some(content) = item
            .get("content")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty())
        else {
            return Ok((None, Some("skipped")));
        };
        let (relation, status) = clean_relation(
            item.get("relation"),
            op,
            context.entities,
            context.journal,
            context.facet,
            context.day,
            context.entity_id,
        )?;
        if status == Some("skipped") {
            return Ok((None, status));
        }
        let mut clean = Map::from_iter([
            ("op".to_owned(), Value::String("add".to_owned())),
            ("content".to_owned(), Value::String(content.to_owned())),
        ]);
        if let Some(relation) = relation {
            clean.insert("relation".to_owned(), relation);
        }
        return Ok((Some(Value::Object(clean)), status));
    }
    if !matches!(op, "update" | "drop" | "keep") {
        return Ok((None, Some("skipped")));
    }
    let Some(index) = target_index(item.get("target_index")) else {
        return Ok((None, Some("skipped")));
    };
    if seen_indexes.contains(&index) {
        return Ok((None, Some("skipped")));
    }
    let quote_value = item.get("target_quote");
    if quote_value.is_some() && quote_value.and_then(Value::as_str).is_none() {
        return Ok((None, Some("skipped")));
    }
    let content = if op == "update" {
        let Some(content) = item
            .get("content")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty())
        else {
            return Ok((None, Some("skipped")));
        };
        Some(content)
    } else {
        None
    };
    seen_indexes.push(index);
    let (relation, status) = clean_relation(
        item.get("relation"),
        op,
        context.entities,
        context.journal,
        context.facet,
        context.day,
        context.entity_id,
    )?;
    if status == Some("skipped") {
        return Ok((None, status));
    }
    let mut clean = Map::from_iter([
        ("op".to_owned(), Value::String(op.to_owned())),
        ("target_index".to_owned(), Value::from(index)),
    ]);
    if let Some(quote) = target_quote(quote_value) {
        clean.insert("target_quote".to_owned(), Value::String(quote));
    }
    if let Some(content) = content {
        clean.insert("content".to_owned(), Value::String(content.to_owned()));
    }
    if let Some(relation) = relation {
        clean.insert("relation".to_owned(), relation);
    }
    Ok((Some(Value::Object(clean)), status))
}

fn merge_counts(counts: &mut Counts, source: &solstone_core_facets::ObservationOperationCounts) {
    for (name, count) in [
        ("update", source.update),
        ("add", source.add),
        ("drop", source.drop),
        ("keep", source.keep),
        ("skipped", source.skipped),
    ] {
        *counts.entry(name).or_default() += count;
    }
}

fn preflight_observation_snapshots(
    journal: &Path,
    facet: &str,
    entries: &[Value],
    attached_ids: &[String],
) -> Result<(), String> {
    let mut checked = BTreeSet::new();
    for entry in entries {
        let Some(entry) = entry.as_object() else {
            continue;
        };
        let Some(entity_id) = entry.get("entity_id").and_then(Value::as_str) else {
            continue;
        };
        if !attached_ids.iter().any(|attached| attached == entity_id)
            || !entry
                .get("operations")
                .and_then(Value::as_array)
                .is_some_and(|operations| !operations.is_empty())
            || !checked.insert(entity_id.to_owned())
        {
            continue;
        }
        let relationship_dir =
            match solstone_core_facets::resolve_observation_entity_dir(journal, facet, entity_id)
                .map_err(|error| error.to_string())?
            {
                solstone_core_facets::ObservationEntityResolution::Resolved { entity_dir } => {
                    entity_dir
                }
                solstone_core_facets::ObservationEntityResolution::NoSuchEntity => {
                    entity_id.to_owned()
                }
            };
        solstone_core_facets::load_observations_strict(journal, facet, &relationship_dir)
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

pub fn gate(prepared: &PreparedTalent, _: &ExecutionContext) -> Result<GateDecision, StageError> {
    let day = prepared
        .config
        .get("day")
        .and_then(Value::as_str)
        .filter(|day| !day.is_empty());
    if day.is_none() {
        return Ok(GateDecision::Skip("no_day".to_owned()));
    }
    let facet = prepared
        .config
        .get("facet")
        .and_then(Value::as_str)
        .filter(|facet| !facet.is_empty());
    if facet.is_none() {
        return Ok(GateDecision::Skip("no_facet".to_owned()));
    }
    Ok(GateDecision::Proceed)
}

fn skip_missing_scope(prepared: &PreparedTalent) -> RuntimeOutcome {
    let has_day = prepared
        .config
        .get("day")
        .and_then(Value::as_str)
        .is_some_and(|day| !day.is_empty());
    RuntimeOutcome::Skipped {
        stage: "entities:entity_observer".to_owned(),
        talent: prepared.name.clone(),
        reason: if has_day {
            "no_facet".to_owned()
        } else {
            "no_day".to_owned()
        },
    }
}

pub fn build(
    prepared: &mut PreparedTalent,
    context: &ExecutionContext,
) -> Result<PrePostState, RuntimeOutcome> {
    let facet = prepared.config.get("facet").and_then(Value::as_str);
    let day = prepared.config.get("day").and_then(Value::as_str);
    let observer_context = match (facet, day) {
        (Some(facet), Some(day)) if !facet.is_empty() && !day.is_empty() => Some(
            assemble_observer_context(&context.journal, facet, day).map_err(|detail| {
                RuntimeOutcome::StageFailed(stage_error(
                    "build",
                    "entities:entity_observer",
                    prepared,
                    detail,
                ))
            })?,
        ),
        _ => return Err(skip_missing_scope(prepared)),
    };
    Ok(PrePostState::EntityObserver(ObserverState {
        context: observer_context,
    }))
}

pub fn apply_prompt_override(
    prepared: &mut PreparedTalent,
    state: &PrePostState,
) -> Result<(), StageError> {
    let PrePostState::EntityObserver(state) = state else {
        return Err(stage_error(
            "prompt_override",
            "entities:entity_observer",
            prepared,
            "missing observer state",
        ));
    };
    if let Some(context) = &state.context {
        apply_template_vars(
            &mut prepared.config,
            &Map::from_iter([(
                "observer_context".to_owned(),
                Value::String(context.clone()),
            )]),
        );
    }
    Ok(())
}

pub fn parse(
    output: &str,
    _: &PreparedTalent,
    _: &PrePostState,
) -> Result<ParsedOutput, StageError> {
    Ok(ParsedOutput::Text(output.to_owned()))
}

pub fn commit(
    parsed: ParsedOutput,
    prepared: &PreparedTalent,
    _: &PrePostState,
) -> Result<CommitPlan, StageError> {
    let ParsedOutput::Text(output) = parsed else {
        return Err(stage_error(
            "commit",
            "entities:entity_observer",
            prepared,
            "expected text output",
        ));
    };
    let (Some(facet), Some(day)) = (
        prepared.config.get("facet").and_then(Value::as_str),
        prepared.config.get("day").and_then(Value::as_str),
    ) else {
        return Ok(CommitPlan::NoOutput);
    };
    Ok(CommitPlan::Write(WriteIntent::EntityObserver {
        output,
        facet: facet.to_owned(),
        day: day.to_owned(),
    }))
}

pub fn apply_result(journal: &Path, output: &str, facet: &str, day: &str) -> Result<(), String> {
    let mut counts = empty_counts();
    let mut error = None;
    let result = (|| -> Result<(), String> {
        let Value::Object(data) = serde_json::from_str(output)
            .map_err(|_| "could not parse result as JSON".to_owned())?
        else {
            return Err("result is not a JSON object".to_owned());
        };
        let Some(entries) = data.get("entities").and_then(Value::as_array) else {
            return Err("entities is not a list".to_owned());
        };
        let (ids, entities) = attached_entities(journal, facet)?;
        preflight_observation_snapshots(journal, facet, entries, &ids)?;
        for entry in entries {
            let Some(entry) = entry.as_object() else {
                continue;
            };
            let operations = entry.get("operations").and_then(Value::as_array);
            let Some(operations) = operations else {
                continue;
            };
            let Some(entity_id) = entry.get("entity_id").and_then(Value::as_str) else {
                *counts.entry("skipped").or_default() += operations.len();
                continue;
            };
            if !ids.iter().any(|id| id == entity_id) {
                *counts.entry("skipped").or_default() += operations.len();
                continue;
            }
            let mut clean = Vec::new();
            let mut seen = Vec::new();
            let operation_context = OperationContext {
                journal,
                facet,
                day,
                entity_id,
                entities: &entities,
            };
            for operation in operations {
                let (operation, status) =
                    clean_operation(operation, &mut seen, &operation_context)?;
                if let Some(status) = status {
                    *counts.entry(status).or_default() += 1;
                }
                if let Some(operation) = operation {
                    clean.push(operation);
                }
            }
            if clean.is_empty() {
                continue;
            }
            match solstone_core_facets::record_observation_ops_strict(
                journal,
                facet,
                entity_id,
                &clean,
                Some(day),
            ) {
                Ok(op_counts) => merge_counts(&mut counts, &op_counts),
                Err(write_error) => {
                    error = Some(format!("ObservationWriteError: {write_error}"));
                    *counts.entry("skipped").or_default() += clean.len();
                }
            }
        }
        Ok(())
    })();
    if let Err(detail) = result {
        error = Some(detail);
    }
    write_outcome(journal, facet, day, &counts, error.as_deref())
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct SegmentOrigin {
    label: String,
    segment: String,
    stream: String,
}

fn valid_segment_key(value: &str) -> bool {
    let Some((_, duration)) = value.split_once('_') else {
        return false;
    };
    solstone_core_format::segment::segment_parse(value).is_some()
        && duration.parse::<u64>().is_ok_and(|duration| duration > 0)
}

fn parse_segment_origin(label: &str, day: &str) -> Result<SegmentOrigin, &'static str> {
    let parts = label.split('/').collect::<Vec<_>>();
    let (origin_day, stream, segment) = match parts.as_slice() {
        [origin_day, segment] => (
            *origin_day,
            solstone_core_journal_io::DEFAULT_STREAM,
            *segment,
        ),
        [origin_day, stream, segment] if !stream.is_empty() => (*origin_day, *stream, *segment),
        _ => return Err("unsupported origin shape"),
    };
    if origin_day != day {
        return Err("origin belongs to another day");
    }
    if stream != solstone_core_journal_io::DEFAULT_STREAM
        && solstone_core_journal_io::StreamName::parse(stream).is_err()
    {
        return Err("invalid stream name");
    }
    if !valid_segment_key(segment) {
        return Err("invalid segment key");
    }
    Ok(SegmentOrigin {
        label: label.to_owned(),
        segment: segment.to_owned(),
        stream: stream.to_owned(),
    })
}

fn truncate_chars(value: &str, maximum: usize) -> String {
    if value.chars().count() <= maximum {
        return value.to_owned();
    }
    const MARKER: &str = "\n[truncated]";
    let keep = maximum.saturating_sub(MARKER.chars().count());
    value
        .chars()
        .take(keep)
        .chain(MARKER.chars().take(maximum.saturating_sub(keep)))
        .collect()
}

fn sense_evidence(segment_dir: &Path, matching_slugs: &BTreeSet<String>) -> Vec<String> {
    let path = segment_dir.join("talents/sense.json");
    if !path.exists() {
        return vec!["- Sense: unavailable (missing)".to_owned()];
    }
    let contents = match fs::read_to_string(&path) {
        Ok(contents) => contents,
        Err(error) => return vec![format!("- Sense: unavailable (read error: {error})")],
    };
    let sense = match serde_json::from_str::<Value>(&contents) {
        Ok(Value::Object(sense)) => sense,
        _ => return vec!["- Sense: unavailable (malformed JSON object)".to_owned()],
    };
    let mut lines = Vec::new();
    match sense.get("activity_summary").and_then(Value::as_str) {
        Some(summary) if !summary.trim().is_empty() => {
            lines.push(format!("- Segment activity summary: {}", summary.trim()));
        }
        _ => lines.push("- Segment activity summary: unavailable".to_owned()),
    }
    let contexts = sense
        .get("entities")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_object)
        .filter(|row| {
            row.get("name")
                .and_then(Value::as_str)
                .map(solstone_core_entity_matching::entity_slug)
                .is_some_and(|slug| matching_slugs.contains(&slug))
        })
        .filter_map(|row| row.get("context").and_then(Value::as_str))
        .map(str::trim)
        .filter(|context| !context.is_empty())
        .collect::<BTreeSet<_>>();
    if contexts.is_empty() {
        lines.push("- Matching sense context: unavailable".to_owned());
    } else {
        lines.extend(
            contexts
                .into_iter()
                .map(|context| format!("- Matching sense context: {context}")),
        );
    }
    lines
}

fn strip_modality_metadata(source: &str) -> String {
    let mut after_heading = false;
    let mut lines = Vec::new();
    for line in source.lines() {
        if line.starts_with("### ") {
            after_heading = true;
            lines.push(line);
            continue;
        }
        if after_heading && line.starts_with("Start: ") {
            after_heading = false;
            continue;
        }
        if !line.trim().is_empty() {
            after_heading = false;
        }
        lines.push(line);
    }
    lines.join("\n")
}

fn segment_evidence(
    journal: &Path,
    day: &str,
    origin: &SegmentOrigin,
    matching_slugs: &BTreeSet<String>,
) -> String {
    let mut lines = vec![format!("##### Source origin: {}", origin.label)];
    let Some(segment_dir) = solstone_core_system_health::find_segment_dir(
        journal,
        day,
        &origin.segment,
        Some(&origin.stream),
    ) else {
        lines.push("- Source: unavailable (segment missing)".to_owned());
        return truncate_chars(&lines.join("\n"), MAX_SEGMENT_CONTEXT_CHARS);
    };
    lines.extend(sense_evidence(&segment_dir, matching_slugs));
    let source_config = Map::from_iter([
        ("transcripts".to_owned(), Value::Bool(true)),
        ("percepts".to_owned(), Value::Bool(true)),
        ("talents".to_owned(), Value::Bool(false)),
    ]);
    let (source, counts) = crate::transcript::load_segment_transcript(
        journal,
        day,
        &origin.segment,
        Some(&origin.stream),
        &source_config,
    );
    if counts.total() == 0 {
        lines.push("- Transcript/percept evidence: unavailable or empty".to_owned());
    } else {
        lines.push(format!(
            "- Source records: {} transcript, {} percept",
            counts.transcripts, counts.percepts
        ));
        lines.push(strip_modality_metadata(&source));
    }
    truncate_chars(&lines.join("\n"), MAX_SEGMENT_CONTEXT_CHARS)
}

fn render_entity_packet(
    journal: &Path,
    facet: &str,
    day: &str,
    entity: &solstone_core_facets::ScopedFacetEntity,
    rows: &[Value],
) -> Result<String, String> {
    let name = entity
        .identity
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or(&entity.entity_id);
    let mut matching_slugs = BTreeSet::from([solstone_core_entity_matching::entity_slug(name)]);
    let mut summaries = BTreeSet::new();
    let mut evidence_status = Vec::new();
    let mut origins = BTreeMap::<String, SegmentOrigin>::new();
    for (row_index, row) in rows.iter().enumerate() {
        let row_number = row_index + 1;
        if let Some(row_name) = row.get("name").and_then(Value::as_str) {
            matching_slugs.insert(solstone_core_entity_matching::entity_slug(row_name));
        }
        match row.get("description").and_then(Value::as_str) {
            Some(summary) if !summary.trim().is_empty() => {
                summaries.insert(summary.trim().to_owned());
            }
            _ => evidence_status.push(format!("detection row {row_number}: summary unavailable")),
        }
        match row.get("segments") {
            None => evidence_status.push(format!(
                "detection row {row_number}: no linked source origins"
            )),
            Some(Value::Array(segments)) => {
                for (origin_index, value) in segments.iter().enumerate() {
                    let Some(label) = value.as_str().filter(|label| !label.trim().is_empty())
                    else {
                        evidence_status.push(format!(
                            "detection row {row_number} origin {}: rejected non-string or blank origin",
                            origin_index + 1
                        ));
                        continue;
                    };
                    match parse_segment_origin(label, day) {
                        Ok(origin) => {
                            origins.entry(origin.label.clone()).or_insert(origin);
                        }
                        Err(reason) => evidence_status.push(format!(
                            "detection row {row_number} origin {}: rejected ({reason})",
                            json!(label)
                        )),
                    }
                }
            }
            Some(_) => evidence_status.push(format!(
                "detection row {row_number}: rejected non-array segments field"
            )),
        }
    }
    let mut selected_origins = origins.into_values().collect::<Vec<_>>();
    selected_origins.sort_by(|left, right| {
        right
            .segment
            .cmp(&left.segment)
            .then_with(|| right.label.cmp(&left.label))
    });
    let origin_total = selected_origins.len();
    selected_origins.truncate(MAX_SOURCE_SEGMENTS);
    if origin_total > MAX_SOURCE_SEGMENTS {
        evidence_status.push(format!(
            "selected newest {MAX_SOURCE_SEGMENTS} of {origin_total} valid origins"
        ));
    }
    if selected_origins.is_empty() {
        evidence_status.push("no usable source origins".to_owned());
    } else {
        evidence_status.push(format!(
            "{} source origin(s) selected",
            selected_origins.len()
        ));
    }

    let mut lines = vec![
        format!("#### {name} ({})", entity.entity_id),
        format!(
            "- Type: {}",
            entity
                .identity
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or_default()
        ),
        format!(
            "- Description: {}",
            entity
                .identity
                .get("description")
                .and_then(Value::as_str)
                .unwrap_or_default()
        ),
        format!("- Evidence status: {}", evidence_status.join("; ")),
        String::new(),
        "Detection summaries:".to_owned(),
    ];
    if summaries.is_empty() {
        lines.push("No usable detection summaries.".to_owned());
    } else {
        lines.extend(summaries.into_iter().map(|summary| format!("- {summary}")));
    }
    lines.extend([String::new(), "Fresh source evidence:".to_owned()]);
    if selected_origins.is_empty() {
        lines.push("No source excerpts available.".to_owned());
    } else {
        for origin in selected_origins {
            lines.push(segment_evidence(journal, day, &origin, &matching_slugs));
        }
    }
    lines.extend([String::new(), "Current observations:".to_owned()]);
    let observations =
        solstone_core_facets::load_observations_strict(journal, facet, &entity.relationship_dir)
            .map_err(|error| error.to_string())?;
    if observations.is_empty() {
        lines.push("No current observations.".to_owned());
    } else {
        for (index, observation) in observations.iter().enumerate() {
            let rendered = observation_context(index, observation).ok_or_else(|| {
                format!(
                    "strict observation row for {} could not be rendered",
                    entity.entity_id
                )
            })?;
            lines.push(rendered);
        }
    }
    let packet = lines.join("\n");
    let packet_chars = packet.chars().count();
    if packet_chars > MAX_ENTITY_CONTEXT_CHARS {
        return Err(format!(
            "entity observer context for {} is {packet_chars} characters; maximum is {MAX_ENTITY_CONTEXT_CHARS}",
            entity.entity_id
        ));
    }
    Ok(packet)
}

fn assemble_observer_context(journal: &Path, facet: &str, day: &str) -> Result<String, String> {
    let scoped = solstone_core_facets::list_scoped_facet_entities(journal, facet, false, false)
        .map_err(|error| error.to_string())?;
    let detected = solstone_core_facets::read_detected_entities_strict(journal, facet, day)
        .map_err(|error| error.to_string())?;
    let candidates = scoped
        .iter()
        .map(
            |entity| solstone_core_entity_matching::EntityNameCandidate {
                id: Some(entity.entity_id.clone()),
                name: entity
                    .identity
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                aka: entity
                    .identity
                    .get("aka")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect(),
                emails: Vec::new(),
            },
        )
        .collect::<Vec<_>>();
    let mut active =
        BTreeMap::<String, (solstone_core_facets::ScopedFacetEntity, Vec<Value>)>::new();
    for row in detected {
        let name = row
            .get("name")
            .and_then(Value::as_str)
            .expect("strict detected rows have names");
        if let Some(found) =
            solstone_core_entity_matching::find_matching_entity(name, &candidates, 90.0)
        {
            let entity = scoped[found.candidate_index].clone();
            active
                .entry(entity.entity_id.clone())
                .or_insert_with(|| (entity, Vec::new()))
                .1
                .push(row);
        }
    }
    if active.is_empty() {
        return Ok("No active entities found for this day.".to_owned());
    }
    let total = active.len();
    let selected = active
        .into_values()
        .take(MAX_ACTIVE_ENTITIES)
        .collect::<Vec<_>>();
    let mut sections = Vec::with_capacity(selected.len());
    for (entity, rows) in &selected {
        sections.push(render_entity_packet(journal, facet, day, entity, rows)?);
    }
    let mut context = [
        "# Entity Observer Context".to_owned(),
        String::new(),
        format!("## Facet: {facet}"),
        format!("## Day: {day}"),
        format!("## Active Entities: {} of {total} active", sections.len()),
        String::new(),
        "### Entities".to_owned(),
        String::new(),
    ]
    .join("\n");
    context.push_str(&sections.join("\n\n---\n\n"));
    let context_chars = context.chars().count();
    if context_chars > MAX_OBSERVER_CONTEXT_CHARS {
        return Err(format!(
            "entity observer context is {context_chars} characters; maximum is {MAX_OBSERVER_CONTEXT_CHARS}"
        ));
    }
    Ok(context)
}

// Keep the identifying quote distinct from both the full content and provenance.
// The writer still verifies it against the current observation at this index.
fn observation_context(index: usize, observation: &Value) -> Option<String> {
    let content = observation.get("content")?.as_str()?.trim();
    if content.is_empty() {
        return None;
    }
    let quote: String = content.chars().take(200).collect();
    let source = observation
        .get("source_day")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    Some(format!(
        "{index}. target_quote: {}\n   source_day: {}\n   content: {}",
        json!(quote),
        json!(source),
        json!(content)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: &str = "20260101";

    fn attach(root: &Path, name: &str) {
        solstone_core_facets::attach_or_reactivate_entity(root, "work", "Person", name, "")
            .unwrap();
    }

    fn segment_path(root: &Path, origin: &str) -> std::path::PathBuf {
        let path = root.join("chronicle").join(origin);
        fs::create_dir_all(path.join("talents")).unwrap();
        path
    }

    fn write_source_segment(
        root: &Path,
        origin: &str,
        transcript: &str,
        percept: &str,
        activity: &str,
        entity_context: &str,
    ) {
        let path = segment_path(root, origin);
        fs::write(
            path.join("audio.jsonl"),
            format!(
                "{}\n{}\n",
                json!({
                    "raw":"audio.wav", "backend":"parakeet", "model":"field-model.gguf",
                    "device":"cpu", "compute_type":"q8_0", "duration":180.0,
                    "noisy":false, "loud_windows":180, "speech_loud_windows":180,
                    "loud_speech_ratio":1.0, "overlap_fraction":0.0104,
                    "overlap_detector":"pyannote-segmentation-3.0-onnx",
                    "speaker_evidence":"multi", "speaker_evidence_multi_fraction":0.1395,
                    "speaker_evidence_version":"windowed-slots-v1", "stream":"field.audio",
                    "_solstone_processing":{"schema":"solstone.processing.v1","state":"analyzed","reason_code":"ok","handler":"transcribe","attempted_at":"2026-07-25T05:24:07Z","input_size":5760078}
                }),
                json!({"start":"00:00:00","text":transcript})
            ),
        )
        .unwrap();
        fs::write(
            path.join("screen.jsonl"),
            json!({"timestamp":0,"content":{"window":percept}}).to_string(),
        )
        .unwrap();
        fs::write(
            path.join("talents/sense.json"),
            json!({
                "activity_summary": activity,
                "entities": [
                    {"name":"Ada", "context":entity_context},
                    {"name":"Grace", "context":"OTHER_ENTITY_CONTAMINANT"}
                ]
            })
            .to_string(),
        )
        .unwrap();
    }

    fn save_detection(root: &Path, name: &str, description: &str, origin: &str) {
        solstone_core_facets::upsert_detection_segment(
            root,
            "work",
            DAY,
            origin,
            &[solstone_core_facets::DetectedEntityInput {
                entity_type: "Person".to_owned(),
                name: name.to_owned(),
                description: description.to_owned(),
            }],
        )
        .unwrap();
    }

    fn observer_request(root: &Path) -> crate::GenerateRequest {
        let mut prepared = PreparedTalent {
            name: "entity_observer".to_owned(),
            config: Map::from_iter([
                ("day".to_owned(), Value::String(DAY.to_owned())),
                ("facet".to_owned(), Value::String("work".to_owned())),
                (
                    "prompt".to_owned(),
                    Value::String("$observer_context".to_owned()),
                ),
            ]),
        };
        let state = build(
            &mut prepared,
            &ExecutionContext {
                journal: root.to_owned(),
            },
        )
        .unwrap();
        apply_prompt_override(&mut prepared, &state).unwrap();
        crate::generate_request(&prepared)
    }

    fn request_text(request: &crate::GenerateRequest) -> String {
        request
            .contents
            .iter()
            .map(|part| match part {
                crate::ContentPart::Text { text } => text.as_str(),
                crate::ContentPart::Image { .. } => "",
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn actual_generate_request_contains_detection_linked_source_and_changes_with_it() {
        let root = tempfile::tempdir().unwrap();
        attach(root.path(), "Ada");
        let origin = "20260101/field/090000_60";
        write_source_segment(
            root.path(),
            origin,
            "TRANSCRIPT_SENTINEL durable expertise in orbital mechanics",
            "PERCEPT_SENTINEL planning window",
            "ACTIVITY_SENTINEL reviewed launch architecture",
            "SENSE_SENTINEL explained the launch constraint",
        );
        save_detection(root.path(), "Ada", "UNCHANGED_DETECTION", origin);

        let first = observer_request(root.path());
        let first_text = request_text(&first);
        for sentinel in [
            "TRANSCRIPT_SENTINEL",
            "PERCEPT_SENTINEL",
            "ACTIVITY_SENTINEL",
            "SENSE_SENTINEL",
            "UNCHANGED_DETECTION",
            origin,
        ] {
            assert!(first_text.contains(sentinel), "missing {sentinel}");
        }
        assert!(!first_text.contains("Speaker_evidence_version"));
        assert!(!first_text.contains("OTHER_ENTITY_CONTAMINANT"));

        let path = root.path().join("chronicle").join(origin);
        fs::write(
            path.join("audio.jsonl"),
            r#"{"start":"00:00:00","text":"CHANGED_SOURCE_SENTINEL durable launch preference"}"#,
        )
        .unwrap();
        let second = observer_request(root.path());
        let second_text = request_text(&second);
        assert_ne!(first.contents, second.contents);
        assert!(second_text.contains("CHANGED_SOURCE_SENTINEL"));
        assert!(second_text.contains("UNCHANGED_DETECTION"));
    }

    #[test]
    fn origin_resolution_is_exact_and_partial_failures_stay_visible() {
        let root = tempfile::tempdir().unwrap();
        attach(root.path(), "Ada");
        write_source_segment(
            root.path(),
            "20260101/090000_60",
            "DIRECT_SENTINEL",
            "direct percept",
            "direct activity",
            "direct context",
        );
        write_source_segment(
            root.path(),
            "20260101/field/090000_60",
            "COLLIDING_NAMED_SENTINEL",
            "named percept",
            "named activity",
            "named context",
        );
        write_source_segment(
            root.path(),
            "090000_60",
            "TRAVERSAL_SENTINEL",
            "traversal percept",
            "traversal activity",
            "traversal context",
        );
        let detected_path = root.path().join("facets/work/entities/20260101.jsonl");
        fs::create_dir_all(detected_path.parent().unwrap()).unwrap();
        fs::write(
            detected_path,
            format!(
                "{}\n",
                json!({
                    "id":"ada", "type":"Person", "name":"Ada",
                    "description":"summary", "segments":[42, "20251231/080000_60", "20260101/bad", "20260101/../090000_60", "20260101/090000_60"]
                })
            ),
        )
        .unwrap();

        let context = assemble_observer_context(root.path(), "work", DAY).unwrap();
        assert!(context.contains("DIRECT_SENTINEL"));
        assert!(!context.contains("COLLIDING_NAMED_SENTINEL"));
        assert!(!context.contains("TRAVERSAL_SENTINEL"));
        assert!(context.contains("rejected non-string or blank origin"));
        assert!(context.contains("origin belongs to another day"));
        assert!(context.contains("invalid segment key"));
        assert!(context.contains("invalid stream name"));
    }

    #[test]
    fn multiple_rows_union_newest_origins_and_keep_each_summary_once() {
        let root = tempfile::tempdir().unwrap();
        attach(root.path(), "Ada");
        for (origin, sentinel) in [
            ("20260101/field/080000_60", "OLD_EXCLUDED"),
            ("20260101/090000_60", "NINE_INCLUDED"),
            ("20260101/field/100000_60", "TEN_INCLUDED"),
            ("20260101/110000_60", "ELEVEN_INCLUDED"),
        ] {
            write_source_segment(root.path(), origin, sentinel, "p", "a", "c");
        }
        let detected_path = root.path().join("facets/work/entities/20260101.jsonl");
        fs::create_dir_all(detected_path.parent().unwrap()).unwrap();
        let rows = [
            json!({"id":"ada-1","type":"Person","name":"Ada","description":"SUMMARY_ONE","segments":["20260101/field/080000_60","20260101/field/100000_60"]}),
            json!({"id":"ada-2","type":"Person","name":"Ada","description":"SUMMARY_TWO","segments":["20260101/090000_60","20260101/110000_60","20260101/110000_60"]}),
        ];
        fs::write(
            detected_path,
            rows.iter()
                .map(|row| row.to_string() + "\n")
                .collect::<String>(),
        )
        .unwrap();

        let context = assemble_observer_context(root.path(), "work", DAY).unwrap();
        assert_eq!(context.matches("SUMMARY_ONE").count(), 1);
        assert_eq!(context.matches("SUMMARY_TWO").count(), 1);
        for sentinel in ["NINE_INCLUDED", "TEN_INCLUDED", "ELEVEN_INCLUDED"] {
            assert!(context.contains(sentinel), "missing {sentinel}");
        }
        assert!(!context.contains("OLD_EXCLUDED"));
        assert!(context.contains("selected newest 3 of 4 valid origins"));
    }

    #[test]
    fn supported_originless_row_survives_beside_valid_evidence() {
        let root = tempfile::tempdir().unwrap();
        attach(root.path(), "Ada");
        attach(root.path(), "Grace");
        solstone_core_facets::save_detected_entity(
            root.path(),
            "work",
            DAY,
            "Person",
            "Ada",
            "originless summary",
        )
        .unwrap();
        let origin = "20260101/field/090000_60";
        write_source_segment(root.path(), origin, "GRACE_SOURCE", "p", "a", "c");
        save_detection(root.path(), "Grace", "with source", origin);

        let context = assemble_observer_context(root.path(), "work", DAY).unwrap();
        assert!(context.contains("originless summary"));
        assert!(context.contains("no linked source origins"));
        assert!(context.contains("GRACE_SOURCE"));
    }

    #[test]
    fn invalid_optional_detection_evidence_is_visible_without_hiding_siblings() {
        let root = tempfile::tempdir().unwrap();
        attach(root.path(), "Ada");
        attach(root.path(), "Grace");
        let origin = "20260101/field/090000_60";
        write_source_segment(root.path(), origin, "GRACE_VALID_SOURCE", "p", "a", "c");
        let detected_path = root.path().join("facets/work/entities/20260101.jsonl");
        fs::create_dir_all(detected_path.parent().unwrap()).unwrap();
        let rows = [
            json!({"id":"ada","type":"Person","name":"Ada","description":null,"segments":{"bad":true}}),
            json!({"id":"grace","type":"Person","name":"Grace","description":"valid summary","segments":[origin]}),
        ];
        fs::write(
            detected_path,
            rows.iter()
                .map(|row| row.to_string() + "\n")
                .collect::<String>(),
        )
        .unwrap();

        let context = assemble_observer_context(root.path(), "work", DAY).unwrap();
        assert!(context.contains("summary unavailable"));
        assert!(context.contains("rejected non-array segments field"));
        assert!(context.contains("GRACE_VALID_SOURCE"));
    }

    #[test]
    fn nullable_relations_persist_and_malformed_relations_do_not() {
        let root = tempfile::tempdir().unwrap();
        attach(root.path(), "Ada");
        apply_result(
            root.path(),
            &json!({"entities":[{"entity_id":"ada","operations":[{
                "op":"add", "target_index":null, "content":"Original durable fact.",
                "target_quote":null, "reasoning":"new fact", "relation":null
            }]}], "summary":"added"})
            .to_string(),
            "work",
            DAY,
        )
        .unwrap();
        apply_result(
            root.path(),
            &json!({"entities":[{"entity_id":"ada","operations":[{
                "op":"update", "target_index":0, "content":"Updated durable fact.",
                "target_quote":"Original durable fact.", "reasoning":"correction", "relation":null
            }]}], "summary":"updated"})
            .to_string(),
            "work",
            DAY,
        )
        .unwrap();
        let before = solstone_core_facets::load_observations(root.path(), "work", "ada").unwrap();
        assert_eq!(before[0]["content"], "Updated durable fact.");

        apply_result(
            root.path(),
            &json!({"entities":[{"entity_id":"ada","operations":[{
                "op":"add", "target_index":null, "content":"Must not persist.",
                "target_quote":null, "reasoning":"bad relation", "relation":"malformed"
            }]}], "summary":"bad"})
            .to_string(),
            "work",
            DAY,
        )
        .unwrap();
        assert_eq!(
            solstone_core_facets::load_observations(root.path(), "work", "ada").unwrap(),
            before
        );
        let outcome: Value = serde_json::from_slice(
            &fs::read(
                root.path()
                    .join("facets/work/entities/20260101_observer_outcome.json"),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(outcome["skipped"], 1);
    }

    #[test]
    fn invalid_duplicate_and_mismatched_targets_are_guarded() {
        let root = tempfile::tempdir().unwrap();
        attach(root.path(), "Ada");
        solstone_core_facets::add_observation(
            root.path(),
            "work",
            "ada",
            "Guarded durable fact.",
            Some(DAY),
            None,
        )
        .unwrap();
        let original = solstone_core_facets::load_observations(root.path(), "work", "ada").unwrap();
        apply_result(
            root.path(),
            &json!({"entities":[{"entity_id":"ada","operations":[
                {"op":"update","target_index":true,"content":"bad","target_quote":"Guarded durable fact.","reasoning":"bad index","relation":null},
                {"op":"update","target_index":0,"content":"bad","target_quote":"mismatch","reasoning":"bad quote","relation":null}
            ]}],"summary":"guards"}).to_string(),
            "work",
            DAY,
        )
        .unwrap();
        assert_eq!(
            solstone_core_facets::load_observations(root.path(), "work", "ada").unwrap(),
            original
        );
        let outcome_path = root
            .path()
            .join("facets/work/entities/20260101_observer_outcome.json");
        let first_outcome: Value =
            serde_json::from_slice(&fs::read(&outcome_path).unwrap()).unwrap();
        assert_eq!(first_outcome["skipped"], 2);

        apply_result(
            root.path(),
            &json!({"entities":[{"entity_id":"ada","operations":[
                {"op":"keep","target_index":0,"content":null,"target_quote":"Guarded durable fact.","reasoning":null,"relation":null},
                {"op":"drop","target_index":0,"content":null,"target_quote":"Guarded durable fact.","reasoning":"duplicate","relation":null}
            ]}],"summary":"duplicates"}).to_string(),
            "work",
            DAY,
        )
        .unwrap();
        assert_eq!(
            solstone_core_facets::load_observations(root.path(), "work", "ada").unwrap(),
            original
        );
        let outcome: Value = serde_json::from_slice(&fs::read(outcome_path).unwrap()).unwrap();
        assert_eq!(outcome["keep"], 1);
        assert_eq!(outcome["skipped"], 1);
    }

    #[test]
    fn shipped_schema_accepts_null_and_rejects_malformed_relation() {
        let schema_path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../payload/solstone/apps/entities/talent/entity_observer.schema.json");
        let schema: Value =
            serde_json::from_str(&fs::read_to_string(schema_path).unwrap()).unwrap();
        let validator = jsonschema::validator_for(&schema).unwrap();
        let output = |relation| {
            json!({
                "entities":[{"entity_id":"ada","operations":[{
                    "op":"add", "target_index":null, "content":"Fact", "target_quote":null,
                    "reasoning":"new", "relation":relation
                }]}], "summary":"result"
            })
        };
        assert!(validator.is_valid(&output(Value::Null)));
        assert!(!validator.is_valid(&output(json!("malformed"))));
    }

    #[test]
    fn strict_snapshot_refusal_is_durable_and_never_rewrites_observations() {
        let root = tempfile::tempdir().unwrap();
        attach(root.path(), "Ada");
        let path = root
            .path()
            .join("facets/work/entities/ada/observations.jsonl");
        fs::write(&path, "{\"content\":\"valid\"}\n{}\n").unwrap();
        let before = fs::read(&path).unwrap();
        apply_result(
            root.path(),
            &json!({"entities":[{"entity_id":"ada","operations":[{
                "op":"add", "target_index":null, "content":"Must not persist",
                "target_quote":null, "reasoning":"new", "relation":null
            }]}], "summary":"result"})
            .to_string(),
            "work",
            DAY,
        )
        .unwrap();
        assert_eq!(fs::read(&path).unwrap(), before);
        let outcome: Value = serde_json::from_slice(
            &fs::read(
                root.path()
                    .join("facets/work/entities/20260101_observer_outcome.json"),
            )
            .unwrap(),
        )
        .unwrap();
        assert!(
            outcome["error"]
                .as_str()
                .unwrap()
                .contains("malformed observation")
        );
    }

    #[test]
    fn strict_preflight_prevents_partial_multi_entity_writes() {
        let root = tempfile::tempdir().unwrap();
        attach(root.path(), "Ada");
        attach(root.path(), "Grace");
        let grace_path = root
            .path()
            .join("facets/work/entities/grace/observations.jsonl");
        fs::write(&grace_path, "{}\n").unwrap();
        apply_result(
            root.path(),
            &json!({"entities":[
                {"entity_id":"ada","operations":[{
                    "op":"add", "target_index":null, "content":"Must not partially persist",
                    "target_quote":null, "reasoning":"new", "relation":null
                }]},
                {"entity_id":"grace","operations":[{
                    "op":"add", "target_index":null, "content":"Also blocked",
                    "target_quote":null, "reasoning":"new", "relation":null
                }]}
            ], "summary":"result"})
            .to_string(),
            "work",
            DAY,
        )
        .unwrap();
        assert!(
            solstone_core_facets::load_observations(root.path(), "work", "ada")
                .unwrap()
                .is_empty()
        );
        assert_eq!(fs::read_to_string(grace_path).unwrap(), "{}\n");
        let outcome: Value = serde_json::from_slice(
            &fs::read(
                root.path()
                    .join("facets/work/entities/20260101_observer_outcome.json"),
            )
            .unwrap(),
        )
        .unwrap();
        assert!(
            outcome["error"]
                .as_str()
                .unwrap()
                .contains("malformed observation")
        );
    }

    #[test]
    fn strict_assembly_refuses_directory_and_json_valid_unusable_rows() {
        let directory_case = tempfile::tempdir().unwrap();
        attach(directory_case.path(), "Ada");
        fs::create_dir_all(
            directory_case
                .path()
                .join("facets/work/entities/20260101.jsonl"),
        )
        .unwrap();
        assert!(assemble_observer_context(directory_case.path(), "work", DAY).is_err());

        let detected_case = tempfile::tempdir().unwrap();
        attach(detected_case.path(), "Ada");
        fs::write(
            detected_case
                .path()
                .join("facets/work/entities/20260101.jsonl"),
            "{}\n",
        )
        .unwrap();
        assert!(assemble_observer_context(detected_case.path(), "work", DAY).is_err());

        for malformed in ["\"scalar\"\n", "[]\n", "{}\n", "{\"content\":\"  \"}\n"] {
            let observation_case = tempfile::tempdir().unwrap();
            attach(observation_case.path(), "Ada");
            solstone_core_facets::save_detected_entity(
                observation_case.path(),
                "work",
                DAY,
                "Person",
                "Ada",
                "summary",
            )
            .unwrap();
            fs::write(
                observation_case
                    .path()
                    .join("facets/work/entities/ada/observations.jsonl"),
                malformed,
            )
            .unwrap();
            assert!(assemble_observer_context(observation_case.path(), "work", DAY).is_err());
        }
    }

    #[test]
    fn failed_outcome_publication_returns_a_caller_visible_error() {
        let root = tempfile::tempdir().unwrap();
        attach(root.path(), "Ada");
        let observations = root
            .path()
            .join("facets/work/entities/ada/observations.jsonl");
        fs::write(&observations, "{}\n").unwrap();
        fs::create_dir_all(
            root.path()
                .join("facets/work/entities/20260101_observer_outcome.json"),
        )
        .unwrap();
        let before = fs::read(&observations).unwrap();
        let error = apply_result(
            root.path(),
            &json!({"entities":[{"entity_id":"ada","operations":[{
                "op":"add", "target_index":null, "content":"Must not persist",
                "target_quote":null, "reasoning":"new", "relation":null
            }]}], "summary":"result"})
            .to_string(),
            "work",
            DAY,
        )
        .unwrap_err();
        assert!(!error.is_empty());
        assert_eq!(fs::read(&observations).unwrap(), before);
    }

    #[test]
    fn empty_result_is_a_successful_no_change() {
        let root = tempfile::tempdir().unwrap();
        apply_result(
            root.path(),
            r#"{"entities":[],"summary":"No durable changes."}"#,
            "work",
            DAY,
        )
        .unwrap();
        let outcome: Value = serde_json::from_slice(
            &fs::read(
                root.path()
                    .join("facets/work/entities/20260101_observer_outcome.json"),
            )
            .unwrap(),
        )
        .unwrap();
        for key in ["add", "update", "drop", "keep", "skipped"] {
            assert_eq!(outcome[key], 0, "nonzero {key}");
        }
        assert!(outcome["error"].is_null());
    }

    #[test]
    fn entity_selection_and_context_bounds_do_not_erase_the_sixth_packet() {
        let root = tempfile::tempdir().unwrap();
        for index in 0..7 {
            let name = format!("Person {index}");
            attach(root.path(), &name);
            solstone_core_facets::save_detected_entity(
                root.path(),
                "work",
                DAY,
                "Person",
                &name,
                &format!("SUMMARY_{index}_{}", "é😀".repeat(500)),
            )
            .unwrap();
        }
        let context = assemble_observer_context(root.path(), "work", DAY).unwrap();
        assert!(context.chars().count() <= MAX_OBSERVER_CONTEXT_CHARS);
        for index in 0..6 {
            assert!(context.contains(&format!("#### Person {index}")));
            assert!(context.contains("Evidence status:"));
        }
        assert!(!context.contains("#### Person 6"));
        assert!(context.contains("6 of 7 active"));
    }

    #[test]
    fn over_budget_observation_inventory_refuses_generation() {
        let root = tempfile::tempdir().unwrap();
        attach(root.path(), "Ada");
        solstone_core_facets::save_detected_entity(
            root.path(),
            "work",
            DAY,
            "Person",
            "Ada",
            "summary",
        )
        .unwrap();
        solstone_core_facets::add_observation(
            root.path(),
            "work",
            "ada",
            &"é😀".repeat(2_000),
            Some(DAY),
            None,
        )
        .unwrap();
        let error = assemble_observer_context(root.path(), "work", DAY).unwrap_err();
        assert!(error.contains("maximum is 3800"));
    }

    #[test]
    fn context_quote_is_bounded_verbatim_and_separate_from_source() {
        for content in [
            "Short observation.".to_owned(),
            "é😀\n\"quoted\" ".repeat(80),
        ] {
            let rendered =
                observation_context(3, &json!({"content":content, "source_day":"20260712"}))
                    .unwrap();
            let mut lines = rendered.lines();
            let quote: String = serde_json::from_str(
                lines
                    .next()
                    .unwrap()
                    .strip_prefix("3. target_quote: ")
                    .unwrap(),
            )
            .unwrap();
            assert!(!quote.is_empty());
            assert!(quote.chars().count() <= 200);
            assert!(content.starts_with(&quote));
            assert_eq!(lines.next().unwrap(), "   source_day: \"20260712\"");
            let full: String =
                serde_json::from_str(lines.next().unwrap().strip_prefix("   content: ").unwrap())
                    .unwrap();
            assert_eq!(full, content.trim());
            assert!(lines.next().is_none());
        }
        assert!(observation_context(0, &json!({"content":"  "})).is_none());
    }

    #[test]
    fn rendered_quote_supports_guarded_keep_and_update_of_long_observation() {
        let root = tempfile::tempdir().unwrap();
        solstone_core_facets::attach_or_reactivate_entity(root.path(), "work", "Person", "Ada", "")
            .unwrap();
        let content = "Long observation with Unicode é😀. ".repeat(30);
        let apply = |operation: Value| {
            apply_result(
                root.path(),
                &json!({"entities":[{"entity_id":"ada","operations":[operation]}]}).to_string(),
                "work",
                "20260101",
            )
            .unwrap();
        };
        apply(json!({"op":"add","content":content}));
        let original = solstone_core_facets::load_observations(root.path(), "work", "ada").unwrap();
        let rendered = observation_context(0, &original[0]).unwrap();
        let quote: String = serde_json::from_str(
            rendered
                .lines()
                .next()
                .unwrap()
                .strip_prefix("0. target_quote: ")
                .unwrap(),
        )
        .unwrap();
        apply(json!({"op":"keep","target_index":0,"target_quote":quote}));
        let outcome: Value = serde_json::from_slice(
            &fs::read(
                root.path()
                    .join("facets/work/entities/20260101_observer_outcome.json"),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(outcome["keep"], 1);
        assert_eq!(
            solstone_core_facets::load_observations(root.path(), "work", "ada").unwrap(),
            original
        );
        apply(
            json!({"op":"update","target_index":0,"target_quote":"does not match","content":"Wrong."}),
        );
        assert_eq!(
            solstone_core_facets::load_observations(root.path(), "work", "ada").unwrap(),
            original
        );
        apply(
            json!({"op":"update","target_index":0,"target_quote":quote,"content":"Updated fact."}),
        );
        let updated = solstone_core_facets::load_observations(root.path(), "work", "ada").unwrap();
        assert_eq!(updated[0]["content"], "Updated fact.");
    }

    #[test]
    fn missing_scope_skips_the_stage() {
        let root = tempfile::tempdir().unwrap();
        let context = ExecutionContext {
            journal: root.path().to_owned(),
        };
        let cases = [
            (Map::new(), "no_day"),
            (
                Map::from_iter([("facet".to_owned(), Value::String("work".to_owned()))]),
                "no_day",
            ),
            (
                Map::from_iter([("day".to_owned(), Value::String("20260101".to_owned()))]),
                "no_facet",
            ),
        ];
        for (config, reason) in cases {
            let mut prepared = PreparedTalent {
                name: "entity_observer".to_owned(),
                config: Map::from_iter([(
                    "prompt".to_owned(),
                    Value::String("$observer_context".to_owned()),
                )])
                .into_iter()
                .chain(config)
                .collect(),
            };
            match gate(&prepared, &context) {
                Ok(GateDecision::Skip(skipped)) => assert_eq!(skipped, reason),
                other => panic!("expected skip {reason}, got {other:?}"),
            }
            match build(&mut prepared, &context) {
                Err(RuntimeOutcome::Skipped {
                    stage,
                    talent,
                    reason: skipped,
                }) => {
                    assert_eq!(stage, "entities:entity_observer");
                    assert_eq!(talent, "entity_observer");
                    assert_eq!(skipped, reason);
                }
                other => panic!("expected skipped {reason}, got {other:?}"),
            }
        }
    }

    #[test]
    fn relation_cleaning_uses_the_shared_relation_set() {
        // Derived from solstone/apps/entities/talent/entity_observer.py:82-139.
        let root = tempfile::tempdir().unwrap();
        let entities = Vec::new();
        let context = OperationContext {
            journal: root.path(),
            facet: "work",
            day: "20260101",
            entity_id: "ada",
            entities: &entities,
        };
        let (accepted, status) = clean_operation(&json!({"op":"add","content":"Works with Ada.","relation":{"kind":"works-with","target_name":"Unknown","note":"collaboration"}}), &mut Vec::new(), &context).unwrap();
        assert_eq!(status, Some("relation_unresolved"));
        assert!(accepted.is_some());
        let (rejected, status) = clean_operation(&json!({"op":"add","content":"x","relation":{"kind":"not-a-relation","target_name":"Ada","note":"x"}}), &mut Vec::new(), &context).unwrap();
        assert!(rejected.is_none());
        assert_eq!(status, Some("skipped"));
    }

    #[test]
    fn operation_cleaning_and_target_boundaries_match_reference() {
        // Derived from solstone/apps/entities/talent/entity_observer.py:67-80,141-211.
        assert_eq!(target_index(Some(&json!(4))), Some(4));
        assert_eq!(target_index(Some(&json!(true))), None);
        assert_eq!(
            target_quote(Some(&json!("  quote  "))),
            Some("quote".to_owned())
        );
        assert_eq!(target_quote(Some(&json!("  "))), None);
        let root = tempfile::tempdir().unwrap();
        let entities = Vec::new();
        let context = OperationContext {
            journal: root.path(),
            facet: "work",
            day: "20260101",
            entity_id: "ada",
            entities: &entities,
        };
        let (accepted, status) = clean_operation(
            &json!({"op":"add","content":"  durable fact  "}),
            &mut Vec::new(),
            &context,
        )
        .unwrap();
        assert_eq!(status, None);
        assert_eq!(accepted.unwrap()["content"], "durable fact");
        let (rejected, status) = clean_operation(
            &json!({"op":"update","target_index":true,"content":"x"}),
            &mut Vec::new(),
            &context,
        )
        .unwrap();
        assert!(rejected.is_none());
        assert_eq!(status, Some("skipped"));
    }

    #[test]
    fn malformed_result_writes_an_error_outcome_sidecar() {
        // Derived from solstone/apps/entities/talent/entity_observer.py:55-64,225-242.
        let root = tempfile::tempdir().unwrap();
        apply_result(root.path(), "not json", "work", "20260101").unwrap();
        let sidecar: Value = serde_json::from_str(
            &fs::read_to_string(
                root.path()
                    .join("facets/work/entities/20260101_observer_outcome.json"),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(sidecar["error"], "could not parse result as JSON");
        assert_eq!(sidecar["add"], 0);
    }

    #[test]
    fn clean_operations_persist_observations_and_a_success_outcome() {
        // Derived from solstone/apps/entities/talent/entity_observer.py:218-318.
        let root = tempfile::tempdir().unwrap();
        solstone_core_facets::attach_or_reactivate_entity(root.path(), "work", "Person", "Ada", "")
            .unwrap();
        apply_result(
            root.path(),
            r#"{"entities":[{"entity_id":"ada","operations":[{"op":"add","content":"Prefers concise updates."}]}]}"#,
            "work",
            "20260101",
        )
        .unwrap();
        let observations =
            solstone_core_facets::load_observations(root.path(), "work", "ada").unwrap();
        assert_eq!(observations[0]["content"], "Prefers concise updates.");
        let sidecar: Value = serde_json::from_str(
            &fs::read_to_string(
                root.path()
                    .join("facets/work/entities/20260101_observer_outcome.json"),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(sidecar["add"], 1);
        assert!(sidecar["error"].is_null());
    }

    #[test]
    fn apply_result_writes_under_the_relationship_dir() {
        let root = tempfile::tempdir().unwrap();
        solstone_core_facets::create_facet(root.path(), "work", "Work", "", "blue", "💼", None)
            .unwrap();
        let identity = root.path().join("entities/dir-ada");
        fs::create_dir_all(&identity).unwrap();
        fs::write(
            identity.join("entity.json"),
            br#"{"id":"effective-ada","name":"Ada"}"#,
        )
        .unwrap();
        let relationship = root.path().join("facets/work/entities/legacy-ada");
        fs::create_dir_all(&relationship).unwrap();
        fs::write(
            relationship.join("entity.json"),
            br#"{"entity_id":"effective-ada"}"#,
        )
        .unwrap();

        apply_result(
            root.path(),
            r#"{"entities":[{"entity_id":"effective-ada","operations":[{"op":"add","content":"from apply"}]}]}"#,
            "work",
            "20260101",
        )
        .unwrap();
        let observations =
            solstone_core_facets::load_observations(root.path(), "work", "legacy-ada").unwrap();
        assert_eq!(observations[0]["content"], "from apply");
        assert!(
            solstone_core_facets::load_observations(root.path(), "work", "effective-ada")
                .unwrap()
                .is_empty()
        );
    }
}
