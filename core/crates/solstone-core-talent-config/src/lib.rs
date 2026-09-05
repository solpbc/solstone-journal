// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Shared discovery, validation, and output layout for talent execution config.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde_json::{Map, Value};
use solstone_core_cogitate::TALENT_ACCESS_TIERS;

#[derive(Debug, Clone)]
pub struct TalentConfig {
    pub key: String,
    pub file: String,
    pub metadata: Map<String, Value>,
    pub body: String,
}

#[derive(Debug, Clone)]
pub struct ParsedFrontmatter {
    pub metadata: Map<String, Value>,
    pub body: String,
}

#[derive(Debug, Clone, Copy)]
pub struct TalentFilter<'a> {
    pub r#type: Option<&'a str>,
    pub schedule: Option<&'a str>,
    pub include_disabled: bool,
}

pub fn load_talent_configs(
    talent_root: &Path,
    apps_root: &Path,
    talent_overrides: Option<&Map<String, Value>>,
    filter: TalentFilter<'_>,
) -> Result<Vec<TalentConfig>, String> {
    let mut configs = discover(talent_root, apps_root)?;
    merge(&mut configs, talent_overrides);
    validate(&mut configs)?;
    Ok(configs
        .into_iter()
        .filter(|config| matches_filter(config, filter))
        .collect())
}

pub fn discover(talent_root: &Path, apps_root: &Path) -> Result<Vec<TalentConfig>, String> {
    let mut configs = Vec::new();
    collect_system(talent_root, &mut configs)?;
    collect_apps(apps_root, &mut configs)?;
    Ok(configs)
}

fn collect_system(root: &Path, configs: &mut Vec<TalentConfig>) -> Result<(), String> {
    for path in markdown_entries(root)? {
        let name = stem(&path)?;
        configs.push(config(
            &path,
            name.clone(),
            format!("talent/{name}.md"),
            "system",
            None,
        )?);
    }
    Ok(())
}

fn collect_apps(root: &Path, configs: &mut Vec<TalentConfig>) -> Result<(), String> {
    if !root.is_dir() {
        return Ok(());
    }
    let mut apps = fs::read_dir(root)
        .map_err(|error| format!("failed to read {}: {error}", root.display()))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("failed to read {}: {error}", root.display()))?;
    apps.sort_by_key(|entry| entry.file_name());
    for app_entry in apps {
        let app_path = app_entry.path();
        if !app_path.is_dir() {
            continue;
        }
        let app = app_entry.file_name().to_string_lossy().into_owned();
        if app.starts_with('_') {
            continue;
        }
        for path in markdown_entries(&app_path.join("talent"))? {
            let name = stem(&path)?;
            let key = format!("{app}:{name}");
            let file = format!("apps/{app}/talent/{name}.md");
            configs.push(config(&path, key, file, "app", Some(app.clone()))?);
        }
    }
    Ok(())
}

fn config(
    path: &Path,
    key: String,
    file: String,
    source: &str,
    app: Option<String>,
) -> Result<TalentConfig, String> {
    let parsed = read_frontmatter(path)?;
    let modified = fs::metadata(path)
        .map_err(|error| format!("failed to stat {}: {error}", path.display()))?
        .modified()
        .map_err(|error| format!("failed to stat {}: {error}", path.display()))?
        .duration_since(UNIX_EPOCH)
        .map_err(|error| format!("failed to stat {}: {error}", path.display()))?
        .as_secs();
    let mut metadata = Map::new();
    metadata.insert("path".to_owned(), Value::String(path.display().to_string()));
    metadata.insert("mtime".to_owned(), Value::Number(modified.into()));
    metadata.extend(parsed.metadata);
    metadata
        .entry("color".to_owned())
        .or_insert_with(|| Value::String("#6c757d".to_owned()));
    metadata.insert("source".to_owned(), Value::String(source.to_owned()));
    if let Some(app) = app {
        metadata.insert("app".to_owned(), Value::String(app));
    }
    Ok(TalentConfig {
        key,
        file,
        metadata,
        body: parsed.body,
    })
}

