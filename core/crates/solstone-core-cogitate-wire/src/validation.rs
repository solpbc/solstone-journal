// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::sync::OnceLock;

use serde_json::{Map, Value};
use thiserror::Error;

const CONTRACT: &str = include_str!("../../../fixtures/cogitate_wire_contract.json");
static CONTRACT_VALUE: OnceLock<Value> = OnceLock::new();

/// Return the checked-in cogitate wire contract bytes for `--contract`.
pub fn contract_source() -> &'static str {
    CONTRACT
}

#[derive(Debug, Error, PartialEq, Eq)]
#[error("event validation failed: {message}")]
pub struct ValidationError {
    message: String,
}

/// Validate one native event object against the checked-in wire contract.
///
/// The serializer and all tests share this one implementation, so a future
/// stdout adapter cannot silently diverge from the fixture's field rules.
pub fn validate_event(value: &Value) -> Result<(), ValidationError> {
    let object = value
        .as_object()
        .ok_or_else(|| invalid("event must be a JSON object"))?;
    let event = object
        .get("event")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid("event must contain a string event field"))?;
    let contract = contract()
        .get("cortex_events")
        .and_then(Value::as_object)
        .and_then(|events| events.get(event))
        .and_then(Value::as_object)
        .ok_or_else(|| invalid(format!("unknown event kind {event:?}")))?;
    let required = fields(contract, "required_fields")?;
    let optional = fields(contract, "optional_fields")?;

    for (field, rule) in required {
        let value = object
            .get(field)
            .ok_or_else(|| invalid(format!("{event} missing required field {field:?}")))?;
        if !matches_rule(value, rule) {
            return Err(invalid(format!("{event} field {field:?} must be {rule:?}")));
        }
    }
    for (field, rule) in optional {
        if let Some(value) = object.get(field)
            && !matches_rule(value, rule)
        {
            return Err(invalid(format!("{event} field {field:?} must be {rule:?}")));
        }
    }
    for field in object.keys() {
        if !required.contains_key(field) && !optional.contains_key(field) {
            return Err(invalid(format!(
                "{event} contains undeclared field {field:?}"
            )));
        }
    }
    if object.get("event").and_then(Value::as_str) != Some(event) {
        return Err(invalid("event field does not match its contract entry"));
    }
    Ok(())
}

fn contract() -> &'static Value {
    CONTRACT_VALUE.get_or_init(|| {
        serde_json::from_str(CONTRACT).expect("cogitate wire contract fixture is valid JSON")
    })
}

fn fields<'a>(
    event_contract: &'a Map<String, Value>,
    name: &str,
) -> Result<&'a Map<String, Value>, ValidationError> {
    event_contract
        .get(name)
        .and_then(Value::as_object)
        .ok_or_else(|| invalid(format!("contract {name:?} must be an object")))
}

fn matches_rule(value: &Value, rule: &Value) -> bool {
    match rule.as_str() {
        Some("string") => value.is_string(),
        Some("integer") => value.as_i64().is_some() || value.as_u64().is_some(),
        Some("boolean") => value.is_boolean(),
        Some("object") => value.is_object(),
        Some("json") => true,
        Some("null_or_string") => value.is_null() || value.is_string(),
        _ => false,
    }
}

fn invalid(message: impl Into<String>) -> ValidationError {
    ValidationError {
        message: message.into(),
    }
}
