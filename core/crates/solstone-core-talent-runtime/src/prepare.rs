// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::{Path, PathBuf};

use serde_json::{Map, Value};
use solstone_core_talent_config::{TalentConfig, discover, is_truthy};

use crate::{ExecutionContext, PreparedTalent};

#[derive(Clone, Debug)]
pub struct RuntimePaths {
    pub talent_root: PathBuf,
    pub apps_root: PathBuf,
    pub templates_dir: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PrepareFailure {
    Refusal(String),
    UnresolvableCwd {
        talent: String,
    },
    NoBrainConfigured,
    UnportedTranscriptLoading {
        talent: String,
        enabled_sources: Vec<String>,
    },
}

impl std::fmt::Display for PrepareFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Refusal(message) => formatter.write_str(message),
            Self::UnresolvableCwd { talent } => write!(
                formatter,
                "Cannot resolve cwd for talent '{talent}' — journal path unavailable"
            ),
            Self::NoBrainConfigured => {
                formatter.write_str("No thinking engine is chosen yet. Choose one in Thinking.")
            }
            Self::UnportedTranscriptLoading { talent, .. } => write!(
                formatter,
                "transcript loading is unported for talent '{talent}'"
            ),
        }
    }
}

pub fn prepare(
    request: Map<String, Value>,
    paths: &RuntimePaths,
    context: &ExecutionContext,
) -> Result<PreparedTalent, PrepareFailure> {
    let name = request
        .get("name")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| PrepareFailure::Refusal("talent request missing name".to_owned()))?
        .to_owned();
    let config = discover(&paths.talent_root, &paths.apps_root)
        .map_err(PrepareFailure::Refusal)?
        .into_iter()
        .find(|config| config.key == name)
        .ok_or_else(|| PrepareFailure::Refusal(format!("talent '{name}' not found")))?;
    reject_definition_fields(&config, &name)?;
    reject_request_fields(&config, &request, &name)?;
    let focused_facet = request.get("facet").and_then(Value::as_str);
    let mut composed = solstone_core_talent_cli::compose_talent(
        &config,
        &context.journal,
        &paths.templates_dir,
        focused_facet,
    )
    .map_err(PrepareFailure::Refusal)?;
    for (key, value) in request {
        if !value.is_null() {
            composed.insert(key, value);
        }
    }
    let (provider, model) = configured_brain(&context.journal);
    composed.insert("provider".to_owned(), Value::String(provider));
    composed.insert("model".to_owned(), Value::String(model));
    if composed.get("cwd").and_then(Value::as_str) == Some("journal") {
        if !context.journal.exists() {
            return Err(PrepareFailure::UnresolvableCwd { talent: name });
        }
        composed.insert(
            "cwd".to_owned(),
            Value::String(context.journal.display().to_string()),
        );
    }
    if composed.get("provider").and_then(Value::as_str) == Some("none") {
        return Err(PrepareFailure::NoBrainConfigured);
    }
    let enabled_sources = composed
        .get("sources")
        .and_then(Value::as_object)
        .map(|sources| {
            sources
                .iter()
                .filter(|(_, value)| is_truthy(value))
                .map(|(name, _)| name.clone())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if !enabled_sources.is_empty() {
        return Err(PrepareFailure::UnportedTranscriptLoading {
            talent: name,
            enabled_sources,
        });
    }
    if let Some(output) = composed.get("output").and_then(Value::as_str)
        && let Some(day) = composed.get("day").and_then(Value::as_str)
    {
        let output_path = composed
            .get("output_path")
            .and_then(Value::as_str)
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                solstone_core_talent_config::get_output_path(
                    &context.journal.join("chronicle").join(day),
                    &name,
                    composed.get("segment").and_then(Value::as_str),
                    Some(output),
                    composed.get("facet").and_then(Value::as_str),
                    std::env::var("SOL_STREAM").ok().as_deref(),
                )
            });
        composed.insert(
            "output_path".to_owned(),
            Value::String(output_path.display().to_string()),
        );
    }
    validate_config(&composed).map_err(PrepareFailure::Refusal)?;
    Ok(PreparedTalent {
        name,
        config: composed,
    })
}