/// Deliberately parses only JSON frontmatter enclosed by a line containing { and a later line containing }; YAML (---) frontmatter is not supported.
pub fn read_frontmatter(path: &Path) -> Result<ParsedFrontmatter, String> {
    let raw = fs::read_to_string(path)
        .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
    let text = raw.trim();
    if text.lines().next() != Some("{") {
        return Ok(ParsedFrontmatter {
            metadata: Map::new(),
            body: text.to_owned(),
        });
    }
    let mut offset = 0;
    let mut close_end = None;
    for line in text.split_inclusive('\n') {
        offset += line.len();
        let logical = line
            .strip_suffix('\n')
            .unwrap_or(line)
            .strip_suffix('\r')
            .unwrap_or(line.strip_suffix('\n').unwrap_or(line));
        if logical == "}" {
            close_end = Some(offset);
            break;
        }
    }
    let Some(close_end) = close_end else {
        return Ok(ParsedFrontmatter {
            metadata: Map::new(),
            body: text.to_owned(),
        });
    };
    let value: Value = serde_json::from_str(&text[..close_end])
        .map_err(|_| format!("failed to parse frontmatter from {}", path.display()))?;
    let metadata = value.as_object().cloned().unwrap_or_default();
    if !value.is_object() {
        return Err(format!(
            "failed to parse frontmatter from {}",
            path.display()
        ));
    }
    Ok(ParsedFrontmatter {
        metadata,
        body: text[close_end..].trim().to_owned(),
    })
}

pub fn read_talent_overrides(journal_root: &Path) -> Result<Option<Map<String, Value>>, String> {
    let path = journal_root.join("config/journal.json");
    let Ok(text) = fs::read_to_string(&path) else {
        return Ok(None);
    };
    let config: Value = serde_json::from_str(&text)
        .map_err(|_| format!("failed to read talent overrides from {}", path.display()))?;
    Ok(config
        .get("talent_overrides")
        .and_then(Value::as_object)
        .cloned())
}

pub fn merge(configs: &mut [TalentConfig], talent_overrides: Option<&Map<String, Value>>) {
    for config in configs {
        let Some(override_value) = talent_overrides
            .and_then(|contexts| contexts.get(&context_key(&config.key)))
            .and_then(Value::as_object)
        else {
            continue;
        };
        for field in ["disabled", "extract"] {
            if let Some(value) = override_value.get(field) {
                config.metadata.insert(field.to_owned(), value.clone());
            }
        }
        // `max_output_tokens` is owner-settable because the right value is a property of the
        // serving backend, not of the talent. Admission is `input_tokens + max_output_tokens
        // <= window`, so this reservation is subtracted from what a talent may read, and the
        // window differs per provider -- 16,384 on one endpoint and 262,144 on another. A
        // build-time constant cannot know which it is running against.
        //
        // Only a positive integer is accepted; anything else is ignored rather than allowed to
        // poison the budget arithmetic. An over-large value is not rejected here because the
        // window is not known at merge time -- the admission check refuses it with
        // `context_budget_exceeded`, which is an honest and visible failure.
        if let Some(tokens) = override_value
            .get("max_output_tokens")
            .and_then(Value::as_u64)
            .filter(|tokens| *tokens > 0)
        {
            config
                .metadata
                .insert("max_output_tokens".to_owned(), Value::from(tokens));
        }
    }
}

pub fn context_key(key: &str) -> String {
    match key.split_once(':') {
        Some((app, name)) => format!("talent.{app}.{name}"),
        None => format!("talent.system.{key}"),
    }
}

