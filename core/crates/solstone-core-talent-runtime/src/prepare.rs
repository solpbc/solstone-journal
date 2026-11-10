// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};
use solstone_core_talent_config::{TalentConfig, discover, is_truthy, merge, validate};

use crate::{ExecutionContext, PreparedTalent, transcript};

pub(crate) const SPARSE_INPUT_NOTE: &str =
    "**Input Note:** Limited recordings for this day. Scale analysis to available input.\n\n";

#[derive(Clone, Debug)]
pub struct RuntimePaths {
    pub talent_root: PathBuf,
    pub apps_root: PathBuf,
    pub templates_dir: PathBuf,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrepareMode {
    Execute,
    Preview,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PrepareFailure {
    Refusal(String),
    NoBrainConfigured,
}

impl std::fmt::Display for PrepareFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Refusal(message) => formatter.write_str(message),
            Self::NoBrainConfigured => {
                formatter.write_str("No model is chosen yet. Choose one in Thinking.")
            }
        }
    }
}

pub fn prepare(
    request: Map<String, Value>,
    paths: &RuntimePaths,
    context: &ExecutionContext,
    mode: PrepareMode,
) -> Result<PreparedTalent, PrepareFailure> {
    let name = request
        .get("name")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| PrepareFailure::Refusal("talent request missing name".to_owned()))?
        .to_owned();
    let config = resolve_talent_config(&name, paths, context)?;
    reject_request_fields(&config, &request, &name)?;
    let focused_facet = request.get("facet").and_then(Value::as_str);
    let mut composed = solstone_core_talent_cli::compose_talent(
        &config,
        &context.journal,
        &paths.templates_dir,
        focused_facet,
    )
    .map_err(PrepareFailure::Refusal)?;
    // Only native source admission below may supply this runtime-owned body.
    composed.remove(crate::story::SOURCE_BODY_KEY);
    let talent_schema = composed.get("json_schema").cloned();
    for (key, value) in request {
        if !value.is_null()
            && !matches!(
                key.as_str(),
                "unavailable_selection"
                    | "_memory_sources"
                    | crate::story::SOURCE_BODY_KEY
                    | crate::story::INPUT_WHOLE_KEY
                    | crate::product_names::VIEW_KEY
            )
        {
            composed.insert(key, value);
        }
    }
    if mode == PrepareMode::Execute && composed.get("activity").is_some() {
        let facet = composed
            .get("facet")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                PrepareFailure::Refusal("activity destination facet is missing".to_owned())
            })?;
        let request_id = composed.get("destination_id");
        let record_id = composed
            .get("activity")
            .and_then(|value| value.get("destination_id"));
        if request_id.is_some() && record_id.is_some() && request_id != record_id {
            return Err(PrepareFailure::Refusal(
                "activity destination identities disagree".to_owned(),
            ));
        }
        let binding = request_id.or(record_id);
        let destination_id = if let Some(binding) = binding {
            let expected = binding
                .as_str()
                .filter(|id| !id.is_empty())
                .ok_or_else(|| {
                    PrepareFailure::Refusal("activity destination identity is invalid".to_owned())
                })?;
            let _guard =
                solstone_core_facets::hold_activity_enrichment(&context.journal, facet, expected)
                    .map_err(|error| PrepareFailure::Refusal(error.to_string()))?;
            expected.to_owned()
        } else {
            let id = solstone_core_facets::admit_activity_destination(&context.journal, facet)
                .map_err(|error| PrepareFailure::Refusal(error.to_string()))?;
            let _guard =
                solstone_core_facets::hold_activity_enrichment(&context.journal, facet, &id)
                    .map_err(|error| PrepareFailure::Refusal(error.to_string()))?;
            id
        };
        composed.insert("destination_id".to_owned(), Value::String(destination_id));
    }
    mobile_local_timeout(&composed, &name)?;
    let (provider, model) = configured_brain(&context.journal);
    composed.insert("provider".to_owned(), Value::String(provider));
    composed.insert("model".to_owned(), Value::String(model));
    if mode == PrepareMode::Execute
        && composed.get("provider").and_then(Value::as_str) == Some("none")
    {
        return Err(PrepareFailure::NoBrainConfigured);
    }
    if composed.get("disabled").is_some_and(is_truthy) {
        composed.insert(
            "skip_reason".to_owned(),
            Value::String("disabled".to_owned()),
        );
        return Ok(PreparedTalent::new(name, composed));
    }
    let sources = composed.get("sources").and_then(Value::as_object).cloned();
    let mut entry_sources = Vec::new();
    if composed
        .get("day")
        .and_then(Value::as_str)
        .is_some_and(|day| !day.is_empty())
        && sources
            .as_ref()
            .is_some_and(transcript::sources_are_enabled)
    {
        let loaded = transcript::load_transcript(&context.journal, &composed)
            .map_err(PrepareFailure::Refusal)?;
        if loaded.memory_incomplete {
            return Err(PrepareFailure::Refusal(
                "private memory context is incomplete".to_owned(),
            ));
        }
        composed.insert(
            "_memory_sources".to_owned(),
            serde_json::to_value(&loaded.memory_sources).unwrap_or(Value::Array(Vec::new())),
        );
        let mut transcript = loaded.text;
        let counts = loaded.counts;
        entry_sources = loaded.sources;
        let mut screen_cuts = loaded.screen_cuts;
        composed.insert("transcript".to_owned(), Value::String(transcript.clone()));
        composed.insert("source_counts".to_owned(), Value::from(counts));

        let mut source_entries = sources
            .as_ref()
            .expect("enabled sources are present")
            .iter()
            .collect::<Vec<_>>();
        source_entries.sort_by_key(|(source, _)| *source);
        for (source, value) in source_entries {
            if solstone_core_talent_config::source_is_required(value)
                && source_count(&counts, source) == 0
            {
                composed.insert(
                    "skip_reason".to_owned(),
                    Value::String(format!("missing_required_{source}")),
                );
                return Ok(PreparedTalent::new(name, composed));
            }
        }
        if solstone_core_transcripts::is_no_input(&transcript, &counts) {
            composed.insert(
                "skip_reason".to_owned(),
                Value::String("no_input".to_owned()),
            );
            return Ok(PreparedTalent::new(name, composed));
        }
        if counts.total() < 3 {
            transcript.insert_str(0, SPARSE_INPUT_NOTE);
            for cut in &mut screen_cuts {
                cut.byte_offset = cut.byte_offset.saturating_add(SPARSE_INPUT_NOTE.len());
                cut.observation_byte_offset = cut
                    .observation_byte_offset
                    .saturating_add(SPARSE_INPUT_NOTE.len());
            }
            composed.insert("transcript".to_owned(), Value::String(transcript));
        }
        if name == "screen" && !screen_cuts.is_empty() {
            composed.insert(
                "_screen_batch_cuts".to_owned(),
                Value::Array(
                    screen_cuts
                        .into_iter()
                        .map(|cut| {
                            serde_json::json!({
                                "byte_offset": cut.byte_offset,
                                "observation_byte_offset": cut.observation_byte_offset,
                                "reset_carry": cut.reset_carry,
                            })
                        })
                        .collect(),
                ),
            );
        }
    }
    let import_facet = (name == "sense")
        .then(|| crate::import_facet::chosen(&entry_sources, &context.journal))
        .flatten();
    if composed
        .get("day")
        .and_then(Value::as_str)
        .is_some_and(|day| !day.is_empty())
    {
        let configured_stream = composed
            .get("stream")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty());
        let environment_stream = std::env::var("SOL_STREAM").ok();
        let stream_name =
            configured_stream.or(environment_stream.as_deref().filter(|s| !s.is_empty()));
        let binding = stream_name.and_then(|name| {
            if name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-')
                && !name.contains("..")
                && name != "."
            {
                let stream_path = context.journal.join("streams").join(format!("{name}.json"));
                fs::read(stream_path)
                    .ok()
                    .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
                    .and_then(|v| match v {
                        Value::Object(map) => Some(map),
                        _ => None,
                    })
            } else {
                None
            }
        });
        let prompt_context =
            crate::prompt_context::build(&context.journal, &composed, binding.as_ref());
        let focused_facet = composed
            .get("facet")
            .and_then(Value::as_str)
            .or(import_facet.as_deref());
        let instruction = solstone_core_talent_cli::compose_talent_instruction(
            &config,
            &context.journal,
            &paths.templates_dir,
            focused_facet,
            &prompt_context,
        )
        .map_err(PrepareFailure::Refusal)?;
        // Conversation reads who spoke as a counted fact, not an inference:
        // the owner is usually a minor speaker in their own conversations.
        let instruction = match (name == "conversation")
            .then(|| composed.get("transcript").and_then(Value::as_str))
            .flatten()
            .and_then(|transcript| crate::story_input::speaker_count(transcript, &entry_sources))
        {
            Some(count) => format!("{count}\n\n{instruction}"),
            None => instruction,
        };
        let instruction = match composed
            .get("activity")
            .and_then(Value::as_object)
            .and_then(crate::prompt_context::correction_last)
        {
            Some(correction) => format!("{instruction}\n\n{correction}"),
            None => instruction,
        };
        let story_talent = composed
            .get("hook")
            .and_then(|hook| hook.get("post"))
            .and_then(Value::as_str)
            == Some("story");
        let short_account_stands = composed
            .get("activity")
            .and_then(Value::as_object)
            .and_then(crate::prompt_context::owner_account)
            .is_some_and(|account| account.len() <= crate::story::REPEATED_ACCOUNT_MAX_BYTES);
        let instruction = match composed
            .get_mut("json_schema")
            .and_then(Value::as_object_mut)
            .filter(|_| story_talent && short_account_stands)
        {
            Some(schema) => {
                crate::story::repeat_account_first(schema);
                format!(
                    "{instruction}\n\nStart your JSON with the field `{}`: copy those words exactly.",
                    crate::story::REPEATED_ACCOUNT
                )
            }
            None => instruction,
        };
        composed.insert("user_instruction".to_owned(), Value::String(instruction));
    }
    if let Some(output) = composed.get("output").and_then(Value::as_str)
        && let Some(day) = composed.get("day").and_then(Value::as_str)
    {
        let output_path = composed
            .get("output_path")
            .and_then(Value::as_str)
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                let environment_stream = std::env::var("SOL_STREAM").ok();
                solstone_core_talent_config::get_output_path(
                    &context.journal.join("chronicle").join(day),
                    &name,
                    composed.get("segment").and_then(Value::as_str),
                    Some(output),
                    composed.get("facet").and_then(Value::as_str),
                    composed
                        .get("stream")
                        .and_then(Value::as_str)
                        .filter(|stream| !stream.is_empty())
                        .or(environment_stream
                            .as_deref()
                            .filter(|stream| !stream.is_empty())),
                )
            });
        composed.insert(
            "output_path".to_owned(),
            Value::String(output_path.display().to_string()),
        );
    }
    validate_config(&composed).map_err(PrepareFailure::Refusal)?;
    // A request that replaces the output schema the model reads asks for a
    // body the talent's own source-word body was never measured against.
    if mode == PrepareMode::Execute
        && composed.get("json_schema") == talent_schema.as_ref()
        && let Some(body) = crate::story::source_body(&name, &composed, &entry_sources)
    {
        composed.insert(
            crate::story::SOURCE_BODY_KEY.to_owned(),
            Value::String(body),
        );
    }
    if let Some(facet) = &import_facet {
        crate::import_facet::narrow_schema(&mut composed, facet);
    }
    let participation_floor = if composed
        .get("sources")
        .and_then(Value::as_object)
        .and_then(|sources| sources.get("sample_to_floor"))
        .and_then(Value::as_bool)
        == Some(true)
        && composed.get("messages").is_none()
    {
        Some(
            crate::participation_floor::apply(&mut composed, &entry_sources, &context.journal)
                .map_err(PrepareFailure::Refusal)?,
        )
    } else {
        None
    };
    crate::product_names::prepare(&context.journal, &name, &mut composed, &entry_sources);
    let mut prepared = PreparedTalent::new(name, composed);
    prepared.participation_floor = participation_floor;
    // Work's message notes read the raw request, before any floor packet.
    if prepared.name == "work" {
        prepared.entry_sources = std::mem::take(&mut entry_sources);
        prepared.story_messages =
            crate::story_messages::Candidate::prepare(&prepared.entry_sources, &prepared.config);
    }
    if matches!(prepared.name.as_str(), "work" | "conversation") {
        prepared.story_input = crate::story_input::prepare(&context.journal, &prepared.config)
            .map_err(PrepareFailure::Refusal)?;
        // Over the floor, the fitted packet is the input on every backend, so
        // a story never depends on the window that happens to serve it.
        if let Some(candidate) = &prepared.story_input
            && crate::story_input::over_floor(&prepared.config)
        {
            prepared.config.insert(
                "transcript".to_owned(),
                Value::String(candidate.compact_transcript.clone()),
            );
            match &candidate.product_spelling_view {
                Some(view) => {
                    prepared
                        .config
                        .insert(crate::product_names::VIEW_KEY.to_owned(), view.clone());
                }
                None => {
                    prepared.config.remove(crate::product_names::VIEW_KEY);
                }
            }
            prepared.story_input_fitted = true;
        }
    }
    if matches!(prepared.name.as_str(), "documents" | "sense")
        && prepared.config.get("stream").and_then(Value::as_str) == Some("import.document")
    {
        prepared.floor_reading = crate::documents::floor_reading(&prepared.config);
    }
    if matches!(prepared.name.as_str(), "sense" | "screen") {
        prepared.entry_sources = entry_sources;
        let report = crate::terminal_reading::prepare(&mut prepared, context);
        prepared.terminal_reading = report;
        if prepared.terminal_reading.is_none() && prepared.floor_reading.is_none() {
            prepared.floor_reading =
                crate::browser_reading::floor_reading(&prepared.config, &context.journal);
        }
    }
    Ok(prepared)
}

