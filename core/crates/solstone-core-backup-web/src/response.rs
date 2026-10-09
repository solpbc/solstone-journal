use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde_json::{Value, json};

pub fn success(value: Value) -> Response {
    (StatusCode::OK, Json(value)).into_response()
}

pub fn error(status: StatusCode, message: &str, reason_code: &str, detail: &str) -> Response {
    error_with_guidance(status, message, reason_code, detail, None)
}

pub fn error_with_guidance(
    status: StatusCode,
    message: &str,
    reason_code: &str,
    detail: &str,
    guidance: Option<&str>,
) -> Response {
    let mut payload = json!({
        "error": message,
        "reason_code": reason_code,
        "detail": detail,
    });
    if let Some(guidance) = guidance {
        payload
            .as_object_mut()
            .unwrap()
            .insert("guidance".into(), json!(guidance));
    }
    (status, Json(payload)).into_response()
}

pub fn invalid_config(detail: &str) -> Response {
    error(
        StatusCode::BAD_REQUEST,
        "that setting couldn't be saved because one value was invalid.",
        "invalid_config_value",
        detail,
    )
}

pub fn missing(detail: &str) -> Response {
    error(
        StatusCode::BAD_REQUEST,
        "a required field is missing.",
        "missing_required_field",
        detail,
    )
}
