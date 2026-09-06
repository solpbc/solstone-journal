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
    (
        status,
        Json(json!({"error": message, "reason_code": reason_code, "detail": detail})),
    )
        .into_response()
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
