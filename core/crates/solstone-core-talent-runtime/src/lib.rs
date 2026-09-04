// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Native, closed-set talent execution worker.

use std::collections::BTreeMap;
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use chrono::{DateTime, Utc};
use serde_json::{Map, Value, json};
use solstone_core_cogitate_wire::CogitateOneShotClient;
use solstone_core_generate::{
    ContentPart, GenerateRequest, GenerateResponse, OneShotClient, ReasonCode, ReasonCodeValue,
    RefusalReason, RefusedResponse, UnknownReasonCode,
};
use solstone_core_system_health::{DataState, read_segment_data_state};

pub mod activity_contract;
pub mod assemble;
pub mod cogitate;
pub mod contract;
pub mod daily_schedule;
pub mod documents;
pub mod entities;
pub mod facet_newsletter;
pub mod morning_briefing;
pub mod participation;
pub mod prepare;
mod prompt_context;
pub mod pulse;
pub mod schedule;
pub mod speaker_attribution;
pub mod steward;
pub mod steward_health;
pub mod steward_log;
pub mod story;
pub mod timeline;
mod transcript;
pub mod writers;

#[cfg(test)]
mod test_support;

use cogitate::{EngineKind, cogitate_request, from_prepared_config};
use contract::{CommitDisposition, GateDecision, PrePostState, resolve_hook};

/// Config key honored only by steward and speaker_attribution pre-steps.
/// Not a general per-stage dry-run flag.
pub(crate) const DRY_RUN_KEY: &str = "dry_run";

#[derive(Clone, Debug)]
pub struct ExecutionContext {
    pub journal: PathBuf,
}

#[derive(Clone, Debug)]
pub struct PreparedTalent {
    pub name: String,
    pub config: Map<String, Value>,
}

pub(crate) fn detected_resolution_entities(
    journal: &Path,
    facet: &str,
    day: &str,
) -> Result<Vec<solstone_core_entity::EntityResolutionEntity>, String> {
    let entities = solstone_core_facets::read_detected_entities(journal, facet, day)
        .map_err(|error| error.to_string())?
        .into_iter()
        .filter_map(|value| value.as_object().cloned())
        .map(|item| solstone_core_entity::EntityResolutionEntity {
            id: item.get("id").and_then(Value::as_str).map(str::to_owned),
            name: item
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            aka: string_values(&item, "aka"),
            emails: string_values(&item, "emails"),
            blocked: item
                .get("blocked")
                .is_some_and(solstone_core_facets::activity_value_truthy),
        })
        .collect::<Vec<_>>();
    Ok(entities)
}

fn string_values(value: &Map<String, Value>, field: &str) -> Vec<String> {
    value
        .get(field)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect()
}

pub fn check_segment_has_no_input(
    journal: &Path,
    day: &str,
    segment: &str,
    stream: Option<&str>,
    sources: &Map<String, Value>,
    now: DateTime<Utc>,
) -> bool {
    if !transcript::sources_are_enabled(sources) {
        return false;
    }
    let (text, counts) =
        transcript::load_segment_transcript(journal, day, segment, stream, sources);
    if solstone_core_transcripts::is_no_input(&text, &counts) {
        return true;
    }
    let data_state = read_segment_data_state(journal, day, segment, stream, now);
    // Talent output is not a detected modality. A talent-only segment has real transcript text
    // and an empty map, so the non-empty guard prevents vacuous all() gating it.
    !data_state.0.is_empty()
        && data_state
            .0
            .values()
            .all(|state| state == DataState::Empty.as_str())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StageError {
    pub phase: &'static str,
    pub stage: &'static str,
    pub talent: String,
    pub detail: String,
}

impl std::fmt::Display for StageError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "{} hook '{}' for talent '{}': {}",
            self.phase, self.stage, self.talent, self.detail
        )
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum RuntimeOutcome {
    Finished {
        output: String,
        disposition: CommitDisposition,
    },
    Skipped {
        stage: String,
        talent: String,
        reason: String,
    },
    UnportedHook {
        hook: String,
        talent: String,
    },
    PrepareSkipped {
        talent: String,
        reason: String,
    },
    SchemaValidationFailed {
        talent: String,
        validation: Value,
    },
    PrepareFailed(prepare::PrepareFailure),
    StageFailed(StageError),
    GenerateRefused {
        error: StageError,
        response: Box<RefusedResponse>,
    },
    CogitateRefused {
        error: StageError,
        response: Box<RefusedResponse>,
    },
}

pub fn run_worker(_args: &[String], journal: &Path) -> ExitCode {
    let context = ExecutionContext {
        journal: journal.to_path_buf(),
    };
    let paths = match runtime_paths_from_current_executable() {
        Ok(paths) => paths,
        Err(error) => {
            eprintln!("talent runtime: {error}");
            return ExitCode::from(70);
        }
    };
    let generate = OneShotClient::sibling();
    let cogitate = CogitateOneShotClient::sibling().map(configure_cogitate_client);
    let stdin = io::stdin().lock();
    let mut stdout = io::stdout().lock();
    run_lines(
        stdin,
        &mut stdout,
        &paths,
        &context,
        generate.as_ref(),
        cogitate.as_ref(),
    );
    ExitCode::SUCCESS
}

#[cfg(test)]
fn configure_generate_client(client: OneShotClient) -> OneShotClient {
    client.with_prefix_arguments(["generate".into()])
}

fn configure_cogitate_client(client: CogitateOneShotClient) -> CogitateOneShotClient {
    client.with_prefix_arguments(["cogitate".into()])
}

fn runtime_paths_from_current_executable() -> Result<prepare::RuntimePaths, String> {
    let executable = std::env::current_exe()
        .map_err(|error| format!("could not resolve worker executable: {error}"))?;
    let executable_dir = executable
        .parent()
        .ok_or_else(|| format!("worker executable has no parent: {}", executable.display()))?;
    runtime_paths_from_executable_dir(executable_dir)
}

pub(crate) fn runtime_paths_from_executable_dir(
    executable_dir: &Path,
) -> Result<prepare::RuntimePaths, String> {
    let root = solstone_core_journal::resolve_installation_root_from_executable_dir(executable_dir)
        .ok_or_else(|| {
            format!(
                "could not resolve installation root from executable directory: {}",
                executable_dir.display()
            )
        })?;
    Ok(prepare::RuntimePaths {
        talent_root: root.join("solstone/talent"),
        apps_root: root.join("solstone/apps"),
        templates_dir: root.join("solstone/think/templates"),
    })
}

fn run_lines(
    reader: impl BufRead,
    writer: &mut impl Write,
    paths: &prepare::RuntimePaths,
    context: &ExecutionContext,
    generate: Result<&OneShotClient, &solstone_core_generate::ClientError>,
    cogitate: Result<&CogitateOneShotClient, &solstone_core_cogitate_wire::ClientError>,
) {
    for line in reader.lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let request = match serde_json::from_str::<Value>(&line).and_then(|value| {
            value.as_object().cloned().ok_or_else(|| {
                serde_json::Error::io(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "request must be an object",
                ))
            })
        }) {
            Ok(request) => request,
            Err(error) => {
                emit(
                    writer,
                    json!({"event":"error", "terminal":true, "error": format!("invalid talent request: {error}")}),
                );
                continue;
            }
        };
        let talent = request
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_owned();
        let outcome = match (generate, cogitate) {
            (Ok(generate), Ok(cogitate)) => {
                execute_request(request, paths, context, generate, cogitate, writer)
            }
            (Err(error), _) => RuntimeOutcome::StageFailed(StageError {
                phase: "generate",
                stage: "runtime",
                talent,
                detail: format!("{error}"),
            }),
            (_, Err(error)) => RuntimeOutcome::StageFailed(StageError {
                phase: "cogitate",
                stage: "runtime",
                talent,
                detail: format!("{error:?}"),
            }),
        };
        emit_outcome(writer, outcome);
    }
}

