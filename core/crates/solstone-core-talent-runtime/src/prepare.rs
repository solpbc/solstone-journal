// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::{Path, PathBuf};

use serde_json::{Map, Value};
use solstone_core_talent_config::{TalentConfig, discover, is_truthy, merge, validate};

use crate::{ExecutionContext, PreparedTalent, transcript};

const SPARSE_INPUT_NOTE: &str =
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
    UnresolvableCwd { talent: String },
    NoBrainConfigured,
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
        return Ok(PreparedTalent {
            name,
            config: composed,
        });
    }
    let sources = composed.get("sources").and_then(Value::as_object).cloned();
    if composed
        .get("day")
        .and_then(Value::as_str)
        .is_some_and(|day| !day.is_empty())
        && sources
            .as_ref()
            .is_some_and(transcript::sources_are_enabled)
    {
        let (mut transcript, counts) = transcript::load_transcript(&context.journal, &composed)
            .map_err(PrepareFailure::Refusal)?;
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
                return Ok(PreparedTalent {
                    name,
                    config: composed,
                });
            }
        }
        if solstone_core_transcripts::is_no_input(&transcript, &counts) {
            composed.insert(
                "skip_reason".to_owned(),
                Value::String("no_input".to_owned()),
            );
            return Ok(PreparedTalent {
                name,
                config: composed,
            });
        }
        if counts.total() < 3 {
            transcript.insert_str(0, SPARSE_INPUT_NOTE);
            composed.insert("transcript".to_owned(), Value::String(transcript));
        }
    }
    if composed
        .get("day")
        .and_then(Value::as_str)
        .is_some_and(|day| !day.is_empty())
    {
        let prompt_context = crate::prompt_context::build(&context.journal, &composed);
        let focused_facet = composed.get("facet").and_then(Value::as_str);
        let instruction = solstone_core_talent_cli::compose_talent_instruction(
            &config,
            &context.journal,
            &paths.templates_dir,
            focused_facet,
            &prompt_context,
        )
        .map_err(PrepareFailure::Refusal)?;
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

fn configured_brain(journal: &Path) -> (String, String) {
    let configured = read_journal_config(journal);
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
    equal_or_refuse("cwd", config.metadata.get("cwd"))?;
    let access = request.get("access_tier").filter(|value| !value.is_null());
    if let Some(access) = access
        && config.metadata.get("access_tier") != Some(access)
    {
        return Err(PrepareFailure::Refusal(format!(
            "Request overrides 'access_tier' for talent '{name}' are not allowed ({} != {})",
            python_repr(config.metadata.get("access_tier")),
            python_repr(Some(access)),
        )));
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
    use std::fs;

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
            "Request overrides 'cwd' for talent 'demo' are not allowed ('journal' != 'other')"
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
            "Request overrides 'access_tier' for talent 'demo' are not allowed ('normal' != 'full')"
        );
        let without_access_tier = TalentConfig {
            key: "demo".to_owned(),
            file: String::new(),
            body: String::new(),
            metadata: Map::from_iter([
                ("cwd".to_owned(), json!("journal")),
                ("type".to_owned(), json!("generate")),
            ]),
        };
        assert_eq!(
            reject_request_fields(
                &without_access_tier,
                &Map::from_iter([("access_tier".to_owned(), json!("normal"))]),
                "demo"
            )
            .unwrap_err()
            .to_string(),
            "Request overrides 'access_tier' for talent 'demo' are not allowed (None != 'normal')"
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

    #[test]
    fn preview_mode_skips_only_the_no_brain_check() {
        let root = tempfile::tempdir().unwrap();
        let talent_root = root.path().join("talent");
        std::fs::create_dir_all(&talent_root).unwrap();
        std::fs::create_dir_all(root.path().join("apps")).unwrap();
        std::fs::create_dir_all(root.path().join("templates")).unwrap();
        std::fs::write(
            talent_root.join("demo.md"),
            "{\n\"type\": \"generate\"\n}\nbody",
        )
        .unwrap();
        let journal = root.path().join("journal");
        std::fs::create_dir_all(journal.join("config")).unwrap();
        std::fs::write(
            journal.join("config/journal.json"),
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
            journal.join("config/journal.json"),
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
            "{\n\"type\":\"generate\"\n}\n$segment_preamble",
        )
        .expect("talent");
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
            prepared.config["user_instruction"],
            "Soleil|Thursday, January 01, 2026|20260101|9:00 AM|9:01 AM|an imported note from Obsidian"
        );
    }

    #[test]
    fn activity_request_renders_the_exact_python_context_sections() {
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
            journal.join("facets/work"),
            journal.join("chronicle/20260101/090000_60/talents/work"),
            journal.join("chronicle/20260101/090200_120/talents/work"),
        ] {
            fs::create_dir_all(directory).expect("fixture directory");
        }
        fs::write(
            journal.join("config/journal.json"),
            r#"{"identity":{"preferred":"Soleil"},"providers":{"active":{"provider":"none"}}}"#,
        )
        .expect("journal config");
        fs::write(
            journal.join("facets/work/facet.json"),
            r#"{"name":"work","description":"Work"}"#,
        )
        .expect("facet");
        fs::write(
            journal.join("chronicle/20260101/090000_60/talents/work/activity_state.json"),
            r#"[{"activity":"coding","level":"high","description":"Debugged retry handling"}]"#,
        )
        .expect("first activity state");
        fs::write(
            journal.join("chronicle/20260101/090200_120/talents/work/activity_state.json"),
            r#"[{"activity":"coding","level":"medium","description":"Verified the provider request"}]"#,
        )
        .expect("second activity state");
        fs::write(
            templates_dir.join("activity_preamble.md"),
            concat!(
                "You are analyzing a **$activity_type** activity from $preferred's journal on **$day** ($day_YYYYMMDD), covering **$segment_start to $segment_end** (~$activity_duration minutes).\n\n",
                "**Activity:** $activity_type\n",
                "**Description:** $activity_description\n",
                "**Entities involved:** $activity_entities\n\n",
                "The transcript below contains $content_description from the segments where this activity occurred. These segments may also contain content from other concurrent activities — focus your analysis ONLY on content related to this $activity_type activity."
            ),
        )
        .expect("activity template");
        fs::write(
            talent_root.join("work.md"),
            "{\n\"type\":\"generate\",\"schedule\":\"activity\",\"activities\":[\"coding\"]\n}\n$activity_context\n\n$activity_preamble",
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
        assert_eq!(
            prepared.config["user_instruction"],
            concat!(
                "## Activity Context\n",
                "- **Type:** coding\n",
                "- **Description:** Release work\n",
                "- **Engagement Level:** 0.8 (high)\n",
                "- **Duration:** ~3 minutes (2 segments)\n",
                "- **Active Entities:** Mina, Ravi\n\n",
                "## Activity State Per Segment\n\n",
                "### 090000_60 (9:00 AM - 9:01 AM)\n",
                "coding [high]: Debugged retry handling\n\n",
                "### 090200_120 (9:02 AM - 9:04 AM)\n",
                "coding [medium]: Verified the provider request\n\n",
                "## Analysis Focus\n",
                "You are analyzing ONLY the **coding** activity within the **work** facet. The transcript segments may contain content from other concurrent activities (e.g., background meetings, messaging). Use the Activity State Per Segment section above to identify which content relates to this activity, and ignore unrelated content. Your analysis should only cover what happened within this specific activity.\n\n",
                "You are analyzing a **coding** activity from Soleil's journal on **Thursday, January 01, 2026** (20260101), covering **9:00 AM to 9:04 AM** (~3 minutes).\n\n",
                "**Activity:** coding\n",
                "**Description:** Release work\n",
                "**Entities involved:** Mina, Ravi\n\n",
                "The transcript below contains audio transcription and screen recording from the segments where this activity occurred. These segments may also contain content from other concurrent activities — focus your analysis ONLY on content related to this coding activity."
            )
        );
        let instruction = prepared.config["user_instruction"]
            .as_str()
            .expect("instruction");
        assert!(!instruction.contains("$activity_"));
        assert!(!instruction.contains("$segment_"));
    }
}