pub fn validate(configs: &mut [TalentConfig]) -> Result<(), String> {
    for config in configs.iter() {
        if config.metadata.get("schedule").is_some_and(is_truthy)
            && !config.metadata.contains_key("priority")
        {
            return Err(format!(
                "Scheduled prompt '{}' is missing required 'priority' field. All prompts with 'schedule' must declare an explicit priority.",
                config.key
            ));
        }
    }
    for config in configs.iter() {
        let output_present = config.metadata.contains_key("output");
        let config_type = config.metadata.get("type");
        if let Some(config_type) = config_type
            && !matches!(config_type.as_str(), Some("generate" | "cogitate"))
        {
            return Err(format!(
                "Prompt '{}' has invalid type {}. Expected 'generate' or 'cogitate'.",
                config.key,
                python_repr(config_type)
            ));
        }
        if !output_present && config_type.is_none() {
            continue;
        }
        if config_type.is_none() {
            return Err(format!(
                "Prompt '{}' has output but is missing required 'type' field.",
                config.key
            ));
        }
        if config_type.and_then(Value::as_str) == Some("generate") && !output_present {
            return Err(format!(
                "Prompt '{}' has type='generate' but is missing required 'output' field.",
                config.key
            ));
        }
    }
    for config in configs.iter() {
        if config.metadata.get("schedule").and_then(Value::as_str) == Some("activity")
            && !config
                .metadata
                .get("activities")
                .and_then(Value::as_array)
                .is_some_and(|items| !items.is_empty())
        {
            return Err(format!(
                "Activity-scheduled prompt '{}' must have a non-empty 'activities' list (activity types to match, or [\"*\"] for all types).",
                config.key
            ));
        }
    }
    for config in configs {
        let talent_type = config
            .metadata
            .get("type")
            .and_then(Value::as_str)
            .map(str::to_owned);
        validate_write(config, talent_type.as_deref())?;
        validate_access_tier(config, talent_type.as_deref())?;
        validate_cwd(config, talent_type.as_deref())?;
    }
    Ok(())
}

pub fn validate_write(config: &TalentConfig, talent_type: Option<&str>) -> Result<(), String> {
    if talent_type == Some("cogitate") && config.metadata.get("write").is_some_and(is_truthy) {
        return Err(format!(
            "Prompt '{}' declares unsupported 'write: true' (cogitate runs are read-only)",
            config.key
        ));
    }
    Ok(())
}

pub fn validate_access_tier(
    config: &mut TalentConfig,
    talent_type: Option<&str>,
) -> Result<(), String> {
    let raw = config.metadata.get("access_tier").cloned();
    if talent_type == Some("cogitate") {
        match raw {
            None => {
                config
                    .metadata
                    .insert("access_tier".to_owned(), Value::String("normal".to_owned()));
            }
            Some(Value::String(value)) if TALENT_ACCESS_TIERS.contains(&value.as_str()) => {}
            Some(value) => {
                return Err(format!(
                    "Prompt '{}' has invalid 'access_tier' value '{}' (must be one of {})",
                    config.key,
                    python_str(&value),
                    tier_tuple()
                ));
            }
        }
    } else if raw.is_some() {
        return Err(format!(
            "Prompt '{}' sets 'access_tier' but access_tier is only valid for type: cogitate",
            config.key
        ));
    }
    Ok(())
}

pub fn validate_cwd(config: &mut TalentConfig, talent_type: Option<&str>) -> Result<(), String> {
    let raw = config.metadata.get("cwd").cloned();
    match talent_type {
        Some("cogitate") => match raw {
            None => {
                config
                    .metadata
                    .insert("cwd".to_owned(), Value::String("journal".to_owned()));
            }
            Some(Value::String(value)) if value == "journal" => {}
            Some(value) => {
                return Err(format!(
                    "Prompt '{}' has invalid 'cwd' value '{}' (must be 'journal')",
                    config.key,
                    python_str(&value)
                ));
            }
        },
        Some("generate") if raw.is_some() => {
            return Err(format!(
                "Prompt '{}' sets 'cwd' but cwd is only valid for type: cogitate",
                config.key
            ));
        }
        _ if raw.is_some() => {
            return Err(format!(
                "Prompt '{}' has invalid 'cwd' value '{}' (must be 'journal')",
                config.key,
                python_str(raw.as_ref().expect("checked"))
            ));
        }
        _ => {}
    }
    Ok(())
}

pub fn is_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(value) => *value,
        Value::Number(value) => value.as_f64().is_none_or(|value| value != 0.0),
        Value::String(value) => !value.is_empty(),
        Value::Array(value) => !value.is_empty(),
        Value::Object(value) => !value.is_empty(),
    }
}

pub fn source_is_enabled(value: &Value) -> bool {
    match value {
        Value::Object(sources) => sources.values().any(source_value_is_enabled),
        value => source_value_is_enabled(value),
    }
}

