// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::{Map, Value};

use solstone_core_generate::ContentPart;
use solstone_core_journal_io::{PathOrDay, iter_segments};
use solstone_core_talent_cli::preview::{PreviewRequest, PromptPreview, PromptPreviewRefusal};

use crate::activity_contract::{self, SpanFailure};
use crate::contract::{GateDecision, resolve_hook};
use crate::prepare::{PrepareMode, RuntimePaths, prepare, resolve_validated_talent_config};
use crate::transcript::sources_are_enabled;
use crate::{DRY_RUN_KEY, ExecutionContext, RuntimeOutcome, generate_contents};

pub fn assemble_prompt_preview(
    request: &PreviewRequest,
    paths: &RuntimePaths,
    context: &ExecutionContext,
) -> PromptPreview {
    let activity = match resolve_activity(request, paths, context) {
        Ok(activity) => activity,
        Err(refusal) => return PromptPreview::Refused(refusal),
    };
    let mut payload = Map::from_iter([("name".to_owned(), Value::String(request.name.clone()))]);
    if let Some(day) = &request.day {
        payload.insert("day".to_owned(), Value::String(day.clone()));
    }
    if let Some(segment) = &request.segment {
        payload.insert("segment".to_owned(), Value::String(segment.clone()));
    }
    if let Some(facet) = &request.facet {
        payload.insert("facet".to_owned(), Value::String(facet.clone()));
    }
    if let Some(activity) = activity {
        let ResolvedActivity {
            record,
            span,
            prompt,
        } = activity;
        payload.insert("activity".to_owned(), Value::Object(record));
        payload.insert(
            "span".to_owned(),
            Value::Array(span.into_iter().map(Value::String).collect()),
        );
        if let Some(prompt) = prompt {
            payload.insert("prompt".to_owned(), Value::String(prompt));
        }
    }

    let mut prepared = match prepare(payload, paths, context, PrepareMode::Preview) {
        Ok(prepared) => prepared,
        Err(error) => {
            return preview_failure(request, "activity_talent_unavailable", error.to_string());
        }
    };
    if let Some(reason) = prepared
        .config
        .get("skip_reason")
        .and_then(Value::as_str)
        .filter(|reason| !reason.is_empty())
    {
        return would_not_run(request, reason);
    }

    let hook = prepared
        .config
        .get("hook")
        .and_then(Value::as_object)
        .and_then(|hook| hook.get("pre"))
        .and_then(Value::as_str);
    if let Some(hook) = hook {
        let Some(stage) = resolve_hook(hook) else {
            return unavailable_prestep(request);
        };
        prepared
            .config
            .insert(DRY_RUN_KEY.to_owned(), Value::Bool(true));
        if let Some(gate) = stage.gate {
            match gate(&prepared, context) {
                Ok(GateDecision::Proceed) => {}
                Ok(GateDecision::Skip(reason)) => {
                    return would_not_run(request, &reason);
                }
                Err(error) => {
                    return preview_failure(request, "activity_preview_failed", error.to_string());
                }
            }
        }
        let state = match stage.build {
            Some(build) => match build(&mut prepared, context) {
                Ok(state) => state,
                Err(RuntimeOutcome::Skipped { reason, .. }) => {
                    return would_not_run(request, &reason);
                }
                Err(RuntimeOutcome::StageFailed(error)) => {
                    return preview_failure(request, "activity_preview_failed", error.to_string());
                }
                Err(_) => unreachable!("a BuildFn can only return Skipped or StageFailed"),
            },
            None => crate::contract::PrePostState::None,
        };
        if let Some(override_prompt) = stage.prompt_override
            && let Err(error) = override_prompt(&mut prepared, &state)
        {
            return preview_failure(request, "activity_preview_failed", error.to_string());
        }
    }

    let parts = if prepared.config.get("type").and_then(Value::as_str) == Some("cogitate") {
        cogitate_contents(&prepared.config)
    } else {
        generate_contents(&prepared)
            .into_iter()
            .filter_map(|part| match part {
                ContentPart::Text { text } => Some(text),
                ContentPart::Image { .. } => None,
            })
            .collect()
    };
    let loads_sources = prepared
        .config
        .get("sources")
        .and_then(Value::as_object)
        .is_some_and(sources_are_enabled);
    PromptPreview::Assembled {
        access_tier: prepared
            .config
            .get("access_tier")
            .and_then(Value::as_str)
            .map(str::to_owned),
        loads_sources,
        parts,
    }
}