pub fn execute_request(
    request: Map<String, Value>,
    paths: &prepare::RuntimePaths,
    context: &ExecutionContext,
    generate: &OneShotClient,
    cogitate: &CogitateOneShotClient,
    writer: &mut impl Write,
) -> RuntimeOutcome {
    let mut prepared =
        match prepare::prepare(request, paths, context, prepare::PrepareMode::Execute) {
            Ok(prepared) => prepared,
            Err(error) => return RuntimeOutcome::PrepareFailed(error),
        };
    emit_start(writer, &prepared);
    if let Some(reason) = prepared.config.get("skip_reason").and_then(Value::as_str) {
        return RuntimeOutcome::PrepareSkipped {
            talent: prepared.name.clone(),
            reason: reason.to_owned(),
        };
    }
    let engine = match from_prepared_config(&prepared.config) {
        Ok(engine) => engine,
        Err(outcome) => return outcome,
    };
    let hook = prepared
        .config
        .get("hook")
        .and_then(Value::as_object)
        .and_then(|hook| hook.get("pre").or_else(|| hook.get("post")))
        .and_then(Value::as_str);
    let Some(hook) = hook else {
        return generate_and_write(
            &mut prepared,
            context,
            generate,
            cogitate,
            writer,
            engine,
            None,
        );
    };
    let Some(stage) = resolve_hook(hook) else {
        return RuntimeOutcome::UnportedHook {
            hook: hook.to_owned(),
            talent: prepared.name.clone(),
        };
    };
    if let Some(gate) = stage.gate {
        match gate(&prepared, context) {
            Ok(GateDecision::Proceed) => {}
            Ok(GateDecision::Skip(reason)) => {
                return RuntimeOutcome::Skipped {
                    stage: hook.to_owned(),
                    talent: prepared.name.clone(),
                    reason,
                };
            }
            Err(error) => return RuntimeOutcome::StageFailed(error),
        }
    }
    let state = match stage.build {
        Some(build) => match build(&mut prepared, context) {
            Ok(state) => state,
            Err(outcome) => return outcome,
        },
        None => PrePostState::None,
    };
    if let Some(override_prompt) = stage.prompt_override
        && let Err(error) = override_prompt(&mut prepared, &state)
    {
        return RuntimeOutcome::StageFailed(error);
    }
    generate_and_write(
        &mut prepared,
        context,
        generate,
        cogitate,
        writer,
        engine,
        Some((stage, state)),
    )
}

fn emit_start(writer: &mut impl Write, prepared: &PreparedTalent) {
    let event = Map::from_iter([
        ("event".to_owned(), json!("start")),
        ("name".to_owned(), json!(prepared.name)),
        (
            "prompt".to_owned(),
            prepared
                .config
                .get("prompt")
                .cloned()
                .unwrap_or(Value::Null),
        ),
        (
            "model".to_owned(),
            prepared.config.get("model").cloned().unwrap_or(Value::Null),
        ),
        (
            "provider".to_owned(),
            prepared
                .config
                .get("provider")
                .cloned()
                .unwrap_or(Value::Null),
        ),
    ]);
    emit(writer, Value::Object(event));
}

