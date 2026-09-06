// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Native Thinking writes retained from the Python Sol application.

use std::sync::Arc;

use axum::Json;
use axum::body::to_bytes;
use axum::extract::{Extension, Request};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::{Map, Value, json};
use solstone_core_convey_http::envelope::error_envelope;
use solstone_core_identity::{IdentityError, ensure_identity_directory};
use solstone_core_journal_config::is_path_shaped_name;
use solstone_core_journal_config_write::{
    CasConfigMutationError, JournalConfigMutation, LockError, mutate_journal_config_cas,
};

use crate::JournalRoot;

const IDENTITY_BUSY: &str = "your journal's identity couldn't be updated right now because it was busy. try again in a moment.";

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
    if is_path_shaped_name(name) {
        return invalid_config_value("owner name must not be a path");
    }
    let bio = body.get("bio").cloned().unwrap_or(Value::Null);

    match mutate_journal_config_cas(&journal.0, |config| {
        let mut identity = match object_or_default(config.get("identity"), "identity", Map::new) {
            Ok(identity) => identity,
            Err(detail) => {
                return JournalConfigMutation {
                    changed: false,
                    value: Err(detail),
                };
            }
        };
        let previous = identity.clone();
        identity.insert("name".to_owned(), json!(name));
        if !bio.is_null() {
            identity.insert("bio".to_owned(), bio.clone());
        }
        config.insert("identity".to_owned(), Value::Object(identity.clone()));
        JournalConfigMutation {
            changed: identity != previous,
            value: Ok(()),
        }
    }) {
        Ok(transaction) => match transaction.value {
            Ok(()) => Json(json!({"name": name, "bio": response_bio(&bio)})).into_response(),
            Err(detail) => operation_failed(detail),
        },
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
    // Unlike Flask's get_json(silent=True), this does not inspect Content-Type.
    // Raw-byte parsing matches speakers_cli_owner; checking only here would be
    // inconsistent, and the released `sol call sol` client sends application/json.
    let bytes = to_bytes(request.into_body(), usize::MAX)
        .await
        .map_err(|_| missing_body("Unable to read request body"))?;
    if bytes.is_empty() {
        return Err(missing_body("no request body"));
    }
    serde_json::from_slice::<Value>(&bytes)
        .ok()
        .and_then(|value| value.as_object().cloned())
        .ok_or_else(invalid_json)
}

fn required<'a>(body: &'a Map<String, Value>, field: &str) -> Option<&'a str> {
    body.get(field)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
}

fn object_or_default(
    value: Option<&Value>,
    key: &str,
    default: impl FnOnce() -> Map<String, Value>,
) -> Result<Map<String, Value>, String> {
    match value {
        Some(Value::Object(value)) => Ok(value.clone()),
        None => Ok(default()),
        Some(_) => Err(format!("{key} must be a JSON object")),
    }
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

fn invalid_config_value(detail: &str) -> Response {
    error_envelope(
        "invalid_config_value",
        "that setting couldn't be saved because one value was invalid.",
        detail,
        StatusCode::BAD_REQUEST,
    )
    .into_response()
}

fn missing_body(detail: &str) -> Response {
    error_envelope(
        "missing_request_body",
        "that request had no data in it.",
        detail,
        StatusCode::BAD_REQUEST,
    )
    .into_response()
}

fn invalid_json() -> Response {
    error_envelope(
        "invalid_json_request",
        "that JSON request couldn't be read.",
        "request body must be a JSON object",
        StatusCode::BAD_REQUEST,
    )
    .into_response()
}

fn required_field(field: &str) -> Response {
    error_envelope(
        "missing_required_field",
        "a required field is missing.",
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
    // Flask lets non-timeout exceptions reach its generic handler. Native routes
    // return internal_error instead; only Timeout maps to identity_busy, so no
    // other failure can be misreported as busy.
    error_envelope(
        "internal_error",
        "your journal's identity couldn't be updated right now.",
        detail,
        StatusCode::INTERNAL_SERVER_ERROR,
    )
    .into_response()
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::truthy;

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
}