pub fn source_is_required(value: &Value) -> bool {
    match value {
        Value::Object(sources) => sources.values().any(source_value_is_required),
        value => source_value_is_required(value),
    }
}

pub fn get_talent_filter(value: &Value) -> Option<Map<String, Value>> {
    match value {
        Value::Object(filter) => Some(filter.clone()),
        Value::Bool(false) => Some(Map::new()),
        _ => None,
    }
}

fn source_value_is_enabled(value: &Value) -> bool {
    matches!(value, Value::Bool(true)) || value.as_str() == Some("required")
}

fn source_value_is_required(value: &Value) -> bool {
    value.as_str() == Some("required")
}

pub fn get_output_name(key: &str) -> String {
    match key.split_once(':') {
        Some((app, name)) => format!("_{app}_{name}"),
        None => key.to_owned(),
    }
}

pub fn output_extension(output_format: Option<&str>) -> &'static str {
    if output_format == Some("json") {
        "json"
    } else {
        "md"
    }
}

pub fn get_output_path(
    day_dir: &Path,
    key: &str,
    segment: Option<&str>,
    output_format: Option<&str>,
    facet: Option<&str>,
    stream: Option<&str>,
) -> PathBuf {
    let filename = format!(
        "{}.{}",
        get_output_name(key),
        output_extension(output_format)
    );
    let root = match segment {
        Some(segment) => match stream {
            Some(stream) => day_dir.join(stream).join(segment),
            None => day_dir.join(segment),
        },
        None => day_dir.to_path_buf(),
    };
    let talents = root.join("talents");
    match facet {
        Some(facet) => talents.join(facet).join(filename),
        None => talents.join(filename),
    }
}

fn matches_filter(config: &TalentConfig, filter: TalentFilter<'_>) -> bool {
    filter
        .r#type
        .is_none_or(|kind| config.metadata.get("type").and_then(Value::as_str) == Some(kind))
        && filter.schedule.is_none_or(|schedule| {
            config.metadata.get("schedule").and_then(Value::as_str) == Some(schedule)
        })
        && (filter.include_disabled || !config.metadata.get("disabled").is_some_and(is_truthy))
}

fn markdown_entries(root: &Path) -> Result<Vec<PathBuf>, String> {
    if !root.is_dir() {
        return Ok(Vec::new());
    }
    let mut entries = fs::read_dir(root)
        .map_err(|error| format!("failed to read {}: {error}", root.display()))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("failed to read {}: {error}", root.display()))?
        .into_iter()
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file() && path.extension().and_then(|ext| ext.to_str()) == Some("md")
        })
        .collect::<Vec<_>>();
    entries.sort();
    Ok(entries)
}

fn stem(path: &Path) -> Result<String, String> {
    path.file_stem()
        .and_then(|stem| stem.to_str())
        .map(ToOwned::to_owned)
        .ok_or_else(|| format!("talent filename has no UTF-8 stem: {}", path.display()))
}