/// Resolve owner overrides and definition-level refusals without composing a
/// prompt or reading facet/segment source context.
pub(crate) fn resolve_talent_config(
    name: &str,
    paths: &RuntimePaths,
    context: &ExecutionContext,
) -> Result<TalentConfig, PrepareFailure> {
    let configs = merged_talent_configs(paths, context)?;
    select_talent_config(configs, name)
}

/// Activity preview admits talents through the same whole-corpus validation
/// gate production activity dispatch uses, without changing execute semantics.
pub(crate) fn resolve_validated_talent_config(
    name: &str,
    paths: &RuntimePaths,
    context: &ExecutionContext,
) -> Result<TalentConfig, PrepareFailure> {
    let mut configs = merged_talent_configs(paths, context)?;
    validate(&mut configs).map_err(PrepareFailure::Refusal)?;
    select_talent_config(configs, name)
}

fn merged_talent_configs(
    paths: &RuntimePaths,
    context: &ExecutionContext,
) -> Result<Vec<TalentConfig>, PrepareFailure> {
    let mut configs =
        discover(&paths.talent_root, &paths.apps_root).map_err(PrepareFailure::Refusal)?;
    let overrides = talent_overrides(&context.journal);
    merge(&mut configs, overrides.as_ref());
    Ok(configs)
}

