// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::PathBuf;
use std::time::Duration;

use serde_json::{Map, Value};
use solstone_core_cogitate::capabilities_for_access_tier;
use solstone_core_cogitate_runtime::{RunConfig, RunInput};
use thiserror::Error;

pub const REQUEST_SCHEMA: &str = "solstone-cogitate-request-v1";

/// A fully validated stdin request for one native cogitate run.
#[derive(Clone, Debug, PartialEq)]
pub struct CogitateRequest {
    pub schema: String,
    pub access_tier: String,
    pub outbound_approval: Option<String>,
    pub expects_emit_final: bool,
    pub max_turns: usize,
    pub cost_cap_usd: f64,
    pub context_window: Option<u64>,
    pub timeout_ms: u64,
    pub read_call_budget: i64,
    pub model: String,
    pub correlation_id: String,
    pub initial_prompt: String,
    pub system_instruction: Option<String>,
    pub journal_root: PathBuf,
    pub dry_run: bool,
}

#[derive(Debug, Error, PartialEq, Eq)]
#[error("malformed request: {message}")]
pub struct MalformedRequest {
    message: String,
}

impl CogitateRequest {
    pub fn parse(input: &str) -> Result<Self, MalformedRequest> {
        let value: Value = serde_json::from_str(input)
            .map_err(|error| malformed(format!("invalid JSON: {error}")))?;
        Self::from_value(&value)
    }

    pub fn from_value(value: &Value) -> Result<Self, MalformedRequest> {
        let object = value
            .as_object()
            .ok_or_else(|| malformed("request must be a JSON object"))?;
        reject_unknown_fields(object)?;

        let schema = required_string(object, "schema")?;
        if schema != REQUEST_SCHEMA {
            return Err(malformed(format!(
                "schema must be {REQUEST_SCHEMA:?}, got {schema:?}"
            )));
        }
        let access_tier = required_string(object, "access_tier")?;
        capabilities_for_access_tier(&access_tier)
            .map_err(|_| malformed(format!("invalid access_tier {access_tier:?}")))?;
        let journal_root = PathBuf::from(required_string(object, "journal_root")?);
        if !journal_root.is_absolute() {
            return Err(malformed("journal_root must be an absolute path"));
        }

        Ok(Self {
            schema,
            access_tier,
            outbound_approval: optional_string(object, "outbound_approval")?,
            expects_emit_final: required_bool(object, "expects_emit_final")?,
            max_turns: required_positive_usize(object, "max_turns")?,
            cost_cap_usd: required_positive_f64(object, "cost_cap_usd")?,
            context_window: optional_positive_u64(object, "context_window")?,
            timeout_ms: required_positive_u64(object, "timeout_ms")?,
            read_call_budget: required_positive_i64(object, "read_call_budget")?,
            model: required_string(object, "model")?,
            correlation_id: required_string(object, "correlation_id")?,
            initial_prompt: required_string(object, "initial_prompt")?,
            system_instruction: optional_string(object, "system_instruction")?,
            journal_root,
            dry_run: optional_bool(object, "dry_run")?.unwrap_or(false),
        })
    }

    pub fn to_run_input(&self) -> RunInput {
        RunInput {
            config: RunConfig {
                access_tier: self.access_tier.clone(),
                outbound_approval: self.outbound_approval.clone(),
                expects_emit_final: self.expects_emit_final,
                max_turns: self.max_turns,
                cost_cap_usd: self.cost_cap_usd,
                context_window: self.context_window,
                timeout: Duration::from_millis(self.timeout_ms),
                read_call_budget: self.read_call_budget,
                model: self.model.clone(),
                correlation_id: self.correlation_id.clone(),
            },
            initial_prompt: self.initial_prompt.clone(),
            system_instruction: self.system_instruction.clone(),
            journal_root: self.journal_root.clone(),
        }
    }
}

fn reject_unknown_fields(object: &Map<String, Value>) -> Result<(), MalformedRequest> {
    const FIELDS: &[&str] = &[
        "schema",
        "access_tier",
        "outbound_approval",
        "expects_emit_final",
        "max_turns",
        "cost_cap_usd",
        "context_window",
        "timeout_ms",
        "read_call_budget",
        "model",
        "correlation_id",
        "initial_prompt",
        "system_instruction",
        "journal_root",
        "dry_run",
    ];
    if let Some(field) = object
        .keys()
        .find(|field| !FIELDS.contains(&field.as_str()))
    {
        return Err(malformed(format!("unknown field {field:?}")));
    }
    Ok(())
}

fn required_string(object: &Map<String, Value>, field: &str) -> Result<String, MalformedRequest> {
    object
        .get(field)
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .ok_or_else(|| malformed(format!("{field} must be a string")))
}

fn optional_string(
    object: &Map<String, Value>,
    field: &str,
) -> Result<Option<String>, MalformedRequest> {
    match object.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.clone())),
        Some(_) => Err(malformed(format!("{field} must be a string or null"))),
    }
}

fn required_bool(object: &Map<String, Value>, field: &str) -> Result<bool, MalformedRequest> {
    object
        .get(field)
        .and_then(Value::as_bool)
        .ok_or_else(|| malformed(format!("{field} must be a boolean")))
}

fn optional_bool(
    object: &Map<String, Value>,
    field: &str,
) -> Result<Option<bool>, MalformedRequest> {
    match object.get(field) {
        None => Ok(None),
        Some(value) => value
            .as_bool()
            .map(Some)
            .ok_or_else(|| malformed(format!("{field} must be a boolean"))),
    }
}

fn required_positive_usize(
    object: &Map<String, Value>,
    field: &str,
) -> Result<usize, MalformedRequest> {
    object
        .get(field)
        .and_then(Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
        .filter(|value| *value > 0)
        .ok_or_else(|| malformed(format!("{field} must be a positive integer")))
}

fn required_positive_u64(
    object: &Map<String, Value>,
    field: &str,
) -> Result<u64, MalformedRequest> {
    object
        .get(field)
        .and_then(Value::as_u64)
        .filter(|value| *value > 0)
        .ok_or_else(|| malformed(format!("{field} must be a positive integer")))
}

fn required_positive_i64(
    object: &Map<String, Value>,
    field: &str,
) -> Result<i64, MalformedRequest> {
    object
        .get(field)
        .and_then(Value::as_i64)
        .filter(|value| *value > 0)
        .ok_or_else(|| malformed(format!("{field} must be a positive integer")))
}

fn optional_positive_u64(
    object: &Map<String, Value>,
    field: &str,
) -> Result<Option<u64>, MalformedRequest> {
    match object.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_u64()
            .filter(|value| *value > 0)
            .map(Some)
            .ok_or_else(|| malformed(format!("{field} must be a positive integer or null"))),
    }
}

fn required_positive_f64(
    object: &Map<String, Value>,
    field: &str,
) -> Result<f64, MalformedRequest> {
    object
        .get(field)
        .and_then(Value::as_f64)
        .filter(|value| value.is_finite() && *value > 0.0)
        .ok_or_else(|| malformed(format!("{field} must be a positive number")))
}

fn malformed(message: impl Into<String>) -> MalformedRequest {
    MalformedRequest {
        message: message.into(),
    }
}