fn cogitate_contents(config: &Map<String, Value>) -> Vec<String> {
    ["prompt", "user_instruction"]
        .into_iter()
        .find_map(|key| {
            config
                .get(key)
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
        })
        .map(|value| vec![value.to_owned()])
        .unwrap_or_else(|| vec!["No input provided.".to_owned()])
}

fn preview_failure(
    request: &PreviewRequest,
    code: &str,
    error: impl Into<String>,
) -> PromptPreview {
    let error = error.into();
    if request.activity.is_some() {
        PromptPreview::Refused(refusal(
            code,
            None,
            format!("Correct the activity preview input, then retry: {error}"),
        ))
    } else {
        PromptPreview::Failed { error }
    }
}

fn unavailable_prestep(request: &PreviewRequest) -> PromptPreview {
    if request.activity.is_some() {
        PromptPreview::Refused(refusal(
            "activity_prestep_unavailable",
            None,
            "Choose a talent whose required pre-step is available in the native runtime.",
        ))
    } else {
        PromptPreview::UnavailablePreStep
    }
}

fn would_not_run(request: &PreviewRequest, reason: &str) -> PromptPreview {
    if request.activity.is_some() {
        PromptPreview::Refused(refusal(
            reason,
            None,
            "Make the selected activity and talent eligible, then retry.",
        ))
    } else {
        PromptPreview::WouldNotRun {
            reason: reason.to_owned(),
        }
    }
}

struct ResolvedActivity {
    record: Map<String, Value>,
    span: Vec<String>,
    prompt: Option<String>,
}

