// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::{Map, Value};

use solstone_core_generate::ContentPart;
use solstone_core_talent_cli::preview::{PreviewRequest, PromptPreview};

use crate::contract::{GateDecision, resolve_hook};
use crate::prepare::{PrepareMode, RuntimePaths, prepare};
use crate::transcript::sources_are_enabled;
use crate::{DRY_RUN_KEY, ExecutionContext, RuntimeOutcome, generate_contents};

pub fn assemble_prompt_preview(
    request: &PreviewRequest,
    paths: &RuntimePaths,
    context: &ExecutionContext,
) -> PromptPreview {
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

    let mut prepared = match prepare(payload, paths, context, PrepareMode::Preview) {
        Ok(prepared) => prepared,
        Err(error) => {
            return PromptPreview::Failed {
                error: error.to_string(),
            };
        }
    };
    if let Some(reason) = prepared
        .config
        .get("skip_reason")
        .and_then(Value::as_str)
        .filter(|reason| !reason.is_empty())
    {
        return PromptPreview::WouldNotRun {
            reason: reason.to_owned(),
        };
    }

    let hook = prepared
        .config
        .get("hook")
        .and_then(Value::as_object)
        .and_then(|hook| hook.get("pre"))
        .and_then(Value::as_str);
    if let Some(hook) = hook {
        let Some(stage) = resolve_hook(hook) else {
            return PromptPreview::UnavailablePreStep;
        };
        prepared
            .config
            .insert(DRY_RUN_KEY.to_owned(), Value::Bool(true));
        if let Some(gate) = stage.gate {
            match gate(&prepared, context) {
                Ok(GateDecision::Proceed) => {}
                Ok(GateDecision::Skip(reason)) => {
                    return PromptPreview::WouldNotRun { reason };
                }
                Err(error) => {
                    return PromptPreview::Failed {
                        error: error.to_string(),
                    };
                }
            }
        }
        let state = match stage.build {
            Some(build) => match build(&mut prepared, context) {
                Ok(state) => state,
                Err(RuntimeOutcome::Skipped { reason, .. }) => {
                    return PromptPreview::WouldNotRun { reason };
                }
                Err(RuntimeOutcome::StageFailed(error)) => {
                    return PromptPreview::Failed {
                        error: error.to_string(),
                    };
                }
                Err(_) => unreachable!("a BuildFn can only return Skipped or StageFailed"),
            },
            None => crate::contract::PrePostState::None,
        };
        if let Some(override_prompt) = stage.prompt_override
            && let Err(error) = override_prompt(&mut prepared, &state)
        {
            return PromptPreview::Failed {
                error: error.to_string(),
            };
        }
    }

    let parts = generate_contents(&prepared)
        .into_iter()
        .filter_map(|part| match part {
            ContentPart::Text { text } => Some(text),
            ContentPart::Image { .. } => None,
        })
        .collect();
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

#[cfg(test)]
mod tests {
    #[test]
    fn assemble_source_has_no_process_spawn() {
        // Criterion D8: preview assemble must not spawn a model process.
        let source = include_str!("assemble.rs");
        let command_new = ["Command", "::new"].concat();
        assert!(!source.contains(&command_new));
    }
}
