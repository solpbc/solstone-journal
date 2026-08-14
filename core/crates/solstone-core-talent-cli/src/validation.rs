// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::Value;
use solstone_core_cogitate::TALENT_ACCESS_TIERS;

use crate::discovery::TalentConfig;

pub(crate) fn validate(configs: &mut [TalentConfig]) -> Result<(), String> {
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

pub(crate) fn validate_write(
    config: &TalentConfig,
    talent_type: Option<&str>,
) -> Result<(), String> {
    if talent_type == Some("cogitate") && config.metadata.get("write").is_some_and(is_truthy) {
        return Err(format!(
            "Prompt '{}' declares unsupported 'write: true' (cogitate runs are read-only)",
            config.key
        ));
    }
    Ok(())
}

pub(crate) fn validate_access_tier(
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

pub(crate) fn validate_cwd(
    config: &mut TalentConfig,
    talent_type: Option<&str>,
) -> Result<(), String> {
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

pub(crate) fn is_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(value) => *value,
        Value::Number(value) => value.as_f64().is_none_or(|value| value != 0.0),
        Value::String(value) => !value.is_empty(),
        Value::Array(value) => !value.is_empty(),
        Value::Object(value) => !value.is_empty(),
    }
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
