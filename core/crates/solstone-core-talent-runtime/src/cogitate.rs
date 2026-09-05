// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::{Map, Value, json};
use solstone_core_cogitate::{ReadScopeConfig, ReadScopeError, resolve_read_scope};
use solstone_core_cogitate_wire::{CogitateRequest, REQUEST_SCHEMA};

use crate::{ExecutionContext, PreparedTalent, RuntimeOutcome, StageError};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EngineKind {
    Generate,
    Cogitate,
}

pub fn from_prepared_config(config: &Map<String, Value>) -> Result<EngineKind, RuntimeOutcome> {
    match config.get("type").and_then(Value::as_str).unwrap_or("") {
        "" | "generate" => Ok(EngineKind::Generate),
        "cogitate" => Ok(EngineKind::Cogitate),
        other => Err(failed_config(
            config,
            format!("unknown talent type {other:?}"),
        )),
    }
}

pub fn cogitate_request(
    prepared: &PreparedTalent,
    context: &ExecutionContext,
) -> Result<CogitateRequest, RuntimeOutcome> {
    if !context.journal.is_absolute() {
        return Err(failed(prepared, "journal_root must be an absolute path"));
    }
    let Some(correlation_id) = non_empty(prepared.config.get("use_id")) else {
        return Err(failed(prepared, "use_id is required"));
    };
    let user_instruction = non_empty(prepared.config.get("user_instruction"));
    let prompt = non_empty(prepared.config.get("prompt"));
    let Some(initial_prompt) = prompt.or(user_instruction.clone()) else {
        return Err(failed(
            prepared,
            "Cogitate talent requires non-empty 'prompt' or 'user_instruction'",
        ));
    };
    let mut object = Map::new();
    object.insert("schema".to_owned(), json!(REQUEST_SCHEMA));
    object.insert(
        "access_tier".to_owned(),
        json!(non_empty(prepared.config.get("access_tier")).unwrap_or_else(|| "normal".to_owned())),
    );
    insert_optional_string(
        &mut object,
        "outbound_approval",
        prepared.config.get("outbound_approval"),
    );
    object.insert(
        "diagnostic".to_owned(),
        json!(
            prepared
                .config
                .get("diagnostic")
                .and_then(Value::as_bool)
                .unwrap_or(false)
        ),
    );
    if let Some(instruction) = user_instruction {
        object.insert("talent_instruction".to_owned(), json!(instruction));
    }
    object.insert(
        "sol_tool_name".to_owned(),
        json!(
            non_empty(prepared.config.get("sol_tool_name"))
                .unwrap_or_else(|| "solstone".to_owned())
        ),
    );
    object.insert("read_scope".to_owned(), json!(read_scope_for(prepared)?));
    insert_optional_string(
        &mut object,
        "output_path",
        prepared.config.get("output_path"),
    );
    insert_optional_string(&mut object, "schedule", prepared.config.get("schedule"));
    object.insert(
        "max_turns".to_owned(),
        json!(positive_usize(prepared.config.get("max_turns")).unwrap_or(60)),
    );
    if let Some(window) = positive_u64(prepared.config.get("context_window")) {
        object.insert("context_window".to_owned(), json!(window));
    }
    object.insert(
        "timeout_ms".to_owned(),
        json!(
            positive_u64(prepared.config.get("timeout_seconds"))
                .and_then(|seconds| seconds.checked_mul(1000))
                .unwrap_or(600_000)
        ),
    );
    object.insert(
        "read_call_budget".to_owned(),
        json!(positive_i64(prepared.config.get("read_call_budget")).unwrap_or(200)),
    );
    object.insert(
        "model".to_owned(),
        json!(
            prepared
                .config
                .get("model")
                .and_then(Value::as_str)
                .unwrap_or("")
        ),
    );
    object.insert("correlation_id".to_owned(), json!(correlation_id));
    object.insert("initial_prompt".to_owned(), json!(initial_prompt));
    object.insert(
        "journal_root".to_owned(),
        json!(context.journal.to_string_lossy()),
    );
    object.insert(
        "dry_run".to_owned(),
        json!(
            prepared
                .config
                .get("dry_run")
                .and_then(Value::as_bool)
                .unwrap_or(false)
        ),
    );
    CogitateRequest::from_value(&Value::Object(object))
        .map_err(|error| failed(prepared, error.to_string()))
}