fn select_talent_config(
    configs: Vec<TalentConfig>,
    name: &str,
) -> Result<TalentConfig, PrepareFailure> {
    let config = configs
        .into_iter()
        .find(|config| config.key == name)
        .ok_or_else(|| PrepareFailure::Refusal(format!("talent '{name}' not found")))?;
    reject_definition_fields(&config, name)?;
    Ok(config)
}

fn source_count(counts: &solstone_core_transcripts::SourceCounts, source: &str) -> usize {
    match source {
        "transcripts" => counts.transcripts,
        "percepts" => counts.percepts,
        "talents" => counts.talents,
        _ => 0,
    }
}

/// The provider and the model a run's records name. On a local endpoint
/// generation sends the endpoint's served model, never the configured active
/// model, so the records name the served one.
fn configured_brain(journal: &Path) -> (String, String) {
    let configured = read_journal_config(journal);
    let active = configured
        .as_ref()
        .and_then(|value| value.pointer("/providers/active"));
    let provider = active
        .and_then(|value| value.get("provider"))
        .and_then(Value::as_str)
        .unwrap_or("none")
        .to_owned();
    let served = configured
        .as_ref()
        .and_then(Value::as_object)
        .filter(|_| provider == "local")
        .and_then(
            |config| match solstone_core_local::resolve_local_endpoint(config) {
                solstone_core_local::LocalEndpointResolution::Byo(endpoint) => {
                    Some(endpoint.served_model_id)
                }
                solstone_core_local::LocalEndpointResolution::Bundled => None,
            },
        );
    let model = served.unwrap_or_else(|| {
        active
            .and_then(|value| value.get("model"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned()
    });
    (provider, model)
}

pub(crate) fn mobile_local_timeout(
    composed: &Map<String, Value>,
    name: &str,
) -> Result<Option<std::time::Duration>, PrepareFailure> {
    let Some(value) = composed.get("mobile_local_timeout_s") else {
        return Ok(None);
    };
    let duration = value
        .as_f64()
        .filter(|seconds| *seconds > 0.0)
        .and_then(|seconds| std::time::Duration::try_from_secs_f64(seconds).ok())
        .filter(|duration| !duration.is_zero())
        .ok_or_else(|| {
            PrepareFailure::Refusal(format!(
                "configuration '{name}' declares an invalid mobile_local_timeout_s"
            ))
        })?;
    Ok(Some(duration))
}

fn talent_overrides(journal: &Path) -> Option<Map<String, Value>> {
    read_journal_config(journal)
        .and_then(|configured| configured.get("talent_overrides").cloned())
        .and_then(|overrides| overrides.as_object().cloned())
}

fn read_journal_config(journal: &Path) -> Option<Value> {
    std::fs::read_to_string(journal.join("config/journal.json"))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
}

fn reject_definition_fields(config: &TalentConfig, name: &str) -> Result<(), PrepareFailure> {
    if config.metadata.contains_key("outbound_approval") {
        return Err(PrepareFailure::Refusal(format!(
            "talent '{name}' declares 'outbound_approval' in frontmatter; this field is launch-config-only and may not come from a talent definition"
        )));
    }
    for field in ["provider", "model"] {
        if config.metadata.contains_key(field) {
            return Err(PrepareFailure::Refusal(format!(
                "talent '{name}' declares '{field}' in frontmatter; thinking provider and model are configured only in Thinking"
            )));
        }
    }
    Ok(())
}

fn reject_request_fields(
    config: &TalentConfig,
    request: &Map<String, Value>,
    name: &str,
) -> Result<(), PrepareFailure> {
    for field in ["provider", "model"] {
        if request.get(field).is_some_and(|value| !value.is_null()) {
            return Err(PrepareFailure::Refusal(format!(
                "request overrides for '{field}' are not allowed; thinking provider and model are configured only in Thinking"
            )));
        }
    }
    // A talent's budgets and sampling are its own; a request can neither raise nor
    // lower them.
    for field in [
        "max_output_tokens",
        "timeout_s",
        "mobile_local_timeout_s",
        "thinking_budget",
        "context_window",
        "temperature",
    ] {
        if request.get(field).is_some_and(|value| !value.is_null()) {
            return Err(PrepareFailure::Refusal(format!(
                "request overrides for '{field}' are not allowed; a talent's budgets are its own"
            )));
        }
    }
    let equal_or_refuse = |field: &str, declared: Option<&Value>| -> Result<(), PrepareFailure> {
        let Some(requested) = request.get(field).filter(|value| !value.is_null()) else {
            return Ok(());
        };
        if declared != Some(requested) {
            return Err(PrepareFailure::Refusal(format!(
                "Request overrides '{field}' for talent '{name}' are not allowed ({} != {})",
                python_repr(declared),
                python_repr(Some(requested)),
            )));
        }
        Ok(())
    };
    for field in ["cwd", "access_tier"] {
        if request.get(field).is_some_and(|value| !value.is_null()) {
            return Err(PrepareFailure::Refusal(format!(
                "request field '{field}' is retired"
            )));
        }
    }
    equal_or_refuse("type", config.metadata.get("type"))
}

fn python_repr(value: Option<&Value>) -> String {
    match value {
        None | Some(Value::Null) => "None".to_owned(),
        Some(Value::String(value)) => format!("'{value}'"),
        Some(Value::Bool(true)) => "True".to_owned(),
        Some(Value::Bool(false)) => "False".to_owned(),
        Some(value) => value.to_string(),
    }
}

pub fn validate_config(config: &Map<String, Value>) -> Result<(), String> {
    let kind = config
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let prompt = config
        .get("prompt")
        .and_then(Value::as_str)
        .is_some_and(|value| !value.is_empty());
    let instruction = config
        .get("user_instruction")
        .and_then(Value::as_str)
        .is_some_and(|value| !value.is_empty());
    let day = config
        .get("day")
        .and_then(Value::as_str)
        .is_some_and(|value| !value.is_empty());
    if !kind.is_empty() && kind != "generate" {
        return Err(format!("invalid talent type '{kind}'"));
    }
    if !day && !instruction && !prompt {
        return Err("Invalid config: must have 'type', 'day', or 'prompt'".to_owned());
    }
    if (config.get("segment").is_some_and(is_truthy) || config.get("span").is_some_and(is_truthy))
        && !day
    {
        return Err("Invalid config: 'segment' or 'span' requires 'day'".to_owned());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use serde_json::json;
    #[cfg(feature = "full-tests")]
    use solstone_core_format::agent_memory::SourceKey;

    use super::*;

    #[cfg(feature = "full-tests")]
    fn test_scratch_root() -> std::path::PathBuf {
        if cfg!(windows) {
            std::env::temp_dir()
        } else {
            std::path::PathBuf::from("/var/tmp")
        }
    }

    #[test]
    fn criterion_3_and_25_request_refusals_preserve_equal_echoes() {
        let config = TalentConfig {
            key: "demo".to_owned(),
            file: String::new(),
            body: String::new(),
            metadata: Map::from_iter([
                ("type".to_owned(), json!("generate")),
                ("max_output_tokens".to_owned(), json!(1024)),
            ]),
        };
        let echo = Map::from_iter([("type".to_owned(), json!("generate"))]);
        assert!(reject_request_fields(&config, &echo, "demo").is_ok());
        for field in ["provider", "model"] {
            assert_eq!(
                reject_request_fields(
                    &config,
                    &Map::from_iter([(field.to_owned(), json!("x"))]),
                    "demo"
                )
                .unwrap_err()
                .to_string(),
                format!(
                    "request overrides for '{field}' are not allowed; thinking provider and model are configured only in Thinking"
                )
            );
        }
        assert_eq!(
            reject_request_fields(
                &config,
                &Map::from_iter([("cwd".to_owned(), json!("other"))]),
                "demo"
            )
            .unwrap_err()
            .to_string(),
            "request field 'cwd' is retired"
        );
        assert_eq!(
            reject_request_fields(
                &config,
                &Map::from_iter([("type".to_owned(), json!("cogitate"))]),
                "demo"
            )
            .unwrap_err()
            .to_string(),
            "Request overrides 'type' for talent 'demo' are not allowed ('generate' != 'cogitate')"
        );
        assert_eq!(
            reject_request_fields(
                &config,
                &Map::from_iter([("access_tier".to_owned(), json!("full"))]),
                "demo"
            )
            .unwrap_err()
            .to_string(),
            "request field 'access_tier' is retired"
        );
        for (field, message) in [
            (
                "outbound_approval",
                "talent 'demo' declares 'outbound_approval' in frontmatter; this field is launch-config-only and may not come from a talent definition",
            ),
            (
                "provider",
                "talent 'demo' declares 'provider' in frontmatter; thinking provider and model are configured only in Thinking",
            ),
            (
                "model",
                "talent 'demo' declares 'model' in frontmatter; thinking provider and model are configured only in Thinking",
            ),
        ] {
            let definition = TalentConfig {
                key: "demo".to_owned(),
                file: String::new(),
                body: String::new(),
                metadata: Map::from_iter([(field.to_owned(), json!(true))]),
            };
            assert_eq!(
                reject_definition_fields(&definition, "demo")
                    .unwrap_err()
                    .to_string(),
                message
            );
        }
    }

    #[test]
    fn request_timeout_cannot_override_the_talent_budget() {
        let config = TalentConfig {
            key: "declared".to_owned(),
            file: String::new(),
            body: String::new(),
            metadata: Map::from_iter([
                ("timeout_s".to_owned(), json!(480)),
                ("mobile_local_timeout_s".to_owned(), json!(1200)),
            ]),
        };
        for field in ["timeout_s", "mobile_local_timeout_s"] {
            for timeout in [1, 480, 1200, 1800] {
                let request = Map::from_iter([(field.to_owned(), json!(timeout))]);
                assert!(reject_request_fields(&config, &request, "declared").is_err());
            }
        }
        assert!(reject_request_fields(&config, &Map::new(), "declared").is_ok());
        let mut configs = vec![config];
        merge(
            &mut configs,
            Some(
                json!({"talent.system.declared": {
                    "timeout_s": 1, "mobile_local_timeout_s": 1
                }})
                .as_object()
                .unwrap(),
            ),
        );
        assert_eq!(configs[0].metadata["timeout_s"], 480);
        assert_eq!(configs[0].metadata["mobile_local_timeout_s"], 1200);
    }

    #[test]
    fn mobile_timeout_preserves_the_base_request_on_every_preparation_lane() {
        for provider in ["local", "google", "openai", "anthropic"] {
            for original in [None, Some(json!(480.5))] {
                let mut config = json!({
                    "provider": provider,
                    "max_output_tokens": 512,
                    "prompt": "Read the supplied source.",
                    "mobile_local_timeout_s": 1200.5
                })
                .as_object()
                .unwrap()
                .clone();
                if let Some(original) = &original {
                    config.insert("timeout_s".into(), original.clone());
                }
                assert_eq!(
                    mobile_local_timeout(&config, "declared").unwrap(),
                    Some(std::time::Duration::from_secs_f64(1200.5))
                );
                let request =
                    crate::native_generate_request(&PreparedTalent::new("declared".into(), config))
                        .unwrap();
                assert_eq!(request.timeout_s, original.as_ref().and_then(Value::as_f64));
            }
        }
        assert_eq!(
            mobile_local_timeout(&Map::new(), "undeclared").unwrap(),
            None
        );
    }

    #[test]
    fn mobile_timeout_refuses_invalid_declarations() {
        for value in [
            Value::Null,
            json!("1200"),
            json!(true),
            json!([]),
            json!({}),
            json!(0),
            json!(-1),
            json!(1e30),
            json!(1e-30),
        ] {
            let config = Map::from_iter([("mobile_local_timeout_s".into(), value)]);
            assert!(mobile_local_timeout(&config, "invalid").is_err());
        }
    }

    #[test]
    fn criterion_4_validation_messages_are_verbatim() {
        assert_eq!(
            validate_config(&Map::from_iter([
                ("type".to_owned(), json!("cogitate")),
                ("max_output_tokens".to_owned(), json!(1024))
            ]))
            .unwrap_err(),
            "invalid talent type 'cogitate'"
        );
        assert_eq!(
            validate_config(&Map::new()).unwrap_err(),
            "Invalid config: must have 'type', 'day', or 'prompt'"
        );
        assert_eq!(
            validate_config(&Map::from_iter([
                ("type".to_owned(), json!("generate")),
                ("max_output_tokens".to_owned(), json!(1024)),
            ]))
            .unwrap_err(),
            "Invalid config: must have 'type', 'day', or 'prompt'"
        );
        assert_eq!(
            validate_config(&Map::from_iter([
                ("type".to_owned(), json!("generate")),
                ("max_output_tokens".to_owned(), json!(1024)),
                ("segment".to_owned(), json!("x")),
                ("prompt".to_owned(), json!("x"))
            ]))
            .unwrap_err(),
            "Invalid config: 'segment' or 'span' requires 'day'"
        );
    }

    #[test]
    fn run_records_name_the_model_generation_sends() {
        let journal = tempfile::tempdir().unwrap();
        fs::create_dir_all(journal.path().join("config")).unwrap();
        let endpoint = r#""local":{"endpoint_url":"https://endpoint.invalid/v1","served_model_id":"served-model"}"#;
        for (config, expected) in [
            (
                r#"{"providers":{"active":{"provider":"local","model":"local/qwen3.5-4b"}}}"#
                    .to_owned(),
                "local/qwen3.5-4b",
            ),
            (
                format!(
                    r#"{{"providers":{{"active":{{"provider":"local","model":"local/qwen3.5-4b"}},{endpoint}}}}}"#
                ),
                "served-model",
            ),
            (
                format!(
                    r#"{{"providers":{{"active":{{"provider":"local","model":"local/qwen3.5-4b"}},{endpoint}}},"services":{{"confidential":{{}}}}}}"#
                ),
                "served-model",
            ),
            (
                format!(
                    r#"{{"providers":{{"active":{{"provider":"google","model":"gemini-model"}},{endpoint}}}}}"#
                ),
                "gemini-model",
            ),
        ] {
            fs::write(journal.path().join("config").join("journal.json"), &config).unwrap();
            assert_eq!(configured_brain(journal.path()).1, expected, "{config}");
        }
    }

    #[test]
    fn activity_preparation_preserves_record_identity_and_rejects_conflicting_bindings() {
        let root = tempfile::tempdir().unwrap();
        let paths = RuntimePaths {
            talent_root: root.path().join("talent"),
            apps_root: root.path().join("apps"),
            templates_dir: root.path().join("templates"),
        };
        let context = ExecutionContext {
            journal: root.path().join("journal"),
        };
        for path in [&paths.talent_root, &paths.apps_root, &paths.templates_dir] {
            fs::create_dir_all(path).unwrap();
        }
        fs::create_dir_all(context.journal.join("config")).unwrap();
        fs::write(
            context.journal.join("config").join("journal.json"),
            r#"{"providers":{"active":{"provider":"test","model":"test-model"}}}"#,
        )
        .unwrap();
        fs::write(
            paths.talent_root.join("probe.md"),
            "{\n\"type\":\"generate\",\"max_output_tokens\":1024\n}\nbody",
        )
        .unwrap();
        solstone_core_facets::create_facet(&context.journal, "work", "Work", "", "", "", None)
            .unwrap();
        let id =
            solstone_core_facets::observe_facet_write_identity(&context.journal, "work").unwrap();
        let request = json!({"name":"probe","day":"20260101","facet":"work","activity":{"id":"a","destination_id":id,"segments":[]}}).as_object().unwrap().clone();
        let prepared = prepare(request.clone(), &paths, &context, PrepareMode::Execute).unwrap();
        assert_eq!(prepared.config["destination_id"], id);
        let mut conflicting = request.clone();
        conflicting.insert(
            "destination_id".to_owned(),
            json!("00000000-0000-4000-8000-000000000099"),
        );
        assert!(
            prepare(conflicting, &paths, &context, PrepareMode::Execute)
                .unwrap_err()
                .to_string()
                .contains("disagree")
        );
        // A journal keeps one enabled facet; the sibling lets "work" go.
        let _ = solstone_core_facets::create_facet(
            &context.journal,
            "personal",
            "Personal",
            "",
            "",
            "",
            None,
        );
        solstone_core_facets::delete_facet(
            &solstone_core_removals::Removal::begin(
                &context.journal,
                solstone_core_removals::Action::FacetDelete,
                solstone_core_removals::Surface::Cli,
            ),
            "work",
        )
        .unwrap();
        // These tests recreate "work" to prove the fences against a replaced facet;
        // the store never reuses a name, so the retired record is dropped as a hand edit would.
        let _ = std::fs::remove_file(
            std::path::Path::new(&context.journal)
                .join("facets")
                .join("retired.json"),
        );
        solstone_core_facets::create_facet(
            &context.journal,
            "work",
            "Replacement",
            "",
            "",
            "",
            None,
        )
        .unwrap();
        assert!(
            prepare(request, &paths, &context, PrepareMode::Execute)
                .unwrap_err()
                .to_string()
                .contains("replaced")
        );
    }

    #[test]
    fn preview_mode_skips_only_the_no_brain_check() {
        let root = tempfile::tempdir().unwrap();
        let talent_root = root.path().join("talent");
        std::fs::create_dir_all(&talent_root).unwrap();
        std::fs::create_dir_all(root.path().join("apps")).unwrap();
        std::fs::create_dir_all(root.path().join("templates")).unwrap();
        std::fs::write(
            talent_root.join("demo.md"),
            "{\n\"type\": \"generate\", \"max_output_tokens\": 1024\n}\nbody",
        )
        .unwrap();
        let journal = root.path().join("journal");
        std::fs::create_dir_all(journal.join("config")).unwrap();
        std::fs::write(
            journal.join("config").join("journal.json"),
            r#"{"providers":{"active":{"provider":"none"}}}"#,
        )
        .unwrap();
        let paths = RuntimePaths {
            talent_root,
            apps_root: root.path().join("apps"),
            templates_dir: root.path().join("templates"),
        };
        let context = ExecutionContext { journal };
        let request = Map::from_iter([
            ("name".to_owned(), json!("demo")),
            ("prompt".to_owned(), json!("hello")),
        ]);
        assert!(matches!(
            prepare(request.clone(), &paths, &context, PrepareMode::Execute),
            Err(PrepareFailure::NoBrainConfigured)
        ));
        let prepared = prepare(request, &paths, &context, PrepareMode::Preview)
            .expect("preview mode must skip the no-brain check");
        assert_eq!(prepared.config["provider"], "none");
    }

    #[cfg(feature = "full-tests")]
    #[test]
    fn corrupt_memory_in_a_mixed_segment_refuses_preparation_without_output() {
        let root = tempfile::Builder::new()
            .prefix("solstone-memory-prepare-refusal-")
            .tempdir_in(test_scratch_root())
            .unwrap();
        let talent_root = root.path().join("talent");
        let apps_root = root.path().join("apps");
        let templates_dir = root.path().join("templates");
        let journal = root.path().join("journal");
        let day = "20260102";
        for directory in [
            talent_root.clone(),
            apps_root.clone(),
            templates_dir.clone(),
            journal.join("config"),
        ] {
            fs::create_dir_all(directory).unwrap();
        }
        fs::write(
            journal.join("config").join("journal.json"),
            r#"{"providers":{"active":{"provider":"none"}}}"#,
        )
        .unwrap();
        fs::write(
            talent_root.join("probe.md"),
            "{\n\"type\":\"generate\",\"max_output_tokens\":1024,\"load\":{\"transcripts\":true},\"output\":\"json\"\n}\nAnalyze the input.",
        )
        .unwrap();

        let source = SourceKey::from_verified_id("corrupt-memory-source");
        let stream = format!("agent-memory-{}", source.component());
        let segment = journal
            .join("chronicle")
            .join(day)
            .join(&stream)
            .join("090000_60");
        fs::create_dir_all(&segment).unwrap();
        fs::write(segment.join("note.txt"), b"not a validated original").unwrap();
        fs::write(segment.join("ready.json"), b"not json").unwrap();
        fs::write(
            segment.join("stream.json"),
            serde_json::json!({
                "stream": stream,
                "prev_day": null,
                "prev_segment": null,
                "seq": 1
            })
            .to_string(),
        )
        .unwrap();
        fs::write(
            segment.join("audio.jsonl"),
            r#"{"start":"00:00:00","text":"readable ordinary transcript"}"#,
        )
        .unwrap();

        let expected_output = journal
            .join("chronicle")
            .join(day)
            .join("talents")
            .join("probe.json");
        let result = prepare(
            json!({"name":"probe","day":day})
                .as_object()
                .unwrap()
                .clone(),
            &RuntimePaths {
                talent_root,
                apps_root,
                templates_dir,
            },
            &ExecutionContext {
                journal: journal.clone(),
            },
            PrepareMode::Preview,
        );
        assert!(matches!(result, Err(PrepareFailure::Refusal(_))));
        assert!(!expected_output.exists());
    }

    #[test]
    fn day_segment_request_renders_the_shipped_preamble_context() {
        let root = tempfile::tempdir().expect("root");
        let talent_root = root.path().join("talent");
        let apps_root = root.path().join("apps");
        let templates_dir = root.path().join("templates");
        let journal = root.path().join("journal");
        for directory in [
            talent_root.clone(),
            apps_root.clone(),
            templates_dir.clone(),
            journal.join("config"),
        ] {
            fs::create_dir_all(directory).expect("fixture directory");
        }
        fs::write(
            journal.join("config").join("journal.json"),
            r#"{"identity":{"preferred":"Soleil"},"providers":{"active":{"provider":"none"}}}"#,
        )
        .expect("journal config");
        fs::write(
            templates_dir.join("segment_preamble.md"),
            "$preferred|$day|$day_YYYYMMDD|$segment_start|$segment_end|$content_description",
        )
        .expect("segment template");
        fs::write(
            talent_root.join("screen.md"),
            "{\n\"type\":\"generate\",\"max_output_tokens\":1024,\"output\":\"json\"\n}\n$segment_preamble",
        )
        .expect("talent");
        let expected_output = journal
            .join("chronicle")
            .join("20260101")
            .join("import.obsidian")
            .join("090000_60")
            .join("talents")
            .join("screen.json");
        let prepared = prepare(
            json!({
                "name":"screen",
                "day":"20260101",
                "segment":"090000_60",
                "stream":"import.obsidian"
            })
            .as_object()
            .expect("request object")
            .clone(),
            &RuntimePaths {
                talent_root,
                apps_root,
                templates_dir,
            },
            &ExecutionContext { journal },
            PrepareMode::Preview,
        )
        .expect("prepare");
        assert_eq!(
            prepared.config["output_path"].as_str(),
            expected_output.to_str()
        );
        assert_eq!(
            prepared.config["user_instruction"],
            "Soleil|Thursday, January 01, 2026|20260101|9:00 AM|9:01 AM|an imported note from Obsidian"
        );
    }

    /// Render an activity talent's instruction for a two-segment `work`
    /// activity, after `seed` has written whatever the segments hold.
    fn render_activity_instruction(seed: impl FnOnce(&Path)) -> String {
        let root = tempfile::tempdir().expect("root");
        let talent_root = root.path().join("talent");
        let apps_root = root.path().join("apps");
        let templates_dir = root.path().join("templates");
        let journal = root.path().join("journal");
        for directory in [
            talent_root.clone(),
            apps_root.clone(),
            templates_dir.clone(),
            journal.join("config"),
            journal.join("facets").join("work"),
            journal
                .join("chronicle")
                .join("20260101")
                .join("090000_60")
                .join("talents"),
            journal
                .join("chronicle")
                .join("20260101")
                .join("090200_120")
                .join("talents"),
        ] {
            fs::create_dir_all(directory).expect("fixture directory");
        }
        fs::write(
            journal.join("config").join("journal.json"),
            r#"{"identity":{"preferred":"Soleil"},"providers":{"active":{"provider":"none"}}}"#,
        )
        .expect("journal config");
        fs::write(
            journal.join("facets").join("work").join("facet.json"),
            r#"{"name":"work","description":"Work"}"#,
        )
        .expect("facet");
        seed(&journal);
        fs::write(
            templates_dir.join("activity_preamble.md"),
            "You are analyzing a **$activity_type** activity from $preferred's journal on **$day** ($day_YYYYMMDD), covering **$segment_start to $segment_end** (~$activity_duration minutes).\n\n**Entities involved:** $activity_entities",
        )
        .expect("activity template");
        fs::write(
            talent_root.join("work.md"),
            "{\n\"type\":\"generate\",\"max_output_tokens\":1024,\"schedule\":\"activity\",\"activities\":[\"coding\"]\n}\n$activity_context\n\n$activity_preamble",
        )
        .expect("talent");
        let prepared = prepare(
            json!({
                "name":"work",
                "day":"20260101",
                "facet":"work",
                "span":["090000_60", "090200_120"],
                "activity":{
                    "id":"coding_090000_60",
                    "activity":"coding",
                    "description":"Release work",
                    "level_avg":0.8,
                    "active_entities":["Mina", "Ravi"],
                    "segments":["090000_60", "090200_120"]
                }
            })
            .as_object()
            .expect("request object")
            .clone(),
            &RuntimePaths {
                talent_root,
                apps_root,
                templates_dir,
            },
            &ExecutionContext { journal },
            PrepareMode::Preview,
        )
        .expect("prepare");
        let instruction = prepared.config["user_instruction"]
            .as_str()
            .expect("instruction")
            .to_owned();
        assert!(!instruction.contains("$activity_"));
        assert!(!instruction.contains("$segment_"));
        instruction
    }

    /// The instruction a shipped activity talent renders for `activity`.
    fn render_shipped_talent(talent: &str, activity: Value) -> String {
        prepare_shipped_talent(talent, activity).config["user_instruction"]
            .as_str()
            .expect("instruction")
            .to_owned()
    }

    fn prepare_shipped_talent(talent: &str, activity: Value) -> PreparedTalent {
        let root = tempfile::tempdir().expect("root");
        let payload = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../payload/solstone");
        let talent_root = root.path().join("talent");
        let journal = root.path().join("journal");
        for directory in [
            talent_root.clone(),
            root.path().join("apps"),
            journal.join("config"),
            journal.join("facets").join("work"),
            journal.join("chronicle").join("20260101").join("090000_60"),
        ] {
            fs::create_dir_all(directory).expect("fixture directory");
        }
        fs::write(
            journal.join("config").join("journal.json"),
            r#"{"identity":{"preferred":"Soleil"},"providers":{"active":{"provider":"none"}}}"#,
        )
        .expect("journal config");
        fs::write(
            journal.join("facets").join("work").join("facet.json"),
            r#"{"name":"work","description":"Work"}"#,
        )
        .expect("facet");
        // Something said in the segment, so the talent has input to run on.
        let mut audio = json!({"raw":"fixture"}).to_string() + "\n";
        for text in ["Let's ship it.", "The release is ready.", "Tag it now."] {
            audio += &(json!({"start":"00:00:01","text":text}).to_string() + "\n");
        }
        fs::write(
            journal.join("chronicle/20260101/090000_60/audio.jsonl"),
            audio,
        )
        .expect("audio");
        for entry in fs::read_dir(payload.join("talent")).expect("talents") {
            let path = entry.expect("talent").path();
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            if name == format!("{talent}.md") || name.ends_with(".schema.json") {
                fs::copy(&path, talent_root.join(&name)).expect("talent copy");
            }
        }
        prepare(
            json!({"name":talent,"day":"20260101","facet":"work","span":["090000_60"],"activity":activity})
                .as_object()
                .expect("request object")
                .clone(),
            &RuntimePaths {
                talent_root,
                apps_root: root.path().join("apps"),
                templates_dir: payload.join("think/templates"),
            },
            &ExecutionContext { journal },
            PrepareMode::Preview,
        )
        .expect("prepare")
    }

    fn sections(text: &str) -> usize {
        text.lines().filter(|line| line.starts_with("## ")).count()
    }

    #[test]
    fn the_owners_account_and_a_recording_reach_every_activity_talent() {
        let base = json!({"id":"coding_090000_60","activity":"coding","description":"Release work","segments":["090000_60"]});
        let account = "We were \"rehearsing\", not presenting.\nMina ran the slides.";
        for talent in ["participation", "work", "conversation"] {
            let plain = render_shipped_talent(talent, base.clone());
            assert!(!plain.contains("$activity_context"), "{talent}");
            let mut told = base.clone();
            told["details"] = json!(account);
            told["playback"] = json!(true);
            let told = render_shipped_talent(talent, told);
            // Each one as a section, and again at the very end of the request.
            assert_eq!(sections(&told), sections(&plain) + 4, "{talent}");
            assert!(told.contains(account), "{talent}");
            // The talent's standing rule names each section it renders, so
            // renaming one in code cannot leave the rule pointing at nothing.
            for heading in ["Your Account", "A Recording"] {
                assert!(told.contains(&format!("\n## {heading}\n")), "{talent}");
                assert!(plain.contains(&format!("`{heading}`")), "{talent}");
            }

            // A record the store normalized, with nothing the owner said.
            let mut normalized = base.clone();
            normalized["details"] = json!("");
            normalized["playback"] = json!(false);
            assert_eq!(render_shipped_talent(talent, normalized), plain, "{talent}");
        }
    }

    #[test]
    fn a_correction_is_the_last_thing_an_activity_talent_reads() {
        let base = json!({"id":"coding_090000_60","activity":"coding","description":"Release work","segments":["090000_60"]});
        let account = "Mina ran the slides.";
        for talent in ["participation", "work", "conversation"] {
            let mut told = base.clone();
            told["details"] = json!(account);
            let told = render_shipped_talent(talent, told);
            let last = format!(
                "## What you told the journal\n\n{account}\n\nThis is what happened. Where anything above says otherwise, write it this way."
            );
            if talent == "participation" {
                assert!(told.ends_with(&last), "{talent}");
            } else {
                assert!(
                    told.ends_with(&format!(
                        "{last}\n\nStart your JSON with the field `correction`: copy those words exactly."
                    )),
                    "{talent}"
                );
            }

            let mut recording = base.clone();
            recording["playback"] = json!(true);
            let recording = render_shipped_talent(talent, recording);
            assert!(recording.contains("\n## A recording\n"), "{talent}");
            assert!(recording.ends_with("write it this way."), "{talent}");

            assert!(
                !render_shipped_talent(talent, base.clone()).contains("write it this way."),
                "{talent}"
            );
        }
    }

    #[test]
    fn a_story_talent_repeats_the_owners_account_first() {
        let base = json!({"id":"coding_090000_60","activity":"coding","description":"Release work","segments":["090000_60"]});
        let fields = |talent: &str, activity: Value| -> (Vec<String>, Value) {
            let prepared = prepare_shipped_talent(talent, activity);
            let schema = prepared.config["json_schema"].clone();
            let properties = schema["properties"].as_object().expect("properties");
            (
                properties.keys().cloned().collect(),
                schema["required"].clone(),
            )
        };
        let mut told = base.clone();
        told["details"] = json!("Mina ran the slides.");
        let mut recording = base.clone();
        recording["playback"] = json!(true);
        for talent in ["work", "conversation"] {
            let (first, required) = fields(talent, told.clone());
            assert_eq!(first[0], "correction", "{talent}");
            assert_eq!(required[0], "correction", "{talent}");
            // Only an account: nothing to copy from a recording, and no field
            // with nothing standing.
            for activity in [base.clone(), recording.clone()] {
                let (plain, required) = fields(talent, activity.clone());
                assert!(!plain.contains(&"correction".to_owned()), "{talent}");
                assert!(!required.to_string().contains("correction"), "{talent}");
                assert!(
                    !render_shipped_talent(talent, activity).contains("Start your JSON"),
                    "{talent}"
                );
            }
        }
        // A long account keeps its restatement, and the story keeps its output
        // budget rather than running out mid-JSON on the copy.
        let mut long = base.clone();
        long["details"] = json!("Mina ran the slides. ".repeat(100));
        for talent in ["work", "conversation"] {
            let (plain, _) = fields(talent, long.clone());
            assert!(!plain.contains(&"correction".to_owned()), "{talent}");
            let rendered = render_shipped_talent(talent, long.clone());
            assert!(rendered.ends_with("write it this way."), "{talent}");
        }
        let participation = prepare_shipped_talent("participation", told);
        assert!(
            !participation.config["json_schema"]
                .to_string()
                .contains("\"correction\""),
            "participation"
        );
    }

    #[test]
    fn a_story_opening_is_not_read_back_once_no_correction_stands() {
        const OPENING: &str = "Watched a recording of the keynote.";
        const STORY: &str = "You watched a recording of the keynote. It ran long.";
        let base = json!({"id":"coding_090000_60","activity":"coding","segments":["090000_60"]});
        for talent in ["participation", "work", "conversation"] {
            let reads = |description: &str, story: Option<&str>, playback: bool| {
                let mut activity = base.clone();
                activity["description"] = json!(description);
                if let Some(body) = story {
                    activity["story"] = json!({"talent":"conversation","body":body});
                }
                if playback {
                    activity["playback"] = json!(true);
                }
                render_shipped_talent(talent, activity)
                    .contains(&format!("**Description:** {description}\n"))
            };
            // The correction was removed: the line is what the corrected story opened with.
            assert!(!reads(OPENING, Some(STORY), false), "{talent}");
            // A correction stands, the line is not the story's, or there is no story yet.
            assert!(reads(OPENING, Some(STORY), true), "{talent}");
            assert!(reads("Owner wrote this.", Some(STORY), false), "{talent}");
            assert!(reads(OPENING, None, false), "{talent}");
        }
    }

    /// The shipped Sense prompt and schema over one transcript in a journal with
    /// `desk-a` and `home` enabled.
    fn prepare_shipped_sense(stream: &str, file: &str, header: Value) -> PreparedTalent {
        let root = tempfile::tempdir().expect("root");
        let payload = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../payload/solstone");
        let talent_root = root.path().join("talent");
        let journal = root.path().join("journal");
        let segment = journal
            .join("chronicle/20260101")
            .join(stream)
            .join("090000_300");
        for directory in [
            talent_root.clone(),
            root.path().join("apps"),
            journal.join("config"),
            segment.clone(),
        ] {
            fs::create_dir_all(directory).expect("fixture directory");
        }
        fs::write(
            journal.join("config/journal.json"),
            r#"{"identity":{"preferred":"Soleil"},"providers":{"active":{"provider":"none"}}}"#,
        )
        .expect("journal config");
        for (slug, title) in [("desk-a", "Desk Alpha"), ("home", "Home Things")] {
            let directory = journal.join("facets").join(slug);
            fs::create_dir_all(&directory).expect("facet directory");
            fs::write(
                directory.join("facet.json"),
                json!({"title":title,"description":title}).to_string(),
            )
            .expect("facet");
        }
        let mut transcript = header.to_string() + "\n";
        for text in ["Let's ship it.", "The release is ready.", "Tag it now."] {
            transcript += &(json!({"start":"00:00:01","text":text}).to_string() + "\n");
        }
        fs::write(segment.join(file), transcript).expect("transcript");
        for name in ["sense.md", "sense.schema.json"] {
            fs::copy(payload.join("talent").join(name), talent_root.join(name))
                .expect("talent copy");
        }
        prepare(
            json!({"name":"sense","day":"20260101","segment":"090000_300","stream":stream})
                .as_object()
                .expect("request object")
                .clone(),
            &RuntimePaths {
                talent_root,
                apps_root: root.path().join("apps"),
                templates_dir: payload.join("think/templates"),
            },
            &ExecutionContext { journal },
            PrepareMode::Preview,
        )
        .expect("prepare")
    }

    fn sense_facet_choices(prepared: &PreparedTalent) -> (Value, Value, Value) {
        let schema = &prepared.config["json_schema"]["properties"]["facets"];
        (
            schema["items"]["properties"]["facet"]["enum"].clone(),
            schema["items"]["properties"]["level"]["enum"].clone(),
            schema["maxItems"].clone(),
        )
    }

    #[test]
    fn an_import_facet_is_the_only_facet_sense_can_answer_with() {
        for (stream, file, header) in [
            (
                "import.audio",
                "imported_audio.jsonl",
                json!({"raw":"imported_audio.m4a","backend":"parakeet","duration":27.78,"import_id":"20260101_090000","stream":"import.audio","facet":"desk-a"}),
            ),
            (
                "import.text",
                "conversation_transcript.jsonl",
                json!({"imported":{"id":"20260101_090000","facet":"desk-a"},"raw":"../../../imports/20260101_090000/note.md","untimed":true,"topics":"lease terms","setting":"workplace"}),
            ),
        ] {
            let prepared = prepare_shipped_sense(stream, file, header);
            assert_eq!(
                sense_facet_choices(&prepared),
                (json!(["desk-a"]), json!(["high"]), json!(1)),
                "{stream}"
            );
            let instruction = prepared.config["user_instruction"].as_str().unwrap();
            assert!(
                instruction.contains("Desk Alpha"),
                "{stream}: {instruction}"
            );
            assert!(
                !instruction.contains("Home Things"),
                "{stream}: {instruction}"
            );
        }
    }

    #[test]
    fn sense_keeps_every_facet_without_a_declared_import_facet() {
        for (stream, file, header) in [
            ("default", "audio.jsonl", json!({"raw":"audio.flac"})),
            // A facet in a header that no import wrote is not the owner's choice.
            (
                "default",
                "audio.jsonl",
                json!({"raw":"audio.flac","facet":"desk-a"}),
            ),
            (
                "import.audio",
                "imported_audio.jsonl",
                json!({"raw":"imported_audio.m4a","import_id":"20260101_090000","stream":"import.audio"}),
            ),
            // Deleted or renamed since the import: it routes like any other segment.
            (
                "import.text",
                "conversation_transcript.jsonl",
                json!({"imported":{"id":"20260101_090000","facet":"gone"},"raw":"../../../imports/20260101_090000/note.md"}),
            ),
        ] {
            let prepared = prepare_shipped_sense(stream, file, header);
            assert_eq!(
                sense_facet_choices(&prepared),
                (
                    json!(["desk-a", "home"]),
                    json!(["high", "medium", "low"]),
                    json!(16)
                ),
                "{stream}"
            );
            let instruction = prepared.config["user_instruction"].as_str().unwrap();
            assert!(
                instruction.contains("Home Things"),
                "{stream}: {instruction}"
            );
        }
    }

    const PER_SEGMENT_SECTION: &str = "Activity State Per Segment";

    #[test]
    fn activity_context_shows_each_segments_facet_classification() {
        let instruction = render_activity_instruction(|journal| {
            fs::write(
                journal.join("chronicle").join("20260101").join("090000_60").join("talents").join("facets.json"),
                r#"[{"facet":"personal","activity":"Replied to a family message.","level":"low"},{"facet":"work","activity":"Debugged retry handling.","level":"high"}]"#,
            )
            .expect("first facets");
            fs::write(
                journal.join("chronicle").join("20260101").join("090200_120").join("talents").join("facets.json"),
                r#"[{"facet":"work","activity":"Verified the provider request.","level":"medium"}]"#,
            )
            .expect("second facets");
        });
        let section = instruction
            .split_once(&format!("## {PER_SEGMENT_SECTION}"))
            .expect("per-segment section is rendered")
            .1;
        let section = section.split_once("\n## ").expect("section ends").0;
        for (segment, level, activity) in [
            ("090000_60", "high", "Debugged retry handling."),
            ("090200_120", "medium", "Verified the provider request."),
        ] {
            assert!(section.contains(segment), "{segment} in {section}");
            assert!(section.contains(level), "{level} in {section}");
            assert!(section.contains(activity), "{activity} in {section}");
        }
        // Another facet's classification of the same segment stays out.
        assert!(!section.contains("family message"));
        assert!(instruction.contains("Release work"));
    }

    #[test]
    fn activity_context_never_points_at_a_per_segment_section_it_did_not_render() {
        for seed in [
            // No segment in the span has been classified.
            (|_: &Path| {}) as fn(&Path),
            // Segments are classified, but only into other facets.
            |journal: &Path| {
                fs::write(
                    journal.join("chronicle").join("20260101").join("090000_60").join("talents").join("facets.json"),
                    r#"[{"facet":"personal","activity":"Replied to a family message.","level":"high"}]"#,
                )
                .expect("facets");
            },
        ] {
            let instruction = render_activity_instruction(seed);
            assert!(
                !instruction.contains(PER_SEGMENT_SECTION),
                "instruction names a section it does not contain: {instruction}"
            );
            assert!(instruction.contains("## Analysis Focus"));
        }
    }

    #[cfg(feature = "full-tests")]
    #[test]
    fn browser_binding_does_not_follow_allocation() {
        let root = tempfile::tempdir().expect("root");
        let talent_root = root.path().join("talent");
        let apps_root = root.path().join("apps");
        let templates_dir = root.path().join("templates");
        let journal = root.path().join("journal");
        for directory in [
            talent_root.clone(),
            apps_root.clone(),
            templates_dir.clone(),
            journal.join("config"),
            journal.join("streams"),
            journal.join("chronicle").join("20260101").join("090000_60"),
        ] {
            fs::create_dir_all(directory).expect("fixture directory");
        }
        fs::write(
            journal.join("config").join("journal.json"),
            r#"{"identity":{"preferred":"Soleil"},"providers":{"active":{"provider":"none"}}}"#,
        )
        .expect("journal config");
        fs::write(
            talent_root.join("test_talent.md"),
            "{\n\"type\":\"generate\",\"max_output_tokens\":1024\n}\n$content_description",
        )
        .expect("talent");

        // Write streams/label_browser.json where top-level source is not browser, top-level kind is not browser,
        // and allocation.source is "browser"
        fs::write(
            journal.join("streams").join("label_browser.json"),
            r#"{"source":"screen","kind":"observed","allocation":{"source":"browser"}}"#,
        )
        .expect("stream record");

        let paths = RuntimePaths {
            talent_root,
            apps_root,
            templates_dir,
        };
        let exec_ctx = ExecutionContext {
            journal: journal.clone(),
        };

        let prepare_instruction = |stream: &str| {
            let prepared = prepare(
                json!({
                    "name": "test_talent",
                    "day": "20260101",
                    "segment": "090000_60",
                    "stream": stream,
                })
                .as_object()
                .expect("request object")
                .clone(),
                &paths,
                &exec_ctx,
                PrepareMode::Preview,
            )
            .expect("prepare");
            prepared.config["user_instruction"]
                .as_str()
                .expect("prepared instruction")
                .to_owned()
        };
        let screen_instruction = prepare_instruction("label_browser");
        let ordinary_instruction = prepare_instruction("ordinary_stream");
        assert!(!ordinary_instruction.is_empty());
        assert_eq!(screen_instruction, ordinary_instruction);

        fs::write(
            journal.join("streams").join("renamed_feed.json"),
            r#"{"source":"browser","kind":"browser","allocation":{"source":"screen"}}"#,
        )
        .expect("browser stream record");
        let browser_instruction = prepare_instruction("renamed_feed");
        let legacy_instruction = prepare_instruction("suze.browser");
        assert!(!browser_instruction.is_empty());
        assert_eq!(browser_instruction, legacy_instruction);
        assert_ne!(browser_instruction, ordinary_instruction);
    }
}