fn tier_tuple() -> String {
    format!(
        "({})",
        TALENT_ACCESS_TIERS
            .iter()
            .map(|tier| format!("'{tier}'"))
            .collect::<Vec<_>>()
            .join(", ")
    )
}
fn python_str(value: &Value) -> String {
    match value {
        Value::String(value) => value.clone(),
        Value::Bool(value) => {
            if *value {
                "True".to_owned()
            } else {
                "False".to_owned()
            }
        }
        Value::Null => "None".to_owned(),
        _ => value.to_string(),
    }
}
fn python_repr(value: &Value) -> String {
    match value {
        Value::String(value) => format!("'{value}'"),
        Value::Bool(value) => python_str(&Value::Bool(*value)),
        Value::Null => "None".to_owned(),
        _ => value.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use serde_json::json;

    use super::*;

    const CASES: [(&str, &str, bool); 11] = [
        (
            "lf",
            "{\n\"type\":\"generate\",\"output\":\"md\",\"schedule\":\"daily\",\"priority\":50\n}\nbody",
            true,
        ),
        (
            "leading_blank",
            "\n{\n\"type\":\"generate\",\"output\":\"md\",\"schedule\":\"daily\",\"priority\":50\n}\nbody",
            true,
        ),
        ("unclosed", "{\n\"type\":\"generate\"\nbody", false),
        (
            "crlf",
            "{\r\n\"type\":\"generate\",\"output\":\"md\",\"schedule\":\"daily\",\"priority\":50\r\n}\r\nbody",
            true,
        ),
        ("opening_space", "{ \n\"type\":\"generate\"\n}\nbody", false),
        (
            "nested_column_zero",
            "{\n\"nested\": {\n\"x\":1\n}\n}\nbody",
            false,
        ),
        (
            "nested_indented",
            "{\n\"nested\": {\n\"x\":1\n }\n}\nbody",
            true,
        ),
        ("invalid", "{\n\"type\": generate\n}\nbody", false),
        ("none", "body", false),
        ("empty", "", false),
        ("array", "[\"generate\"]\nbody", false),
    ];

    #[test]
    fn criterion_1_2_3_reader_conformance_table() {
        let directory = tempfile::tempdir().unwrap();
        for (name, text, parses) in CASES {
            let path = directory.path().join(format!("{name}.md"));
            fs::write(&path, text).unwrap();
            let result = read_frontmatter(&path);
            match name {
                "nested_column_zero" | "invalid" => assert!(result.is_err(), "{name}"),
                _ if parses => assert!(!result.unwrap().metadata.is_empty(), "{name}"),
                _ => assert!(result.unwrap().metadata.is_empty(), "{name}"),
            }
        }
    }

    #[test]
    fn criterion_2_crlf_metadata_equals_lf() {
        let directory = tempfile::tempdir().unwrap();
        let lf = directory.path().join("lf.md");
        let crlf = directory.path().join("crlf.md");
        fs::write(&lf, CASES[0].1).unwrap();
        fs::write(&crlf, CASES[3].1).unwrap();
        assert_eq!(
            read_frontmatter(&lf).unwrap().metadata,
            read_frontmatter(&crlf).unwrap().metadata
        );
    }

    #[test]
    fn criterion_3_trailing_space_keeps_json_in_body() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("space.md");
        fs::write(&path, CASES[4].1).unwrap();
        assert!(read_frontmatter(&path).unwrap().body.starts_with("{ "));
    }

    #[test]
    fn criterion_5_type_selector_composes() {
        let directory = tempfile::tempdir().unwrap();
        let talent_root = directory.path().join("talent");
        fs::create_dir(&talent_root).unwrap();
        fs::write(
            talent_root.join("generate_daily.md"),
            "{\n\"type\":\"generate\",\"output\":\"md\",\"schedule\":\"daily\",\"priority\":50\n}\n",
        )
        .unwrap();
        fs::write(
            talent_root.join("cogitate.md"),
            "{\n\"type\":\"cogitate\"\n}\n",
        )
        .unwrap();
        fs::write(
            talent_root.join("generate_segment.md"),
            "{\n\"type\":\"generate\",\"output\":\"md\",\"schedule\":\"segment\",\"priority\":50\n}\n",
        )
        .unwrap();
        fs::write(
            talent_root.join("disabled.md"),
            "{\n\"type\":\"generate\",\"output\":\"md\",\"schedule\":\"daily\",\"priority\":50\n}\n",
        )
        .unwrap();
        let overrides = Map::from_iter([(context_key("disabled"), json!({"disabled": true}))]);

        let type_only = load_talent_configs(
            &talent_root,
            &directory.path().join("apps"),
            Some(&overrides),
            TalentFilter {
                r#type: Some("generate"),
                schedule: None,
                include_disabled: true,
            },
        )
        .unwrap();
        assert!(
            type_only
                .iter()
                .any(|config| config.key == "generate_daily")
        );
        assert!(!type_only.iter().any(|config| config.key == "cogitate"));

        let type_and_schedule = load_talent_configs(
            &talent_root,
            &directory.path().join("apps"),
            Some(&overrides),
            TalentFilter {
                r#type: Some("generate"),
                schedule: Some("daily"),
                include_disabled: true,
            },
        )
        .unwrap();
        assert!(
            type_and_schedule
                .iter()
                .any(|config| config.key == "generate_daily")
        );
        assert!(
            !type_and_schedule
                .iter()
                .any(|config| config.key == "generate_segment")
        );

        let type_and_disabled = load_talent_configs(
            &talent_root,
            &directory.path().join("apps"),
            Some(&overrides),
            TalentFilter {
                r#type: Some("generate"),
                schedule: None,
                include_disabled: false,
            },
        )
        .unwrap();
        assert!(
            type_and_disabled
                .iter()
                .any(|config| config.key == "generate_daily")
        );
        assert!(
            !type_and_disabled
                .iter()
                .any(|config| config.key == "disabled")
        );
    }

    #[test]
    fn criterion_6_validation_precedes_schedule_and_disabled_filters() {
        let directory = tempfile::tempdir().unwrap();
        let talent_root = directory.path().join("talent");
        fs::create_dir(&talent_root).unwrap();
        fs::write(
            talent_root.join("scheduled.md"),
            "{\n\"type\":\"generate\",\"output\":\"md\",\"schedule\":\"daily\"\n}\n",
        )
        .unwrap();
        let expected = "Scheduled prompt 'scheduled' is missing required 'priority' field.";

        let schedule_error = load_talent_configs(
            &talent_root,
            &directory.path().join("apps"),
            None,
            TalentFilter {
                r#type: Some("generate"),
                schedule: Some("segment"),
                include_disabled: true,
            },
        )
        .unwrap_err();
        assert!(schedule_error.contains(expected));

        let overrides = Map::from_iter([(context_key("scheduled"), json!({"disabled": true}))]);
        let disabled_error = load_talent_configs(
            &talent_root,
            &directory.path().join("apps"),
            Some(&overrides),
            TalentFilter {
                r#type: Some("generate"),
                schedule: None,
                include_disabled: false,
            },
        )
        .unwrap_err();
        assert!(disabled_error.contains(expected));
    }

    #[test]
    fn criterion_7_defaults_are_field_specific() {
        let directory = tempfile::tempdir().unwrap();
        fs::create_dir(directory.path().join("talent")).unwrap();
        fs::write(
            directory.path().join("talent/cogitate.md"),
            "{\n\"type\":\"cogitate\"\n}\n",
        )
        .unwrap();
        fs::write(directory.path().join("talent/plain.md"), "{}\n").unwrap();
        let configs = load_talent_configs(
            directory.path().join("talent").as_path(),
            directory.path().join("apps").as_path(),
            None,
            TalentFilter {
                r#type: None,
                schedule: None,
                include_disabled: true,
            },
        )
        .unwrap();
        let cogitate = configs
            .iter()
            .find(|config| config.key == "cogitate")
            .unwrap();
        let plain = configs.iter().find(|config| config.key == "plain").unwrap();
        assert_eq!(cogitate.metadata["access_tier"], "normal");
        assert_eq!(cogitate.metadata["cwd"], "journal");
        assert_eq!(cogitate.metadata["color"], "#6c757d");
        assert!(!plain.metadata.contains_key("access_tier"));
        assert!(!plain.metadata.contains_key("cwd"));
    }

    #[test]
    fn criterion_9_output_paths_match_reference() {
        let day = Path::new("day");
        assert_eq!(
            get_output_path(day, "daily", None, None, None, None),
            PathBuf::from("day/talents/daily.md")
        );
        assert_eq!(
            get_output_path(day, "daily", None, None, Some("work"), None),
            PathBuf::from("day/talents/work/daily.md")
        );
        assert_eq!(
            get_output_path(day, "segment", Some("segment"), None, None, None),
            PathBuf::from("day/segment/talents/segment.md")
        );
        assert_eq!(
            get_output_path(day, "segment", Some("segment"), None, None, Some("stream")),
            PathBuf::from("day/stream/segment/talents/segment.md")
        );
        assert_eq!(
            get_output_path(day, "segment", Some("segment"), None, Some("work"), None),
            PathBuf::from("day/segment/talents/work/segment.md")
        );
        assert_eq!(
            get_output_path(
                day,
                "app:name",
                Some("segment"),
                Some("json"),
                Some("work"),
                Some("stream")
            ),
            PathBuf::from("day/stream/segment/talents/work/_app_name.json")
        );
    }

    #[test]
    fn criterion_10_output_extension_matches_reference() {
        assert_eq!(output_extension(Some("json")), "json");
        assert_eq!(output_extension(None), "md");
        assert_eq!(output_extension(Some("md")), "md");
        assert_eq!(output_extension(Some("other")), "md");
    }

    // AC: `max_output_tokens` is owner-settable, because admission is
    // `input_tokens + max_output_tokens <= window` and the window is a property of the serving
    // backend, not of the talent -- 16,384 on one endpoint and 262,144 on another. Only a
    // positive integer is accepted; junk is ignored rather than allowed to poison the budget.
    #[test]
    fn max_output_tokens_is_overridable_and_rejects_non_positive_values() {
        let base = || {
            vec![TalentConfig {
                key: "one".to_owned(),
                file: String::new(),
                metadata: Map::from_iter([("max_output_tokens".to_owned(), json!(12288))]),
                body: String::new(),
            }]
        };
        let apply = |value: Value| {
            let mut configs = base();
            merge(
                &mut configs,
                Some(&Map::from_iter([(
                    context_key("one"),
                    json!({"max_output_tokens": value}),
                )])),
            );
            configs[0].metadata["max_output_tokens"].clone()
        };

        assert_eq!(
            apply(json!(2048)),
            json!(2048),
            "a positive integer applies"
        );
        assert_eq!(apply(json!(0)), json!(12288), "zero is ignored");
        assert_eq!(apply(json!(-5)), json!(12288), "a negative is ignored");
        assert_eq!(apply(json!("lots")), json!(12288), "a string is ignored");

        // an override for a different talent must not leak across
        let mut configs = base();
        merge(
            &mut configs,
            Some(&Map::from_iter([(
                context_key("other"),
                json!({"max_output_tokens": 2048}),
            )])),
        );
        assert_eq!(configs[0].metadata["max_output_tokens"], json!(12288));
    }

    #[test]
    fn criterion_8_merge_keeps_non_boolean_override() {
        let mut configs = vec![TalentConfig {
            key: "one".to_owned(),
            file: String::new(),
            metadata: Map::new(),
            body: String::new(),
        }];
        merge(
            &mut configs,
            Some(&Map::from_iter([(
                context_key("one"),
                json!({"disabled":"yes"}),
            )])),
        );
        assert_eq!(configs[0].metadata["disabled"], "yes");
    }

    #[test]
    fn criterion_5_source_rules_match_the_reference_enabledness_and_filter_semantics() {
        let all_false = json!({"transcripts": false, "percepts": false});
        let any_true = json!({"transcripts": false, "percepts": true});
        let any_required = json!({"transcripts": false, "percepts": "required"});
        let filter = json!({"screen": true, "sense": "required", "other": false});

        assert!(!source_is_enabled(&all_false));
        assert!(!source_is_enabled(&json!("enabled")));
        assert!(source_is_enabled(&any_true));
        assert!(source_is_enabled(&any_required));
        assert!(source_is_enabled(&json!(true)));
        assert!(source_is_enabled(&json!("required")));

        assert!(!source_is_required(&all_false));
        assert!(!source_is_required(&json!(true)));
        assert!(source_is_required(&any_required));
        assert!(source_is_required(&json!("required")));

        assert_eq!(get_talent_filter(&filter), filter.as_object().cloned());
        assert_eq!(get_talent_filter(&json!(false)), Some(Map::new()));
        assert_eq!(get_talent_filter(&json!(true)), None);
        assert_eq!(get_talent_filter(&json!("required")), None);
    }

    #[test]
    fn shipped_payload_does_not_discover_retired_chat_talents() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(3)
            .expect("talent-config crate is nested under the repository root")
            .join("core/payload");
        let configs = discover(&root.join("solstone/talent"), &root.join("solstone/apps"))
            .expect("discover shipped talent corpus");
        let keys: Vec<_> = configs.iter().map(|config| config.key.as_str()).collect();
        for retired in ["chat", "read", "exec", "support:support"] {
            assert!(
                !keys.contains(&retired),
                "retired talent {retired} must not be discovered"
            );
        }
    }
}
