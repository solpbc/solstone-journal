// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Native Thinking writes retained from the Python Sol application.

use std::sync::Arc;

use axum::Json;
use axum::body::to_bytes;
use axum::extract::{Extension, Request};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use chrono::Utc;
use serde_json::{Map, Value, json};
use solstone_core_convey_http::envelope::error_envelope;
use solstone_core_identity::{IdentityError, ensure_identity_directory};
use solstone_core_journal_config_write::{
    CasConfigMutationError, JournalConfigMutation, LockError, mutate_journal_config_cas,
};

use crate::JournalRoot;

const IDENTITY_BUSY: &str =
    "I couldn't update my identity right now because it was busy. Try again in a moment.";

pub(crate) async fn api_set_name(
    Extension(journal): Extension<Arc<JournalRoot>>,
    request: Request,
) -> Response {
    let body = match json_body(request).await {
        Ok(body) => body,
        Err(response) => return response,
    };
    let Some(name) = required(&body, "name") else {
        return required_field("name");
    };
    let status = body
        .get("status")
        .filter(|value| truthy(value))
        .cloned()
        .unwrap_or_else(|| json!("chosen"));
    let today = Utc::now().format("%Y-%m-%d").to_string();

    match mutate_journal_config_cas(&journal.0, |config| {
        let mut agent = object_or_default(config.get("agent"), default_agent);
        let changed = apply_set_name(&mut agent, name, status.clone(), &today);
        config.insert("agent".to_owned(), Value::Object(agent.clone()));
        JournalConfigMutation {
            changed,
            value: agent,
        }
    }) {
        Ok(transaction) => Json(Value::Object(transaction.value)).into_response(),
        Err(CasConfigMutationError::Timeout(_)) => identity_busy(),
        Err(error) => operation_failed(error.to_string()),
    }
}

pub(crate) async fn api_reset(Extension(journal): Extension<Arc<JournalRoot>>) -> Response {
    match mutate_journal_config_cas(&journal.0, |config| {
        let mut agent = object_or_default(config.get("agent"), default_agent);
        let previous = agent.clone();
        agent.insert("name".to_owned(), json!("sol"));
        agent.insert("name_status".to_owned(), json!("default"));
        agent.insert("named_date".to_owned(), Value::Null);
        config.insert("agent".to_owned(), Value::Object(agent.clone()));
        JournalConfigMutation {
            changed: agent != previous,
            value: agent,
        }
    }) {
        Ok(transaction) => Json(Value::Object(transaction.value)).into_response(),
        Err(CasConfigMutationError::Timeout(_)) => identity_busy(),
        Err(error) => operation_failed(error.to_string()),
    }
}

pub(crate) async fn api_set_owner(
    Extension(journal): Extension<Arc<JournalRoot>>,
    request: Request,
) -> Response {
    let body = match json_body(request).await {
        Ok(body) => body,
        Err(response) => return response,
    };
    let Some(name) = required(&body, "name") else {
        return required_field("name");
    };
    let bio = body.get("bio").cloned().unwrap_or(Value::Null);

    match mutate_journal_config_cas(&journal.0, |config| {
        let mut identity = object_or_default(config.get("identity"), Map::new);
        let previous = identity.clone();
        identity.insert("name".to_owned(), json!(name));
        if !bio.is_null() {
            identity.insert("bio".to_owned(), bio.clone());
        }
        config.insert("identity".to_owned(), Value::Object(identity.clone()));
        JournalConfigMutation {
            changed: identity != previous,
            value: (),
        }
    }) {
        Ok(_) => Json(json!({"name": name, "bio": response_bio(&bio)})).into_response(),
        Err(CasConfigMutationError::Timeout(_)) => identity_busy(),
        Err(error) => operation_failed(error.to_string()),
    }
}

pub(crate) async fn api_sol_init(Extension(journal): Extension<Arc<JournalRoot>>) -> Response {
    match ensure_identity_directory(&journal.0) {
        Ok(identity_dir) => {
            Json(json!({"identity_dir": identity_dir, "status": "ok"})).into_response()
        }
        Err(IdentityError::Lock(LockError::Timeout(_))) => identity_busy(),
        Err(error) => operation_failed(error.to_string()),
    }
}