pub(crate) fn generate_and_write(
    prepared: &mut PreparedTalent,
    context: &ExecutionContext,
    generate: &OneShotClient,
    cogitate: &CogitateOneShotClient,
    writer: &mut impl Write,
    engine: EngineKind,
    stage: Option<(&'static contract::StageSpec, PrePostState)>,
) -> RuntimeOutcome {
    let mut stage = stage;
    let response = match engine {
        EngineKind::Generate => match generate.execute(&generate_request(prepared)) {
            Ok(GenerateResponse::Generated(response)) => {
                if prepared.config.contains_key("json_schema")
                    && schema_validation_failed(response.schema_validation.as_ref())
                {
                    return RuntimeOutcome::SchemaValidationFailed {
                        talent: prepared.name.clone(),
                        validation: response.schema_validation.clone().unwrap_or(Value::Null),
                    };
                }
                if let Some((stage, state)) = stage.as_mut()
                    && matches!(stage.stage, contract::StageId::TimelineSegmentSummary)
                    && let Err(detail) = timeline::attach_generated_provenance(state, &response)
                {
                    return RuntimeOutcome::StageFailed(stage_error(
                        "generate",
                        "timeline:segment_summary",
                        prepared,
                        detail,
                    ));
                }
                response.text.clone()
            }
            Ok(GenerateResponse::Refused(response)) => {
                return RuntimeOutcome::GenerateRefused {
                    error: stage_error("generate", "runtime", prepared, response.detail.clone()),
                    response: Box::new(response),
                };
            }
            Err(error) => {
                return RuntimeOutcome::StageFailed(stage_error(
                    "generate",
                    "runtime",
                    prepared,
                    format!("{error}"),
                ));
            }
        },
        EngineKind::Cogitate => match cogitate_output(prepared, context, cogitate, writer) {
            Ok(output) => output,
            Err(outcome) => return outcome,
        },
    };
    if let Some((stage, state)) = stage {
        let disposition;
        if let Some(commit) = stage.commit {
            let parsed = match (commit.parse)(&response, prepared, &state) {
                Ok(parsed) => parsed,
                Err(_) if matches!(stage.stage, contract::StageId::Story) => {
                    // Empty body/topics (and other parse refusals) do not
                    // mutate the activity. This is a finish with
                    // RejectedNoMutation, not StageFailed, so think-cli
                    // records talent.complete and health/fail-rate that key
                    // on talent.fail do not see it.
                    return RuntimeOutcome::Finished {
                        output: response,
                        disposition: CommitDisposition::RejectedNoMutation,
                    };
                }
                Err(error) => return RuntimeOutcome::StageFailed(error),
            };
            let plan = match (commit.commit)(parsed, prepared, &state) {
                Ok(plan) => plan,
                Err(error) => return RuntimeOutcome::StageFailed(error),
            };
            disposition = match stage.writes_as_intent {
                Some(apply) => match apply(plan, context) {
                    Ok(value) => value,
                    Err(error) => return RuntimeOutcome::StageFailed(error),
                },
                None => CommitDisposition::CommittedNoOutput,
            };
        } else {
            disposition = match writers::write_output_if_configured(prepared, &response) {
                Ok(_) => CommitDisposition::Written,
                Err(error) => {
                    return RuntimeOutcome::StageFailed(stage_error(
                        "write", "runtime", prepared, error,
                    ));
                }
            };
        }
        let output = match stage.output_override {
            Some(override_output) => match override_output(&response, prepared, &state) {
                Ok(output) => output,
                Err(error) => return RuntimeOutcome::StageFailed(error),
            },
            None => response,
        };
        return RuntimeOutcome::Finished {
            output,
            disposition,
        };
    }
    match writers::write_output_if_configured(prepared, &response) {
        Ok(_) => RuntimeOutcome::Finished {
            output: response,
            disposition: CommitDisposition::Written,
        },
        Err(error) => RuntimeOutcome::StageFailed(stage_error("write", "runtime", prepared, error)),
    }
}

fn cogitate_output(
    prepared: &PreparedTalent,
    context: &ExecutionContext,
    client: &CogitateOneShotClient,
    writer: &mut impl Write,
) -> Result<String, RuntimeOutcome> {
    let request = cogitate_request(prepared, context)?;
    let run = client.execute(&request).map_err(|error| {
        RuntimeOutcome::StageFailed(stage_error(
            "cogitate",
            "runtime",
            prepared,
            format!("{error:?}"),
        ))
    })?;
    for event in &run.events {
        emit(writer, event.clone());
    }
    let Some(terminal) = run.events.iter().rev().find(|event| {
        matches!(
            event.get("event").and_then(Value::as_str),
            Some("finish" | "error")
        )
    }) else {
        return Err(RuntimeOutcome::StageFailed(stage_error(
            "cogitate",
            "runtime",
            prepared,
            "cogitate one-shot produced no terminal event",
        )));
    };
    match terminal.get("event").and_then(Value::as_str) {
        Some("error")
            if terminal
                .get("provider_failure")
                .is_some_and(Value::is_object) =>
        {
            Err(cogitate_refused(prepared, terminal))
        }
        Some("error") => Err(RuntimeOutcome::StageFailed(stage_error(
            "cogitate",
            "runtime",
            prepared,
            terminal
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("cogitate run failed"),
        ))),
        Some("finish") => Ok(terminal
            .get("result")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned()),
        _ => Err(RuntimeOutcome::StageFailed(stage_error(
            "cogitate",
            "runtime",
            prepared,
            "cogitate one-shot produced no terminal event",
        ))),
    }
}

fn cogitate_refused(prepared: &PreparedTalent, terminal: &Value) -> RuntimeOutcome {
    let detail = terminal
        .get("error")
        .and_then(Value::as_str)
        .unwrap_or("cogitate run failed")
        .to_owned();
    let failure = terminal.get("provider_failure");
    let code = failure
        .and_then(|value| value.get("reason_code"))
        .and_then(Value::as_str)
        .or_else(|| terminal.get("reason_code").and_then(Value::as_str));
    RuntimeOutcome::CogitateRefused {
        error: stage_error("cogitate", "runtime", prepared, detail.clone()),
        response: Box::new(RefusedResponse {
            id: None,
            reason: RefusalReason::Unknown,
            reason_code: code.map(reason_code_value),
            retryable: failure
                .and_then(|value| value.get("retryable"))
                .and_then(Value::as_bool)
                .unwrap_or(false),
            blocking: failure
                .and_then(|value| value.get("blocking"))
                .and_then(Value::as_bool)
                .unwrap_or(false),
            reset_at_ms: None,
            provider: None,
            detail,
        }),
    }
}

fn reason_code_value(code: &str) -> ReasonCodeValue {
    match ReasonCode::new(code) {
        Ok(code) => ReasonCodeValue::Known(code),
        Err(_) => ReasonCodeValue::Unknown(UnknownReasonCode {
            received: code.to_owned(),
            canonical: ReasonCode::new("unknown").expect("unknown is in the generate fixture"),
        }),
    }
}

fn schema_validation_failed(validation: Option<&Value>) -> bool {
    validation.is_some_and(|validation| {
        validation.get("valid") == Some(&Value::Bool(false))
            || validation
                .get("errors")
                .and_then(Value::as_array)
                .is_some_and(|errors| !errors.is_empty())
    })
}

pub(crate) fn generate_contents(prepared: &PreparedTalent) -> Vec<ContentPart> {
    let contents: Vec<ContentPart> = prepared
        .config
        .get("messages")
        .and_then(Value::as_array)
        .map(|messages| {
            messages
                .iter()
                .filter_map(|message| message.get("content").and_then(Value::as_str))
                .map(|text| ContentPart::Text {
                    text: text.to_owned(),
                })
                .collect()
        })
        .unwrap_or_else(|| {
            ["transcript", "user_instruction", "prompt"]
                .into_iter()
                .filter_map(|key| prepared.config.get(key).and_then(Value::as_str))
                .filter(|text| !text.is_empty())
                .map(|text| ContentPart::Text {
                    text: text.to_owned(),
                })
                .collect()
        });
    if contents.is_empty() {
        vec![ContentPart::Text {
            text: "No input provided.".to_owned(),
        }]
    } else {
        contents
    }
}

fn generate_request(prepared: &PreparedTalent) -> GenerateRequest {
    GenerateRequest {
        id: None,
        context: prepared.name.clone(),
        contents: generate_contents(prepared),
        system_instruction: None,
        temperature: prepared
            .config
            .get("temperature")
            .and_then(Value::as_f64)
            .unwrap_or(0.3),
        max_output_tokens: prepared
            .config
            .get("max_output_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(8192 * 6),
        thinking_budget: prepared
            .config
            .get("thinking_budget")
            .and_then(Value::as_u64),
        timeout_s: None,
        json_output: prepared.config.contains_key("json_schema"),
        json_schema: prepared.config.get("json_schema").cloned(),
        enforce_responsiveness: false,
        attempt_index: 0,
        exclusive_admission: false,
        transport_retries: None,
    }
}

pub fn apply_template_vars(config: &mut Map<String, Value>, values: &Map<String, Value>) {
    let mut vars = BTreeMap::new();
    for (key, value) in values {
        let value = value_to_string(value);
        vars.insert(key.clone(), value.clone());
        vars.insert(python_capitalize(key), python_capitalize(&value));
    }
    for field in ["user_instruction", "transcript", "prompt"] {
        if let Some(Value::String(value)) = config.get_mut(field) {
            *value = solstone_core_talent_cli::safe_substitute(value, &vars);
        }
    }
}

fn python_capitalize(value: &str) -> String {
    let mut characters = value.chars();
    let Some(first) = characters.next() else {
        return String::new();
    };
    first
        .to_uppercase()
        .chain(characters.flat_map(char::to_lowercase))
        .collect()
}

fn value_to_string(value: &Value) -> String {
    match value {
        Value::String(value) => value.clone(),
        _ => value.to_string(),
    }
}

pub fn stage_error(
    phase: &'static str,
    stage: &'static str,
    prepared: &PreparedTalent,
    detail: impl Into<String>,
) -> StageError {
    StageError {
        phase,
        stage,
        talent: prepared.name.clone(),
        detail: detail.into(),
    }
}

fn emit(writer: &mut impl Write, event: Value) {
    let _ = serde_json::to_writer(&mut *writer, &event);
    let _ = writer.write_all(b"\n");
}

fn emit_outcome(writer: &mut impl Write, outcome: RuntimeOutcome) {
    match outcome {
        RuntimeOutcome::Finished {
            output,
            disposition,
        } => emit(
            writer,
            json!({"event":"finish", "output": output, "disposition": format!("{disposition:?}")}),
        ),
        RuntimeOutcome::Skipped {
            stage,
            talent,
            reason,
        } => emit(
            writer,
            json!({"event":"finish", "stage":stage, "name":talent, "skip_reason":reason}),
        ),
        RuntimeOutcome::UnportedHook { hook, talent } => emit(
            writer,
            json!({"event":"error", "terminal":true, "name":talent, "error":format!("unported talent hook: {hook}")}),
        ),
        RuntimeOutcome::PrepareSkipped { talent, reason } => emit(
            writer,
            json!({"event":"finish", "name":talent, "skip_reason":reason}),
        ),
        RuntimeOutcome::SchemaValidationFailed { talent, validation } => emit(
            writer,
            json!({"event":"error", "terminal":true, "name":talent, "error":"talent output failed schema validation", "schema_validation":validation}),
        ),
        RuntimeOutcome::PrepareFailed(error) => emit(
            writer,
            json!({"event":"error", "terminal":true, "error":error.to_string()}),
        ),
        RuntimeOutcome::StageFailed(error) => emit(
            writer,
            json!({"event":"error", "terminal":true, "name":error.talent, "error":error.to_string()}),
        ),
        RuntimeOutcome::GenerateRefused { error, response }
        | RuntimeOutcome::CogitateRefused { error, response } => emit(
            writer,
            json!({
                "event": "error",
                "terminal": true,
                "name": error.talent,
                "error": error.to_string(),
                "reason": response.reason.as_str(),
                "reason_code": response.reason_code.as_ref().map(ReasonCodeValue::as_wire),
                "retryable": response.retryable,
                "blocking": response.blocking,
                "provider": response.provider,
            }),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Cursor;

    fn fixture(
        name: &str,
        metadata: &str,
    ) -> (tempfile::TempDir, prepare::RuntimePaths, ExecutionContext) {
        let root = tempfile::tempdir().unwrap();
        let talent_root = root.path().join("talent");
        let apps_root = root.path().join("apps");
        let templates_dir = root.path().join("templates");
        fs::create_dir_all(&talent_root).unwrap();
        fs::create_dir_all(&apps_root).unwrap();
        fs::create_dir_all(&templates_dir).unwrap();
        fs::write(
            talent_root.join(format!("{name}.md")),
            format!("{metadata}\nworker fixture"),
        )
        .unwrap();
        let paths = prepare::RuntimePaths {
            talent_root,
            apps_root,
            templates_dir,
        };
        let context = ExecutionContext {
            journal: root.path().join("journal"),
        };
        fs::create_dir_all(&context.journal).unwrap();
        fs::create_dir_all(context.journal.join("config")).unwrap();
        fs::write(
            context.journal.join("config/journal.json"),
            r#"{"providers":{"active":{"provider":"test","model":"test-model"}}}"#,
        )
        .unwrap();
        (root, paths, context)
    }

    fn events(bytes: &[u8]) -> Vec<Value> {
        std::str::from_utf8(bytes)
            .unwrap()
            .lines()
            .map(serde_json::from_str)
            .collect::<Result<_, _>>()
            .unwrap()
    }

    fn unused_cogitate(root: &Path) -> CogitateOneShotClient {
        CogitateOneShotClient::at_path(root.join("unused-cogitate"))
    }

    fn segment_dir(context: &ExecutionContext, day: &str, segment: &str) -> PathBuf {
        let path = context.journal.join("chronicle").join(day).join(segment);
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn source_config(value: Value) -> Map<String, Value> {
        value.as_object().cloned().unwrap()
    }

    #[test]
    fn criterion_5_template_vars_match_python_capitalize_and_keep_unmatched() {
        let mut config = Map::from_iter([
            (
                "user_instruction".to_owned(),
                Value::String("$foo $Foo $missing".to_owned()),
            ),
            (
                "transcript".to_owned(),
                Value::String("${foo} ${Foo}".to_owned()),
            ),
            ("prompt".to_owned(), Value::String("$foo".to_owned())),
        ]);
        apply_template_vars(
            &mut config,
            &Map::from_iter([("foo".to_owned(), Value::String("bAR".to_owned()))]),
        );
        assert_eq!(config["user_instruction"], "bAR Bar $missing");
        assert_eq!(config["transcript"], "bAR Bar");
        assert_eq!(config["prompt"], "bAR");
    }

    #[test]
    fn criterion_23_injected_one_shot_path_reaches_the_client() {
        let root = tempfile::tempdir().unwrap();
        let client = OneShotClient::at_path(test_support::one_shot_stub(root.path(), "stubbed"));
        let prepared = PreparedTalent {
            name: "plain".to_owned(),
            config: Map::from_iter([("prompt".to_owned(), Value::String("hello".to_owned()))]),
        };
        let request = generate_request(&prepared);
        let GenerateResponse::Generated(response) = client.execute(&request).unwrap() else {
            panic!("stub generates")
        };
        assert_eq!(response.text, "stubbed");
        assert!(
            OneShotClient::at_path(root.path().join("missing"))
                .execute(&request)
                .is_err()
        );
    }

    #[test]
    fn sibling_client_uses_the_generate_one_shot_boundary() {
        let root = tempfile::Builder::new()
            .prefix("solstone-talent-generate-client-")
            .tempdir_in("/var/tmp")
            .unwrap();
        let client = configure_generate_client(OneShotClient::at_path(
            test_support::generate_one_shot_stub(root.path(), "generated"),
        ));
        let prepared = PreparedTalent {
            name: "plain".to_owned(),
            config: Map::from_iter([("prompt".to_owned(), Value::String("hello".to_owned()))]),
        };
        let GenerateResponse::Generated(response) =
            client.execute(&generate_request(&prepared)).unwrap()
        else {
            panic!("stub generates")
        };
        assert_eq!(response.text, "generated");
    }

    #[test]
    fn cogitate_execute_request_replays_events_and_writes_output_path() {
        let (root, paths, context) = fixture(
            "weekly_reflection",
            r#"{
"type":"cogitate", "schedule":"weekly", "output":"md", "load":{"transcripts":false}
}"#,
        );
        let output_path = context.journal.join("reflections/weekly/20260809.md");
        let cogitate = configure_cogitate_client(CogitateOneShotClient::at_path(
            test_support::cogitate_one_shot_stub(
                root.path(),
                &[
                    r#"{"event":"tool_start","tool":"solstone"}"#,
                    r#"{"event":"tool_end","tool":"solstone","is_error":false}"#,
                    r#"{"event":"finish","terminal":true,"result":"week notes","usage":{"input_tokens":1}}"#,
                ],
            ),
        ));
        let generate = OneShotClient::at_path(test_support::generate_one_shot_stub(
            root.path(),
            "should-not-run",
        ));
        let mut output = Vec::new();
        let outcome = execute_request(
            json!({
                "name":"weekly_reflection",
                "use_id":"use-week",
                "day":"20260809",
                "prompt":"Running scheduled weekly reflection.",
                "output_path": output_path.display().to_string()
            })
            .as_object()
            .unwrap()
            .clone(),
            &paths,
            &context,
            &generate,
            &cogitate,
            &mut output,
        );
        assert!(
            matches!(outcome, RuntimeOutcome::Finished { .. }),
            "{outcome:?}"
        );
        let output_events = events(&output);
        let kinds: Vec<_> = output_events
            .iter()
            .filter_map(|event| event.get("event").and_then(Value::as_str))
            .collect();
        assert!(
            kinds.contains(&"tool_start")
                && kinds.contains(&"tool_end")
                && kinds.iter().filter(|event| **event == "finish").count() >= 1,
            "{kinds:?}"
        );
        assert_eq!(fs::read_to_string(&output_path).unwrap(), "week notes");

        let mut failed_output = Vec::new();
        let mismatched = configure_cogitate_client(CogitateOneShotClient::at_path(
            test_support::generate_one_shot_stub(root.path(), "generated"),
        ));
        let failed = execute_request(
            json!({
                "name":"weekly_reflection",
                "use_id":"use-week-2",
                "prompt":"hello"
            })
            .as_object()
            .unwrap()
            .clone(),
            &paths,
            &context,
            &generate,
            &mismatched,
            &mut failed_output,
        );
        assert!(
            matches!(failed, RuntimeOutcome::StageFailed(_)),
            "{failed:?}"
        );
    }

    #[test]
    fn cogitate_execute_request_preserves_use_id_as_correlation_id() {
        let (root, paths, context) = fixture(
            "partner",
            r#"{
"type":"cogitate", "access_tier":"synthesis", "schedule":"weekly", "load":{"transcripts":false}
}"#,
        );
        let capture = root.path().join("captured-request.json");
        let stub = root.path().join("capture-stub.sh");
        fs::write(
            &stub,
            format!(
                "#!/bin/sh\n[ \"$1\" = cogitate ] && [ \"$2\" = --one-shot ] || exit 92\ncat > '{}'\nprintf '%s\\n' '{{ \"event\":\"finish\",\"terminal\":true,\"result\":\"ok\" }}'\n",
                capture.display()
            ),
        )
        .unwrap();
        let mut permissions = fs::metadata(&stub).unwrap().permissions();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            permissions.set_mode(0o700);
            fs::set_permissions(&stub, permissions).unwrap();
        }
        let cogitate = configure_cogitate_client(CogitateOneShotClient::at_path(&stub));
        let generate = OneShotClient::at_path(test_support::one_shot_stub(root.path(), "no"));
        for use_id in ["use-a", "use-b"] {
            let mut output = Vec::new();
            execute_request(
                json!({
                    "name":"partner",
                    "use_id": use_id,
                    "prompt":"hello"
                })
                .as_object()
                .unwrap()
                .clone(),
                &paths,
                &context,
                &generate,
                &cogitate,
                &mut output,
            );
            let captured: Value =
                serde_json::from_str(&fs::read_to_string(&capture).unwrap()).unwrap();
            assert_eq!(captured["correlation_id"], use_id);
        }
    }

    #[test]
    fn cogitate_execute_request_missing_use_id_is_stage_failed() {
        let (root, paths, context) = fixture(
            "partner",
            r#"{
"type":"cogitate", "load":{"transcripts":false}
}"#,
        );
        let generate =
            OneShotClient::at_path(test_support::one_shot_stub(root.path(), "generated"));
        let cogitate = unused_cogitate(root.path());
        let mut output = Vec::new();
        let outcome = execute_request(
            json!({"name":"partner", "prompt":"hello"})
                .as_object()
                .unwrap()
                .clone(),
            &paths,
            &context,
            &generate,
            &cogitate,
            &mut output,
        );
        let RuntimeOutcome::StageFailed(error) = outcome else {
            panic!("expected StageFailed, got {outcome:?}");
        };
        assert_eq!(error.phase, "cogitate");
        assert!(error.detail.contains("use_id"), "{}", error.detail);
    }

    #[test]
    fn criterion_1_ndjson_start_then_finish_writes_derived_output() {
        let (root, paths, context) = fixture(
            "plain",
            r#"{
"type":"generate", "output":"md", "load":{"transcripts":false}
}"#,
        );
        let client = OneShotClient::at_path(test_support::one_shot_stub(root.path(), "generated"));
        let mut output = Vec::new();
        run_lines(
            Cursor::new("{\"name\":\"plain\",\"day\":\"20260101\",\"prompt\":\"hello\"}\n"),
            &mut output,
            &paths,
            &context,
            Ok(&client),
            Ok(&unused_cogitate(root.path())),
        );
        let output_events = events(&output);
        assert_eq!(output_events.len(), 2);
        assert_eq!(output_events[0]["event"], "start");
        assert_eq!(output_events[0]["name"], "plain");
        assert!(output_events[0].get("model").is_some());
        assert!(output_events[0].get("provider").is_some());
        assert_eq!(output_events[1]["event"], "finish");
        assert_eq!(
            fs::read_to_string(context.journal.join("chronicle/20260101/talents/plain.md"))
                .unwrap(),
            "generated"
        );
    }

    #[test]
    fn disabled_talent_skips_and_enabled_talent_runs() {
        let (root, paths, context) = fixture(
            "plain",
            r#"{
"type":"generate", "output":"md", "load":{"transcripts":false}
}"#,
        );
        fs::write(
            context.journal.join("config/journal.json"),
            r#"{"providers":{"active":{"provider":"test","model":"test-model"}},"talent_overrides":{"talent.system.plain":{"disabled":true}}}"#,
        )
        .unwrap();
        let client = OneShotClient::at_path(test_support::one_shot_stub(root.path(), "generated"));
        let request = "{\"name\":\"plain\",\"day\":\"20260101\",\"prompt\":\"hello\"}\n";
        let mut disabled_output = Vec::new();
        run_lines(
            Cursor::new(request),
            &mut disabled_output,
            &paths,
            &context,
            Ok(&client),
            Ok(&unused_cogitate(root.path())),
        );
        let disabled_events = events(&disabled_output);
        assert_eq!(disabled_events.len(), 2);
        assert_eq!(disabled_events[0]["event"], "start");
        assert_eq!(disabled_events[1]["event"], "finish");
        assert_eq!(disabled_events[1]["skip_reason"], "disabled");
        let output_path = context.journal.join("chronicle/20260101/talents/plain.md");
        assert!(!output_path.exists());

        fs::write(
            context.journal.join("config/journal.json"),
            r#"{"providers":{"active":{"provider":"test","model":"test-model"}}}"#,
        )
        .unwrap();
        let mut enabled_output = Vec::new();
        run_lines(
            Cursor::new(request),
            &mut enabled_output,
            &paths,
            &context,
            Ok(&client),
            Ok(&unused_cogitate(root.path())),
        );
        let enabled_events = events(&enabled_output);
        assert_eq!(enabled_events.len(), 2);
        assert_eq!(enabled_events[0]["event"], "start");
        assert_eq!(enabled_events[1]["event"], "finish");
        assert_eq!(fs::read_to_string(output_path).unwrap(), "generated");
    }

    #[test]
    fn criterion_7_schema_validation_blocks_story_commit_end_to_end() {
        let (root, paths, context) = fixture(
            "conversation",
            r#"{
"type":"generate", "output":"json", "schema":"story.schema.json", "hook":{"post":"story"}, "load":{"transcripts":false}
}"#,
        );
        fs::write(
            paths.talent_root.join("story.schema.json"),
            r#"{"type":"object","required":["body"]}"#,
        )
        .unwrap();
        let activity_path = context
            .journal
            .join("facets/work/activities/20260101.jsonl");
        fs::create_dir_all(activity_path.parent().unwrap()).unwrap();
        fs::write(
            context.journal.join("facets/work/facet.json"),
            r#"{"title":"Work"}"#,
        )
        .unwrap();
        fs::write(
            &activity_path,
            "{\"id\":\"activity-1\",\"story\":{\"old\":true}}\n",
        )
        .unwrap();
        let before = fs::read(&activity_path).unwrap();
        let client = OneShotClient::at_path(test_support::one_shot_stub_with_schema_validation(
            root.path(),
            r#"{"body":"valid enough for the hook","topics":["work"],"confidence":1,"commitments":[],"closures":[],"decisions":[],"relations":[]}"#,
            json!({"valid":false,"errors":[{"path":"/body","constraint":"minLength"}]}),
        ));
        let mut output = Vec::new();
        run_lines(
            Cursor::new(
                "{\"name\":\"conversation\",\"day\":\"20260101\",\"facet\":\"work\",\"activity\":{\"id\":\"activity-1\"},\"prompt\":\"hello\"}\n",
            ),
            &mut output,
            &paths,
            &context,
            Ok(&client),
            Ok(&unused_cogitate(root.path())),
        );
        let output_events = events(&output);
        assert_eq!(output_events.len(), 2);
        assert_eq!(output_events[0]["event"], "start");
        assert_eq!(output_events[1]["event"], "error");
        assert_eq!(
            output_events[1]["error"],
            "talent output failed schema validation"
        );
        assert_eq!(fs::read(&activity_path).unwrap(), before);
        assert!(
            !context
                .journal
                .join("chronicle/20260101/talents/conversation.json")
                .exists()
        );
    }

    #[test]
    fn criterion_2_line_loop_skips_blank_reports_one_error_and_continues() {
        let (root, paths, context) = fixture(
            "plain",
            r#"{
"type":"generate", "output":"md", "load":{"transcripts":false}
}"#,
        );
        let client = OneShotClient::at_path(test_support::one_shot_stub(root.path(), "generated"));
        let input = "\nnot json\n{\"name\":\"plain\",\"day\":\"20260101\",\"prompt\":\"hello\"}\n";
        let mut output = Vec::new();
        run_lines(
            Cursor::new(input),
            &mut output,
            &paths,
            &context,
            Ok(&client),
            Ok(&unused_cogitate(root.path())),
        );
        let output_events = events(&output);
        assert_eq!(
            output_events
                .iter()
                .filter(|event| event["event"] == "error")
                .count(),
            1
        );
        assert_eq!(
            output_events
                .iter()
                .filter(|event| event["event"] == "start")
                .count(),
            1
        );
        assert_eq!(
            output_events
                .iter()
                .filter(|event| event["event"] == "finish")
                .count(),
            1
        );
    }

    #[test]
    fn criterion_10_unported_hook_and_ported_transcript_loading() {
        let (root, paths, context) = fixture(
            "speaker-attribution-fixture",
            r#"{
"type":"generate", "hook":{"pre":"unknown_native_hook"}, "load":{"transcripts":false}
}"#,
        );
        let client = OneShotClient::at_path(test_support::one_shot_stub(root.path(), "generated"));
        let mut output = Vec::new();
        let outcome = execute_request(
            json!({"name":"speaker-attribution-fixture", "prompt":"$placeholder"})
                .as_object()
                .unwrap()
                .clone(),
            &paths,
            &context,
            &client,
            &unused_cogitate(root.path()),
            &mut output,
        );
        assert!(
            matches!(outcome, RuntimeOutcome::UnportedHook { ref hook, ref talent } if hook == "unknown_native_hook" && talent == "speaker-attribution-fixture")
        );
        let hook_events = events(&output);
        assert_eq!(hook_events.len(), 1);
        assert_eq!(hook_events[0]["event"], "start");
        assert!(
            !hook_events
                .iter()
                .any(|event| event.get("skip_reason").is_some())
        );
        assert!(
            !hook_events
                .iter()
                .any(|event| event.get("output") == Some(&json!("$placeholder")))
        );

        let (source_root, source_paths, source_context) = fixture(
            "schedule-source-fixture",
            r#"{
"type":"generate", "hook":{"post":"schedule"}, "load":{"transcripts":true,"percepts":false,"talents":{"screen":true}}
}"#,
        );
        let source_client =
            OneShotClient::at_path(test_support::one_shot_stub(source_root.path(), "generated"));
        let source_day = "20260101";
        let source_segment = "090000_60";
        fs::write(
            segment_dir(&source_context, source_day, source_segment).join("capture_audio.jsonl"),
            r#"{"start":"00:00:00","text":"This transcript is long enough to prepare and execute normally."}"#,
        )
        .unwrap();
        let source_request = json!({
            "name":"schedule-source-fixture", "day":source_day, "segment":source_segment, "prompt":"hello"
        })
        .as_object()
        .unwrap()
        .clone();
        let source_prepared = prepare::prepare(
            source_request.clone(),
            &source_paths,
            &source_context,
            prepare::PrepareMode::Execute,
        )
        .expect("schedule source prepares");
        assert!(
            source_prepared.config["transcript"]
                .as_str()
                .unwrap()
                .contains("long enough")
        );
        assert_eq!(
            source_prepared.config["source_counts"],
            json!({"transcripts": 1, "percepts": 0, "talents": 0})
        );
        let mut source_output = Vec::new();
        let source_outcome = execute_request(
            source_request,
            &source_paths,
            &source_context,
            &source_client,
            &unused_cogitate(source_root.path()),
            &mut source_output,
        );
        assert!(matches!(source_outcome, RuntimeOutcome::Finished { .. }));
        emit_outcome(&mut source_output, source_outcome);
        let source_events = events(&source_output);
        assert_eq!(source_events.len(), 2);
        assert_eq!(source_events[0]["event"], "start");
        assert_eq!(source_events[1]["event"], "finish");
        assert_eq!(source_events[1]["output"], "generated");
    }

    #[test]
    fn criterion_4_execute_request_still_refuses_without_a_brain() {
        let (root, paths, context) = fixture(
            "plain",
            r#"{
"type":"generate", "load":{"transcripts":false}
}"#,
        );
        fs::write(
            context.journal.join("config/journal.json"),
            r#"{"providers":{"active":{"provider":"none"}}}"#,
        )
        .unwrap();
        let client = OneShotClient::at_path(test_support::one_shot_stub(root.path(), "generated"));
        let mut output = Vec::new();
        let outcome = execute_request(
            json!({"name":"plain", "day":"20260101", "prompt":"hello"})
                .as_object()
                .unwrap()
                .clone(),
            &paths,
            &context,
            &client,
            &unused_cogitate(root.path()),
            &mut output,
        );
        let RuntimeOutcome::PrepareFailed(error) = outcome else {
            panic!("expected PrepareFailed, got {outcome:?}");
        };
        assert_eq!(
            error.to_string(),
            "No thinking engine is chosen yet. Choose one in Thinking."
        );
    }

    #[test]
    fn criterion_24_prepare_failures_are_named_outcomes() {
        let (root, paths, mut context) = fixture(
            "cwd-fixture",
            r#"{
"type":"cogitate", "cwd":"journal", "load":{"transcripts":false}
}"#,
        );
        context.journal = root.path().join("unavailable-journal");
        let client = OneShotClient::at_path(test_support::one_shot_stub(root.path(), "generated"));
        let mut output = Vec::new();
        let outcome = execute_request(
            json!({"name":"cwd-fixture", "prompt":"hello"})
                .as_object()
                .unwrap()
                .clone(),
            &paths,
            &context,
            &client,
            &unused_cogitate(root.path()),
            &mut output,
        );
        assert!(
            matches!(outcome, RuntimeOutcome::PrepareFailed(prepare::PrepareFailure::UnresolvableCwd { ref talent }) if talent == "cwd-fixture")
        );
        assert!(output.is_empty());

        fs::write(
            root.path().join("journal/config/journal.json"),
            r#"{"providers":{"active":{"provider":"none"}}}"#,
        )
        .unwrap();
        let mut no_brain = Vec::new();
        let no_brain_outcome = execute_request(
            json!({"name":"cwd-fixture", "prompt":"hello"})
                .as_object()
                .unwrap()
                .clone(),
            &paths,
            &ExecutionContext {
                journal: root.path().join("journal"),
            },
            &client,
            &unused_cogitate(root.path()),
            &mut no_brain,
        );
        assert!(matches!(
            no_brain_outcome,
            RuntimeOutcome::PrepareFailed(prepare::PrepareFailure::NoBrainConfigured)
        ));
        emit_outcome(&mut no_brain, no_brain_outcome);
        let no_brain_events = events(&no_brain);
        assert_eq!(
            no_brain_events[0]["error"],
            "No thinking engine is chosen yet. Choose one in Thinking."
        );
    }

    #[test]
    fn criterion_8_framework_failure_is_terminal_and_typed() {
        let (root, paths, context) = fixture(
            "conversation",
            r#"{
"type":"generate", "hook":{"post":"story"}, "load":{"transcripts":false}
}"#,
        );
        // Prepare now requires a real named-facet declaration. Poison only the
        // story write path so compose succeeds and commit still fails.
        let work = context.journal.join("facets/work");
        fs::create_dir_all(&work).unwrap();
        fs::write(work.join("facet.json"), r#"{"title":"Work"}"#).unwrap();
        fs::write(work.join("activities"), b"not a directory").unwrap();
        let client = OneShotClient::at_path(test_support::one_shot_stub(
            root.path(),
            r#"{"body":"body","topics":["work"],"confidence":1,"commitments":[],"closures":[],"decisions":[],"relations":[]}"#,
        ));
        let mut output = Vec::new();
        let outcome = execute_request(
            json!({
                "name":"conversation", "day":"20260101", "facet":"work",
                "activity":{"id":"activity-1"}, "prompt":"hello"
            })
            .as_object()
            .unwrap()
            .clone(),
            &paths,
            &context,
            &client,
            &unused_cogitate(root.path()),
            &mut output,
        );
        let RuntimeOutcome::StageFailed(error) = outcome else {
            panic!("terminal stage failure")
        };
        assert_eq!(error.phase, "commit");
        assert_eq!(error.stage, "story");
        assert_eq!(error.talent, "conversation");
    }

    #[test]
    fn criterion_1_and_2_and_4_generate_refusal_preserves_structured_facts() {
        let live_detail = "the configured provider could not produce a usable response";
        let cases = [
            (
                "known",
                Some("provider_response_invalid"),
                true,
                false,
                true,
                false,
                "openai",
            ),
            ("absent", None, false, true, false, true, "openai"),
            (
                "unknown",
                Some("future_code"),
                true,
                false,
                false,
                true,
                "openai",
            ),
        ];
        for (label, reason_code, stub_retryable, stub_blocking, retryable, blocking, provider) in
            cases
        {
            let (root, paths, context) = fixture(
                "plain",
                r#"{
"type":"generate", "load":{"transcripts":false}
}"#,
            );
            let client = OneShotClient::at_path(test_support::refused_one_shot_stub(
                root.path(),
                reason_code,
                stub_retryable,
                stub_blocking,
                provider,
                live_detail,
            ));
            let mut start = Vec::new();
            let outcome = execute_request(
                json!({"name":"plain", "day":"20260101", "prompt":"hello"})
                    .as_object()
                    .unwrap()
                    .clone(),
                &paths,
                &context,
                &client,
                &unused_cogitate(root.path()),
                &mut start,
            );
            let RuntimeOutcome::GenerateRefused { error, response } = &outcome else {
                panic!("{label}: expected GenerateRefused, got {outcome:?}");
            };
            assert_eq!(error.phase, "generate", "{label}");
            assert_eq!(error.stage, "runtime", "{label}");
            assert_eq!(error.talent, "plain", "{label}");
            assert_eq!(
                response.reason.as_str(),
                "provider-response-invalid",
                "{label}"
            );
            assert_eq!(response.provider.as_deref(), Some(provider), "{label}");
            assert_eq!(
                response.reason_code.as_ref().map(ReasonCodeValue::as_wire),
                reason_code,
                "{label}"
            );
            assert_eq!(response.retryable, retryable, "{label}");
            assert_eq!(response.blocking, blocking, "{label}");
            assert!(
                !response
                    .detail
                    .contains("fixture provider-response-invalid"),
                "{label}"
            );
            assert_eq!(&response.detail, live_detail, "{label}");
            assert!(
                !error
                    .to_string()
                    .contains("fixture provider-response-invalid"),
                "{label}"
            );
            let mut emitted = Vec::new();
            emit_outcome(&mut emitted, outcome);
            let output_events = events(&emitted);
            assert_eq!(output_events.len(), 1, "{label}");
            let event = &output_events[0];
            assert_eq!(event["event"], "error", "{label}");
            assert_eq!(event["terminal"], true, "{label}");
            assert_eq!(event["name"], "plain", "{label}");
            let error_text = event["error"].as_str().expect("error string");
            assert!(
                error_text.contains("generate hook 'runtime' for talent 'plain'"),
                "{label}: {error_text}"
            );
            assert!(error_text.contains(live_detail), "{label}: {error_text}");
            assert!(
                !error_text.contains("fixture provider-response-invalid"),
                "{label}"
            );
            assert_eq!(event["reason"], "provider-response-invalid", "{label}");
            match reason_code {
                Some(code) => assert_eq!(event["reason_code"], code, "{label}"),
                None => assert!(event["reason_code"].is_null(), "{label}"),
            }
            assert_eq!(event["retryable"], retryable, "{label}");
            assert_eq!(event["blocking"], blocking, "{label}");
            assert_eq!(event["provider"], provider, "{label}");
        }
    }

    #[test]
    fn criterion_1_runtime_manifest_has_no_axum_dependency() {
        assert!(
            !include_str!("../Cargo.toml")
                .lines()
                .any(|line| line.trim_start().starts_with("axum"))
        );
    }

    #[test]
    fn criterion_3_required_percepts_are_enabled_and_gathered() {
        let (_root, paths, context) = fixture(
            "required-percepts",
            r#"{
"type":"generate", "load":{"percepts":"required"}
}"#,
        );
        let day = "20260102";
        let segment = "090000_60";
        fs::write(
            segment_dir(&context, day, segment).join("screen.jsonl"),
            r#"{"timestamp":0,"content":{"window":"Enough recorded percept text to prepare this talent normally."}}"#,
        )
        .unwrap();

        let prepared = prepare::prepare(
            json!({"name":"required-percepts", "day":day, "segment":segment, "prompt":"hello"})
                .as_object()
                .unwrap()
                .clone(),
            &paths,
            &context,
            prepare::PrepareMode::Execute,
        )
        .unwrap();

        assert!(prepared.config.get("skip_reason").is_none());
        assert!(
            prepared.config["transcript"]
                .as_str()
                .unwrap()
                .contains("Screen Activity")
        );
        assert_eq!(prepared.config["source_counts"]["percepts"], 1);
    }

    #[test]
    fn empty_day_does_not_gather_or_skip() {
        let (_root, paths, context) = fixture(
            "empty-day",
            r#"{
"type":"generate", "load":{"transcripts":true}
}"#,
        );

        let prepared = prepare::prepare(
            json!({"name":"empty-day", "day":"", "prompt":"hello"})
                .as_object()
                .unwrap()
                .clone(),
            &paths,
            &context,
            prepare::PrepareMode::Execute,
        )
        .unwrap();

        assert!(prepared.config.get("transcript").is_none());
        assert!(prepared.config.get("source_counts").is_none());
        assert!(prepared.config.get("skip_reason").is_none());
    }

    #[test]
    fn criterion_9_gate_does_not_probe_when_no_source_is_enabled() {
        let root = tempfile::tempdir().unwrap();
        let sources =
            source_config(json!({"transcripts": false, "percepts": false, "talents": false}));

        assert!(!check_segment_has_no_input(
            root.path(),
            "20260103",
            "090000_60",
            None,
            &sources,
            Utc::now(),
        ));
        assert!(!root.path().join("chronicle").exists());
    }

    #[test]
    fn criterion_9_gate_returns_true_for_content_emptiness() {
        let root = tempfile::tempdir().unwrap();
        let sources = source_config(json!({"transcripts": true}));

        assert!(check_segment_has_no_input(
            root.path(),
            "20260103",
            "090000_60",
            None,
            &sources,
            Utc::now(),
        ));
    }

    #[test]
    fn criterion_9_gate_returns_true_for_nonempty_all_empty_data_state() {
        let root = tempfile::tempdir().unwrap();
        let context = ExecutionContext {
            journal: root.path().to_path_buf(),
        };
        let path = segment_dir(&context, "20260103", "090000_60");
        fs::write(
            path.join("audio.jsonl"),
            r#"{"_solstone_processing":{"state":"empty"}}"#,
        )
        .unwrap();
        fs::create_dir_all(path.join("talents")).unwrap();
        fs::write(
            path.join("talents/sense.md"),
            "This talent output is deliberately long enough to avoid the content emptiness gate.",
        )
        .unwrap();
        let sources = source_config(json!({"talents": true}));

        assert!(check_segment_has_no_input(
            &context.journal,
            "20260103",
            "090000_60",
            None,
            &sources,
            Utc::now(),
        ));
    }

    #[test]
    fn criterion_12_public_gate_keeps_talent_only_input_when_data_state_is_empty() {
        let root = tempfile::tempdir().unwrap();
        let context = ExecutionContext {
            journal: root.path().to_path_buf(),
        };
        let path = segment_dir(&context, "20260103", "090000_60");
        fs::create_dir_all(path.join("talents")).unwrap();
        fs::write(
            path.join("talents/sense.md"),
            "This talent output is deliberately long enough to avoid the content emptiness gate.",
        )
        .unwrap();
        let sources = source_config(json!({"talents": true}));

        assert!(!check_segment_has_no_input(
            &context.journal,
            "20260103",
            "090000_60",
            None,
            &sources,
            Utc::now(),
        ));
    }

    #[test]
    fn criterion_16_talent_filter_reaches_the_talents_count_key() {
        let (_root, paths, context) = fixture(
            "filtered-talents",
            r#"{
"type":"generate", "load":{"talents":{"app:name":true}}
}"#,
        );
        let day = "20260104";
        let segment = "090000_60";
        let path = segment_dir(&context, day, segment);
        fs::create_dir_all(path.join("talents")).unwrap();
        fs::write(
            path.join("talents/_app_name.md"),
            "This selected talent output is long enough for the gather to retain it.",
        )
        .unwrap();
        fs::write(
            path.join("talents/other.md"),
            "This other talent output must not appear in the gathered transcript.",
        )
        .unwrap();

        let prepared = prepare::prepare(
            json!({"name":"filtered-talents", "day":day, "segment":segment, "prompt":"hello"})
                .as_object()
                .unwrap()
                .clone(),
            &paths,
            &context,
            prepare::PrepareMode::Execute,
        )
        .unwrap();

        assert_eq!(
            prepared.config["sources"]["talents"],
            json!({"app:name":true})
        );
        assert!(
            prepared.config["transcript"]
                .as_str()
                .unwrap()
                .contains("### _app_name summary")
        );
        assert!(
            !prepared.config["transcript"]
                .as_str()
                .unwrap()
                .contains("other summary")
        );
        assert_eq!(prepared.config["source_counts"]["talents"], 1);
    }

    #[test]
    fn criterion_18_required_source_skip_keeps_gathered_counts() {
        let (_root, paths, context) = fixture(
            "required-missing",
            r#"{
"type":"generate", "load":{"percepts":"required"}
}"#,
        );
        let day = "20260105";
        let segment = "090000_60";
        segment_dir(&context, day, segment);

        let prepared = prepare::prepare(
            json!({"name":"required-missing", "day":day, "segment":segment, "prompt":"hello"})
                .as_object()
                .unwrap()
                .clone(),
            &paths,
            &context,
            prepare::PrepareMode::Execute,
        )
        .unwrap();

        assert_eq!(prepared.config["skip_reason"], "missing_required_percepts");
        assert_eq!(
            prepared.config["source_counts"],
            json!({"transcripts":0,"percepts":0,"talents":0})
        );
    }

    #[test]
    fn criterion_18_empty_gather_skips_with_no_input() {
        let (_root, paths, context) = fixture(
            "empty-gather",
            r#"{
"type":"generate", "load":{"transcripts":true}
}"#,
        );
        let prepared = prepare::prepare(
            json!({"name":"empty-gather", "day":"20260105", "segment":"090000_60", "prompt":"hello"})
                .as_object()
                .unwrap()
                .clone(),
            &paths,
            &context,
            prepare::PrepareMode::Execute,
        )
        .unwrap();

        assert_eq!(prepared.config["skip_reason"], "no_input");
        assert_eq!(
            prepared.config["transcript"],
            "Segment folder not found: 20260105/090000_60"
        );
    }

    #[test]
    fn criterion_18_sparse_gather_prepends_the_exact_input_note() {
        let (_root, paths, context) = fixture(
            "sparse-gather",
            r#"{
"type":"generate", "load":{"transcripts":true}
}"#,
        );
        let day = "20260105";
        let segment = "090000_60";
        fs::write(
            segment_dir(&context, day, segment).join("capture_audio.jsonl"),
            r#"{"start":"00:00:00","text":"This single transcript entry is long enough to avoid the no input skip."}"#,
        )
        .unwrap();
        let prepared = prepare::prepare(
            json!({"name":"sparse-gather", "day":day, "segment":segment, "prompt":"hello"})
                .as_object()
                .unwrap()
                .clone(),
            &paths,
            &context,
            prepare::PrepareMode::Execute,
        )
        .unwrap();

        assert_eq!(prepared.config["source_counts"]["transcripts"], 1);
        assert!(prepared.config["transcript"]
            .as_str()
            .unwrap()
            .starts_with("**Input Note:** Limited recordings for this day. Scale analysis to available input.\n\n"));
    }

    #[test]
    fn criterion_18_prepare_skip_emits_start_before_finish() {
        let (root, paths, context) = fixture(
            "ordered-skip",
            r#"{
"type":"generate", "load":{"transcripts":true}
}"#,
        );
        let client = OneShotClient::at_path(test_support::one_shot_stub(root.path(), "generated"));
        let mut output = Vec::new();
        run_lines(
            Cursor::new(
                "{\"name\":\"ordered-skip\",\"day\":\"20260105\",\"segment\":\"090000_60\",\"prompt\":\"hello\"}\n",
            ),
            &mut output,
            &paths,
            &context,
            Ok(&client),
            Ok(&unused_cogitate(root.path())),
        );

        let output_events = events(&output);
        assert_eq!(output_events.len(), 2);
        assert_eq!(output_events[0]["event"], "start");
        assert_eq!(
            output_events[1],
            json!({"event":"finish", "name":"ordered-skip", "skip_reason":"no_input"})
        );
    }

    #[test]
    fn share_layout_resolves_and_fails_when_anchor_removed() {
        let root = tempfile::tempdir().unwrap();
        let prefix = root.path().join("tree");
        let bin = prefix.join("bin");
        let share = prefix.join("share");
        fs::create_dir_all(&bin).unwrap();
        for relative in [
            solstone_core_journal::LAYOUT_BUNDLE_ANCHOR,
            solstone_core_journal::LAYOUT_LAYOUT_ANCHOR,
            solstone_core_journal::LAYOUT_TEMPLATE_ANCHOR,
        ] {
            let path = share.join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, relative).unwrap();
        }
        let paths = runtime_paths_from_executable_dir(&bin).unwrap();
        assert_eq!(paths.talent_root, share.join("solstone/talent"));
        assert_eq!(paths.apps_root, share.join("solstone/apps"));
        fs::remove_file(share.join(solstone_core_journal::LAYOUT_LAYOUT_ANCHOR)).unwrap();
        assert!(runtime_paths_from_executable_dir(&bin).is_err());
    }
}