fn resolve_activity(
    request: &PreviewRequest,
    paths: &RuntimePaths,
    context: &ExecutionContext,
) -> Result<Option<ResolvedActivity>, PromptPreviewRefusal> {
    let Some(activity_id) = request.activity.as_deref() else {
        return Ok(None);
    };
    let day = request
        .day
        .as_deref()
        .filter(|day| !day.is_empty())
        .ok_or_else(|| {
            refusal(
                "activity_requires_day",
                None,
                "Pass --day YYYYMMDD with --activity.",
            )
        })?;
    let facet = request
        .facet
        .as_deref()
        .filter(|facet| !facet.is_empty())
        .ok_or_else(|| {
            refusal(
                "activity_requires_facet",
                None,
                "Pass --facet NAME with --activity.",
            )
        })?;
    if request.segment.is_some() {
        return Err(refusal(
            "activity_segment_conflict",
            request.segment.clone(),
            "Choose either --activity or --segment, not both.",
        ));
    }

    let config =
        resolve_validated_talent_config(&request.name, paths, context).map_err(|error| {
            refusal(
                "activity_talent_unavailable",
                None,
                format!("Correct the talent configuration: {error}"),
            )
        })?;
    if config
        .metadata
        .get("skip_reason")
        .and_then(Value::as_str)
        .filter(|reason| !reason.is_empty())
        .is_some()
        || config
            .metadata
            .get("disabled")
            .is_some_and(solstone_core_talent_config::is_truthy)
    {
        return Err(refusal(
            "disabled",
            None,
            "Make the selected talent eligible, then retry.",
        ));
    }
    if config.metadata.get("schedule").and_then(Value::as_str) != Some("activity") {
        return Err(refusal(
            "activity_schedule_unsupported",
            None,
            "Choose a talent whose schedule is activity.",
        ));
    }

    let rows = solstone_core_facets::load_activity_records(&context.journal, facet, day, true)
        .map_err(|error| {
            refusal(
                "activity_record_unavailable",
                None,
                format!("Restore a readable activity record file, then retry: {error}"),
            )
        })?;
    let matches = rows
        .into_iter()
        .filter(|row| row.get("id").and_then(Value::as_str) == Some(activity_id))
        .collect::<Vec<_>>();
    let record = match matches.as_slice() {
        [] => {
            return Err(refusal(
                "activity_not_found",
                None,
                "Choose an activity ID stored in the requested day and facet.",
            ));
        }
        [record] => record.clone(),
        _ => {
            return Err(refusal(
                "activity_ambiguous",
                None,
                "Remove duplicate records with this ID in the requested day and facet.",
            ));
        }
    };
    if activity_contract::is_synthetic(&record) {
        return Err(refusal(
            "activity_synthetic",
            None,
            "Choose a completed owner activity, not an anticipated or cogitate record.",
        ));
    }
    let span = activity_contract::validated_span(&record).map_err(|failure| match failure {
        SpanFailure::Empty => refusal(
            "activity_span_empty",
            None,
            "Choose an activity with at least one recorded segment.",
        ),
        SpanFailure::Invalid => refusal(
            "activity_span_invalid",
            None,
            "Repair the activity span so every segment is a non-empty string.",
        ),
    })?;
    let kind = record
        .get("activity")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if !activity_contract::matches_activity(&config.metadata, kind) {
        return Err(refusal(
            "activity_kind_unsupported",
            None,
            "Choose a talent configured for this activity kind.",
        ));
    }
    if activity_contract::skips_low_level_work(&request.name, kind, &record) {
        return Err(refusal(
            "low_level_activity",
            None,
            "Work does not run for browsing or reading below level_avg 0.4.",
        ));
    }
    let prompt = (!activity_contract::is_explicit_generate(&config.metadata))
        .then(|| activity_contract::cogitate_prompt(activity_id, kind, facet, day));

    let available = iter_segments(&context.journal, PathOrDay::Day(day)).map_err(|error| {
        refusal(
            "activity_segment_unavailable",
            span.first().cloned(),
            format!("Restore readable segment directories, then retry: {error}"),
        )
    })?;
    for segment in &span {
        let Some(candidate) = available
            .iter()
            .find(|candidate| candidate.key() == segment)
        else {
            return Err(refusal(
                "activity_segment_unavailable",
                Some(segment.clone()),
                "Restore the selected activity segment, then retry.",
            ));
        };
        let entries = std::fs::read_dir(candidate.path()).map_err(|error| {
            refusal(
                "activity_segment_unavailable",
                Some(segment.clone()),
                format!("Restore a readable segment directory, then retry: {error}"),
            )
        })?;
        for entry in entries {
            entry.map_err(|error| {
                refusal(
                    "activity_segment_unavailable",
                    Some(segment.clone()),
                    format!("Restore a readable segment directory, then retry: {error}"),
                )
            })?;
        }
    }
    Ok(Some(ResolvedActivity {
        record,
        span,
        prompt,
    }))
}