fn read_scope_for(prepared: &PreparedTalent) -> Result<Vec<String>, RuntimeOutcome> {
    let Some(day) = non_empty(prepared.config.get("day")) else {
        return Ok(Vec::new());
    };
    let explicit = prepared
        .config
        .get("read_scope")
        .and_then(Value::as_array)
        .and_then(|values| {
            values
                .iter()
                .map(|value| value.as_str().map(str::to_owned))
                .collect::<Option<Vec<_>>>()
        });
    resolve_read_scope(
        ReadScopeConfig {
            read_scope: explicit.as_deref(),
            read_scope_span: prepared
                .config
                .get("read_scope_span")
                .and_then(Value::as_i64),
        },
        &day,
        prepared
            .config
            .get("span")
            .and_then(Value::as_i64)
            .unwrap_or(0),
    )
    .map_err(|error| match error {
        ReadScopeError::InvalidDay(day) => failed(prepared, format!("invalid day {day}")),
    })
}

fn insert_optional_string(object: &mut Map<String, Value>, field: &str, value: Option<&Value>) {
    if let Some(value) = non_empty(value) {
        object.insert(field.to_owned(), json!(value));
    }
}

fn non_empty(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn positive_usize(value: Option<&Value>) -> Option<usize> {
    value
        .and_then(Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
        .filter(|value| *value > 0)
}

fn positive_u64(value: Option<&Value>) -> Option<u64> {
    value.and_then(Value::as_u64).filter(|value| *value > 0)
}

fn positive_i64(value: Option<&Value>) -> Option<i64> {
    value.and_then(Value::as_i64).filter(|value| *value > 0)
}

fn failed(prepared: &PreparedTalent, detail: impl Into<String>) -> RuntimeOutcome {
    failed_named(&prepared.name, detail)
}

fn failed_config(config: &Map<String, Value>, detail: impl Into<String>) -> RuntimeOutcome {
    failed_named(
        config
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("unknown"),
        detail,
    )
}

fn failed_named(talent: &str, detail: impl Into<String>) -> RuntimeOutcome {
    RuntimeOutcome::StageFailed(StageError {
        phase: "cogitate",
        stage: "runtime",
        talent: talent.to_owned(),
        detail: detail.into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    fn payload_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(3)
            .expect("talent runtime crate is nested under the repository root")
            .join("core/payload/solstone")
    }

    fn prepared(config: Map<String, Value>) -> PreparedTalent {
        let name = config
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("demo")
            .to_owned();
        PreparedTalent { name, config }
    }

    fn context(journal: PathBuf) -> ExecutionContext {
        ExecutionContext { journal }
    }

    #[test]
    fn engine_kind_reads_shipped_cogitate_and_generate_types() {
        let root = payload_root();
        let configs =
            solstone_core_talent_config::discover(&root.join("talent"), &root.join("apps"))
                .expect("discover shipped talent corpus");
        for key in ["weekly_reflection", "partner", "entities:entity_assist"] {
            let config = configs
                .iter()
                .find(|config| config.key == key)
                .unwrap_or_else(|| panic!("missing shipped talent {key}"));
            assert_eq!(
                from_prepared_config(&config.metadata).unwrap(),
                EngineKind::Cogitate,
                "{key}"
            );
        }
        assert_eq!(
            from_prepared_config(&Map::from_iter([("type".to_owned(), json!("generate"))]))
                .unwrap(),
            EngineKind::Generate
        );
        assert_eq!(
            from_prepared_config(&Map::new()).unwrap(),
            EngineKind::Generate
        );
        let RuntimeOutcome::StageFailed(error) = from_prepared_config(&Map::from_iter([
            ("type".to_owned(), json!("invented")),
            ("name".to_owned(), json!("demo")),
        ]))
        .unwrap_err() else {
            panic!("expected StageFailed");
        };
        assert_eq!(error.talent, "demo");
        assert!(error.detail.contains("invented"));
    }

    #[test]
    fn weekly_reflection_read_scope_delegates_to_resolve_read_scope() {
        let weekly = prepared(Map::from_iter([
            ("name".to_owned(), json!("weekly_reflection")),
            ("day".to_owned(), json!("20260809")),
            ("read_scope_span".to_owned(), json!(7)),
            ("use_id".to_owned(), json!("u1")),
            ("prompt".to_owned(), json!("hello")),
        ]));
        let expected = resolve_read_scope(
            ReadScopeConfig {
                read_scope: None,
                read_scope_span: Some(7),
            },
            "20260809",
            0,
        )
        .unwrap();
        assert_eq!(read_scope_for(&weekly).unwrap(), expected);
        let assist = prepared(Map::from_iter([(
            "name".to_owned(),
            json!("entities:entity_assist"),
        )]));
        assert_eq!(read_scope_for(&assist).unwrap(), Vec::<String>::new());
    }

    fn translate(mut config: Map<String, Value>) -> CogitateRequest {
        config
            .entry("use_id".to_owned())
            .or_insert_with(|| json!("1750000000001"));
        config
            .entry("prompt".to_owned())
            .or_insert_with(|| json!("run"));
        let journal = PathBuf::from("/var/tmp/solstone-cogitate-translate");
        cogitate_request(&prepared(config), &context(journal)).unwrap()
    }

    #[test]
    fn translator_maps_shipped_config_shapes() {
        let weekly = translate(Map::from_iter([
            ("name".to_owned(), json!("weekly_reflection")),
            ("access_tier".to_owned(), json!("synthesis")),
            ("schedule".to_owned(), json!("weekly")),
            (
                "output_path".to_owned(),
                json!("/var/tmp/solstone-journal/reflections/weekly/20260809.md"),
            ),
            ("day".to_owned(), json!("20260809")),
            ("read_scope_span".to_owned(), json!(7)),
            ("max_turns".to_owned(), json!(100)),
            ("max_run_cost_usd".to_owned(), json!(5.0)),
            ("user_instruction".to_owned(), json!("weekly body")),
            (
                "prompt".to_owned(),
                json!("Running scheduled weekly reflection for 2026-08-09: No recordings."),
            ),
            ("model".to_owned(), json!("test-model")),
        ]));
        assert_eq!(weekly.access_tier, "synthesis");
        assert_eq!(weekly.schedule.as_deref(), Some("weekly"));
        assert_eq!(
            weekly.output_path.as_deref(),
            Some("/var/tmp/solstone-journal/reflections/weekly/20260809.md")
        );
        assert!(!weekly.diagnostic);
        assert_eq!(
            weekly.initial_prompt,
            "Running scheduled weekly reflection for 2026-08-09: No recordings."
        );
        assert_eq!(weekly.talent_instruction.as_deref(), Some("weekly body"));
        assert_eq!(weekly.max_turns, 100);
        assert_eq!(weekly.timeout_ms, 600_000);
        assert_eq!(weekly.read_call_budget, 200);
        assert_eq!(weekly.sol_tool_name.as_deref(), Some("solstone"));
        assert_eq!(weekly.correlation_id, "1750000000001");
        assert_eq!(
            weekly.read_scope,
            resolve_read_scope(
                ReadScopeConfig {
                    read_scope: None,
                    read_scope_span: Some(7),
                },
                "20260809",
                0,
            )
            .unwrap()
        );
        assert!(weekly.to_run_input().config.expects_emit_final);

        let partner = translate(Map::from_iter([
            ("name".to_owned(), json!("partner")),
            ("access_tier".to_owned(), json!("synthesis")),
            ("schedule".to_owned(), json!("weekly")),
            ("day".to_owned(), json!("20260813")),
            ("max_turns".to_owned(), json!(100)),
            ("user_instruction".to_owned(), json!("partner body")),
            (
                "prompt".to_owned(),
                json!("Running scheduled task for 2026-08-13: No recordings."),
            ),
        ]));
        assert_eq!(partner.access_tier, "synthesis");
        assert_eq!(partner.schedule.as_deref(), Some("weekly"));
        assert_eq!(partner.output_path, None);
        assert_eq!(partner.max_turns, 100);
        assert_eq!(partner.read_scope, ["chronicle/20260813"]);
        assert!(!partner.diagnostic);
        assert!(partner.to_run_input().config.expects_emit_final);

        let assist = translate(Map::from_iter([
            ("name".to_owned(), json!("entities:entity_assist")),
            ("user_instruction".to_owned(), json!("assist body")),
            ("prompt".to_owned(), json!("add Alice Chen as a person")),
        ]));
        assert_eq!(assist.access_tier, "normal");
        assert_eq!(assist.schedule, None);
        assert_eq!(assist.output_path, None);
        assert_eq!(assist.max_turns, 60);
        assert!(assist.read_scope.is_empty());
        assert_eq!(assist.initial_prompt, "add Alice Chen as a person");
        assert!(!assist.to_run_input().config.expects_emit_final);
    }

    #[test]
    fn correlation_id_is_the_request_use_id() {
        let journal = PathBuf::from("/var/tmp/solstone-cogitate-translate");
        let one = cogitate_request(
            &prepared(Map::from_iter([
                ("use_id".to_owned(), json!("use-a")),
                ("prompt".to_owned(), json!("hello")),
            ])),
            &context(journal.clone()),
        )
        .unwrap();
        let two = cogitate_request(
            &prepared(Map::from_iter([
                ("use_id".to_owned(), json!("use-b")),
                ("prompt".to_owned(), json!("hello")),
            ])),
            &context(journal),
        )
        .unwrap();
        assert_eq!(one.correlation_id, "use-a");
        assert_eq!(two.correlation_id, "use-b");
    }

    #[test]
    fn translator_failures_are_stage_failed() {
        let journal = PathBuf::from("/var/tmp/solstone-cogitate-translate");
        let cases = [
            (
                Map::from_iter([("use_id".to_owned(), json!("u1"))]),
                "prompt",
            ),
            (
                Map::from_iter([
                    ("use_id".to_owned(), json!("u1")),
                    ("prompt".to_owned(), json!("hello")),
                ]),
                "absolute",
            ),
            (
                Map::from_iter([("prompt".to_owned(), json!("hello"))]),
                "use_id",
            ),
            (
                Map::from_iter([
                    ("use_id".to_owned(), json!("")),
                    ("prompt".to_owned(), json!("hello")),
                ]),
                "use_id",
            ),
        ];
        for (config, needle) in cases {
            let ctx = if needle == "absolute" {
                context(PathBuf::from("relative-journal"))
            } else {
                context(journal.clone())
            };
            let RuntimeOutcome::StageFailed(error) =
                cogitate_request(&prepared(config), &ctx).unwrap_err()
            else {
                panic!("expected StageFailed");
            };
            assert_eq!(error.phase, "cogitate");
            assert!(
                error.detail.to_lowercase().contains(needle) || error.detail.contains("prompt"),
                "{}",
                error.detail
            );
        }
        let RuntimeOutcome::StageFailed(error) = cogitate_request(
            &prepared(Map::from_iter([
                ("use_id".to_owned(), json!("u1")),
                ("prompt".to_owned(), json!("hello")),
                ("access_tier".to_owned(), json!("invented")),
            ])),
            &context(journal),
        )
        .unwrap_err() else {
            panic!("expected StageFailed");
        };
        assert!(error.detail.contains("access_tier"), "{}", error.detail);
    }
}