fn configured_brain(journal: &Path) -> (String, String) {
    let configured = std::fs::read_to_string(journal.join("config/journal.json"))
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok());
    let active = configured
        .as_ref()
        .and_then(|value| value.pointer("/providers/active"));
    (
        active
            .and_then(|value| value.get("provider"))
            .and_then(Value::as_str)
            .unwrap_or("none")
            .to_owned(),
        active
            .and_then(|value| value.get("model"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
    )
}

fn reject_definition_fields(config: &TalentConfig, name: &str) -> Result<(), PrepareFailure> {
    if config.metadata.contains_key("outbound_approval") {
        return Err(PrepareFailure::Refusal(format!(
            "talent {name:?} declares 'outbound_approval' in frontmatter; this field is launch-config-only and may not come from a talent definition"
        )));
    }
    for field in ["provider", "model"] {
        if config.metadata.contains_key(field) {
            return Err(PrepareFailure::Refusal(format!(
                "talent {name:?} declares {field:?} in frontmatter; thinking provider and model are configured only in Thinking"
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
                "request overrides for {field:?} are not allowed; thinking provider and model are configured only in Thinking"
            )));
        }
    }
    let equal_or_refuse = |field: &str, declared: Option<&Value>| -> Result<(), PrepareFailure> {
        let Some(requested) = request.get(field).filter(|value| !value.is_null()) else {
            return Ok(());
        };
        if declared != Some(requested) {
            return Err(PrepareFailure::Refusal(format!(
                "Request overrides '{field}' for talent '{name}' are not allowed ({declared:?} != {requested:?})"
            )));
        }
        Ok(())
    };
    equal_or_refuse("cwd", config.metadata.get("cwd"))?;
    let access = request.get("access_tier").filter(|value| !value.is_null());
    if let Some(access) = access
        && config.metadata.get("access_tier") != Some(access)
    {
        return Err(PrepareFailure::Refusal(format!(
            "Request overrides 'access_tier' for talent '{name}' are not allowed ({:?} != {access:?})",
            config.metadata.get("access_tier")
        )));
    }
    equal_or_refuse("type", config.metadata.get("type"))
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
    if kind == "cogitate" && !prompt && !instruction {
        return Err("Cogitate talent requires non-empty 'prompt' or 'user_instruction'".to_owned());
    }
    if kind != "cogitate" && !day && !instruction && !prompt {
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
    use serde_json::json;

    use super::*;

    #[test]
    fn criterion_3_and_25_request_refusals_preserve_equal_echoes() {
        let config = TalentConfig {
            key: "demo".to_owned(),
            file: String::new(),
            body: String::new(),
            metadata: Map::from_iter([
                ("cwd".to_owned(), json!("journal")),
                ("type".to_owned(), json!("generate")),
                ("access_tier".to_owned(), json!("normal")),
            ]),
        };
        let echo = Map::from_iter([
            ("cwd".to_owned(), json!("journal")),
            ("type".to_owned(), json!("generate")),
            ("access_tier".to_owned(), json!("normal")),
        ]);
        assert!(reject_request_fields(&config, &echo, "demo").is_ok());
        for field in ["provider", "model"] {
            assert!(
                reject_request_fields(
                    &config,
                    &Map::from_iter([(field.to_owned(), json!("x"))]),
                    "demo"
                )
                .is_err()
            );
        }
        assert!(
            reject_request_fields(
                &config,
                &Map::from_iter([("cwd".to_owned(), json!("other"))]),
                "demo"
            )
            .is_err()
        );
        assert!(
            reject_request_fields(
                &config,
                &Map::from_iter([("type".to_owned(), json!("cogitate"))]),
                "demo"
            )
            .is_err()
        );
    }

    #[test]
    fn criterion_4_validation_messages_are_verbatim() {
        assert_eq!(
            validate_config(&Map::from_iter([("type".to_owned(), json!("cogitate"))])).unwrap_err(),
            "Cogitate talent requires non-empty 'prompt' or 'user_instruction'"
        );
        assert_eq!(
            validate_config(&Map::new()).unwrap_err(),
            "Invalid config: must have 'type', 'day', or 'prompt'"
        );
        assert_eq!(
            validate_config(&Map::from_iter([
                ("type".to_owned(), json!("generate")),
                ("segment".to_owned(), json!("x")),
                ("prompt".to_owned(), json!("x"))
            ]))
            .unwrap_err(),
            "Invalid config: 'segment' or 'span' requires 'day'"
        );
    }
}