async fn json_body(request: Request) -> Result<Map<String, Value>, Response> {
    let bytes = to_bytes(request.into_body(), usize::MAX)
        .await
        .map_err(|_| missing_body("Unable to read request body"))?;
    if bytes.is_empty() {
        return Err(missing_body("no request body"));
    }
    serde_json::from_slice::<Value>(&bytes)
        .ok()
        .and_then(|value| value.as_object().cloned())
        .ok_or_else(|| invalid_json())
}

fn required<'a>(body: &'a Map<String, Value>, field: &str) -> Option<&'a str> {
    body.get(field)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
}

fn default_agent() -> Map<String, Value> {
    [
        ("name".to_owned(), json!("sol")),
        ("name_status".to_owned(), json!("default")),
        ("named_date".to_owned(), Value::Null),
    ]
    .into_iter()
    .collect()
}

fn object_or_default(
    value: Option<&Value>,
    default: impl FnOnce() -> Map<String, Value>,
) -> Map<String, Value> {
    value
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_else(default)
}

fn apply_set_name(agent: &mut Map<String, Value>, name: &str, status: Value, today: &str) -> bool {
    let previous = agent.clone();
    agent.insert("name".to_owned(), json!(name));
    agent.insert("name_status".to_owned(), status);
    agent.insert("named_date".to_owned(), json!(today));
    agent != &previous
}

fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(value) => *value,
        Value::Number(value) => value.as_f64() != Some(0.0),
        Value::String(value) => !value.is_empty(),
        Value::Array(value) => !value.is_empty(),
        Value::Object(value) => !value.is_empty(),
    }
}

fn response_bio(bio: &Value) -> Value {
    if truthy(bio) { bio.clone() } else { json!("") }
}

fn missing_body(detail: &str) -> Response {
    error_envelope(
        "missing_request_body",
        "I couldn't find any data in that request.",
        detail,
        StatusCode::BAD_REQUEST,
    )
    .into_response()
}

fn invalid_json() -> Response {
    error_envelope(
        "invalid_json_request",
        "I couldn't read that JSON request.",
        "request body must be a JSON object",
        StatusCode::BAD_REQUEST,
    )
    .into_response()
}

fn required_field(field: &str) -> Response {
    error_envelope(
        "missing_required_field",
        "I couldn't find a required field.",
        format!("{field} is required"),
        StatusCode::BAD_REQUEST,
    )
    .into_response()
}

fn identity_busy() -> Response {
    error_envelope(
        "identity_busy",
        IDENTITY_BUSY,
        "identity is busy; try again",
        StatusCode::SERVICE_UNAVAILABLE,
    )
    .into_response()
}

fn operation_failed(detail: String) -> Response {
    error_envelope(
        "internal_error",
        "I couldn't update my identity right now.",
        detail,
        StatusCode::INTERNAL_SERVER_ERROR,
    )
    .into_response()
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::{apply_set_name, truthy};

    #[test]
    fn truthy_matches_python_json_value_truthiness() {
        for value in [
            Value::Null,
            json!(false),
            json!(0),
            json!(0.0),
            json!(""),
            json!([]),
            json!({}),
        ] {
            assert!(!truthy(&value), "{value}");
        }

        for value in [json!(123), json!(u64::MAX), json!("x"), json!(["x"])] {
            assert!(truthy(&value), "{value}");
        }
    }

    #[test]
    fn set_name_is_a_same_day_no_op_and_changes_on_the_next_day() {
        let mut agent = serde_json::from_value(json!({
            "name": "Nova",
            "name_status": "chosen",
            "named_date": "2026-08-15",
            "sibling": true,
        }))
        .unwrap();

        assert!(!apply_set_name(
            &mut agent,
            "Nova",
            json!("chosen"),
            "2026-08-15"
        ));
        assert!(apply_set_name(
            &mut agent,
            "Nova",
            json!("chosen"),
            "2026-08-16"
        ));
        assert_eq!(agent["sibling"], json!(true));
    }
}