fn refusal(
    code: impl Into<String>,
    segment: Option<String>,
    recovery: impl Into<String>,
) -> PromptPreviewRefusal {
    PromptPreviewRefusal {
        code: code.into(),
        segment,
        recovery: recovery.into(),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    use serde_json::json;

    use super::*;

    #[test]
    fn assemble_source_has_no_process_spawn() {
        // Criterion D8: preview assemble must not spawn a model process.
        let source = include_str!("assemble.rs");
        let command_new = ["Command", "::new"].concat();
        assert!(!source.contains(&command_new));
    }

    #[test]
    fn activity_preview_contents_match_the_real_execute_provider_request() {
        let root = tempfile::tempdir().expect("root");
        let journal = root.path().join("journal");
        let talent_root = root.path().join("talent");
        let apps_root = root.path().join("apps");
        let templates_dir = root.path().join("templates");
        for directory in [
            journal.join("config"),
            journal.join("facets/work/activities"),
            journal.join("chronicle/20260101/090000_60"),
            talent_root.clone(),
            apps_root.clone(),
            templates_dir.clone(),
        ] {
            fs::create_dir_all(directory).expect("fixture directory");
        }
        fs::write(
            journal.join("config/journal.json"),
            r#"{"providers":{"active":{"provider":"test","model":"test-model"}}}"#,
        )
        .expect("journal config");
        fs::write(
            journal.join("facets/work/facet.json"),
            r#"{"name":"work","description":"Work"}"#,
        )
        .expect("facet");
        let record = json!({
            "id":"activity-a",
            "activity":"work",
            "segments":["090000_60"]
        });
        fs::write(
            journal.join("facets/work/activities/20260101.jsonl"),
            format!("{record}\n"),
        )
        .expect("activity");
        fs::write(
            journal.join("chronicle/20260101/090000_60/imported.md"),
            "provider-spy-source",
        )
        .expect("source");
        fs::write(
            talent_root.join("probe.md"),
            concat!(
                "{\n",
                "\"type\":\"generate\",\n",
                "\"schedule\":\"activity\",\n",
                "\"priority\":1,\n",
                "\"output\":\"md\",\n",
                "\"activities\":[\"work\"],\n",
                "\"load\":{\"transcripts\":true}\n",
                "}\n",
                "provider-spy-instruction"
            ),
        )
        .expect("talent");
        let paths = RuntimePaths {
            talent_root,
            apps_root,
            templates_dir,
        };
        let context = ExecutionContext {
            journal: journal.clone(),
        };
        let preview = assemble_prompt_preview(
            &PreviewRequest {
                name: "probe".to_owned(),
                day: Some("20260101".to_owned()),
                segment: None,
                facet: Some("work".to_owned()),
                activity: Some("activity-a".to_owned()),
            },
            &paths,
            &context,
        );
        let PromptPreview::Assembled {
            parts: preview_parts,
            ..
        } = preview
        else {
            panic!("activity preview did not assemble: {preview:?}");
        };

        let capture = root.path().join("provider-request.json");
        let provider = root.path().join("provider-spy.sh");
        let response = json!({
            "schema":"solstone-generate-response-v2",
            "id":null,
            "outcome":"generated",
            "text":"ok",
            "model":"test-model",
            "usage":{},
            "finish_reason":"stop",
            "thinking":null,
            "schema_validation":null,
            "input_budget":null,
            "request_budget":null,
            "inference":null
        });
        fs::write(
            &provider,
            format!(
                "#!/bin/sh\ncat > '{}'\nprintf '%s\\n' '{}'\n",
                capture.display(),
                response
            ),
        )
        .expect("provider spy");
        let mut permissions = fs::metadata(&provider).unwrap().permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(&provider, permissions).expect("provider executable");

        let output_path = journal.join("preview-parity-output.md");
        let request = json!({
            "name":"probe",
            "day":"20260101",
            "facet":"work",
            "activity":record,
            "schedule":"activity",
            "span":["090000_60"],
            "output":"md",
            "output_path":output_path,
            "env":{
                "SOL_DAY":"20260101",
                "SOL_FACET":"work",
                "SOL_ACTIVITY":"activity-a"
            }
        })
        .as_object()
        .expect("request object")
        .clone();
        let generate = solstone_core_generate::OneShotClient::at_path(&provider);
        let cogitate = solstone_core_cogitate_wire::CogitateOneShotClient::at_path(
            root.path().join("unused-cogitate"),
        );
        let mut events = Vec::new();
        let outcome =
            crate::execute_request(request, &paths, &context, &generate, &cogitate, &mut events);
        assert!(
            matches!(outcome, crate::RuntimeOutcome::Finished { .. }),
            "{outcome:?}"
        );
        let captured: Value =
            serde_json::from_str(&fs::read_to_string(&capture).expect("captured request"))
                .expect("provider request JSON");
        let provider_parts = captured["contents"]
            .as_array()
            .expect("contents")
            .iter()
            .map(|part| {
                part.get("text")
                    .and_then(Value::as_str)
                    .expect("text part")
                    .to_owned()
            })
            .collect::<Vec<_>>();
        assert_eq!(preview_parts, provider_parts);
        assert!(preview_parts.join("\n").contains("provider-spy-source"));

        fs::write(
            paths.talent_root.join("cogitate_probe.md"),
            concat!(
                "{\n",
                "\"type\":\"cogitate\",\n",
                "\"schedule\":\"activity\",\n",
                "\"priority\":1,\n",
                "\"activities\":[\"work\"],\n",
                "\"load\":{\"transcripts\":true}\n",
                "}\n",
                "cogitate-body-does-not-win"
            ),
        )
        .expect("cogitate talent");
        let cogitate_preview = assemble_prompt_preview(
            &PreviewRequest {
                name: "cogitate_probe".to_owned(),
                day: Some("20260101".to_owned()),
                segment: None,
                facet: Some("work".to_owned()),
                activity: Some("activity-a".to_owned()),
            },
            &paths,
            &context,
        );
        let PromptPreview::Assembled {
            parts: cogitate_parts,
            ..
        } = cogitate_preview
        else {
            panic!("cogitate activity preview did not assemble: {cogitate_preview:?}");
        };
        let activity_prompt =
            "Processing activity 'activity-a' (work) in facet 'work' for 2026-01-01.";
        assert_eq!(cogitate_parts, vec![activity_prompt]);

        let cogitate_request = json!({
            "name":"cogitate_probe",
            "day":"20260101",
            "facet":"work",
            "activity":record,
            "schedule":"activity",
            "span":["090000_60"],
            "prompt":activity_prompt,
            "use_id":"use-cogitate-preview-oracle",
            "output_path":journal.join("cogitate-output.md"),
            "env":{
                "SOL_DAY":"20260101",
                "SOL_FACET":"work",
                "SOL_ACTIVITY":"activity-a"
            }
        })
        .as_object()
        .expect("cogitate request object")
        .clone();
        let prepared = prepare(cogitate_request, &paths, &context, PrepareMode::Preview)
            .expect("prepare cogitate request");
        let wire = crate::cogitate::cogitate_request(&prepared, &context)
            .expect("assemble cogitate request");
        assert_eq!(cogitate_parts, vec![wire.initial_prompt]);

        fs::write(
            paths.talent_root.join("untyped_probe.md"),
            concat!(
                "{\n",
                "\"schedule\":\"activity\",\n",
                "\"priority\":1,\n",
                "\"activities\":[\"work\"],\n",
                "\"load\":{\"transcripts\":true}\n",
                "}\n",
                "untyped-body"
            ),
        )
        .expect("untyped talent");
        let untyped_preview = assemble_prompt_preview(
            &PreviewRequest {
                name: "untyped_probe".to_owned(),
                day: Some("20260101".to_owned()),
                segment: None,
                facet: Some("work".to_owned()),
                activity: Some("activity-a".to_owned()),
            },
            &paths,
            &context,
        );
        let PromptPreview::Assembled {
            parts: untyped_parts,
            ..
        } = untyped_preview
        else {
            panic!("untyped activity preview did not assemble: {untyped_preview:?}");
        };
        assert!(untyped_parts.iter().any(|part| part == activity_prompt));

        let untyped_request = json!({
            "name":"untyped_probe",
            "day":"20260101",
            "facet":"work",
            "activity":record,
            "schedule":"activity",
            "span":["090000_60"],
            "prompt":activity_prompt,
            "output_path":journal.join("untyped-preview-parity-output.md"),
            "env":{
                "SOL_DAY":"20260101",
                "SOL_FACET":"work",
                "SOL_ACTIVITY":"activity-a"
            }
        })
        .as_object()
        .expect("untyped request object")
        .clone();
        let mut events = Vec::new();
        let outcome = crate::execute_request(
            untyped_request,
            &paths,
            &context,
            &generate,
            &cogitate,
            &mut events,
        );
        assert!(
            matches!(outcome, crate::RuntimeOutcome::Finished { .. }),
            "{outcome:?}"
        );
        let captured: Value =
            serde_json::from_str(&fs::read_to_string(capture).expect("captured request"))
                .expect("provider request JSON");
        let provider_parts = captured["contents"]
            .as_array()
            .expect("contents")
            .iter()
            .map(|part| {
                part.get("text")
                    .and_then(Value::as_str)
                    .expect("text part")
                    .to_owned()
            })
            .collect::<Vec<_>>();
        assert_eq!(untyped_parts, provider_parts);
    }
}
