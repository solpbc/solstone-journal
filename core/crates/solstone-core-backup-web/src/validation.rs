#![allow(clippy::result_large_err)] // Validation returns the exact HTTP refusal envelope.

use axum::{body::Bytes, response::Response};
use serde_json::{Map, Value};

use crate::response;

pub fn object(
    body: &Bytes,
    missing_reason: fn(&str) -> Response,
) -> Result<Map<String, Value>, Response> {
    serde_json::from_slice::<Value>(body)
        .ok()
        .and_then(|value| value.as_object().cloned())
        .ok_or_else(|| missing_reason("missing request body"))
}
pub fn required_string(value: &Map<String, Value>, key: &str) -> Result<String, Response> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .ok_or_else(|| response::missing(&format!("missing {key}")))
}
pub fn destination(value: &Map<String, Value>) -> Result<(), Response> {
    let _ = required_string(value, "repository")?;
    let backend = required_string(value, "backend")?;
    if backend != "s3" && backend != "b2" {
        return Err(response::error(
            axum::http::StatusCode::BAD_REQUEST,
            "I couldn't use one of those values.",
            "invalid_request_value",
            "unsupported backend",
        ));
    }
    let source = value
        .get("credentials")
        .and_then(Value::as_object)
        .unwrap_or(value);
    for key in if backend == "s3" {
        ["access_key_id", "secret_access_key"]
    } else {
        ["account_id", "account_key"]
    } {
        let present = source
            .get(key)
            .and_then(Value::as_str)
            .is_some_and(|item| !item.trim().is_empty());
        if !present {
            return Err(response::missing(&format!("missing {key}")));
        }
    }
    Ok(())
}
pub fn retention(value: &Map<String, Value>) -> Result<Map<String, Value>, Response> {
    let mut result = Map::new();
    for key in ["hourly", "daily", "weekly", "monthly"] {
        let Some(value) = value.get(key) else {
            return Err(response::invalid_config(""));
        };
        let number = match value {
            Value::Number(number) if number.as_i64().is_some() => number.as_i64().unwrap(),
            Value::String(text)
                if text.is_ascii()
                    && !text.is_empty()
                    && text.chars().all(|item| item.is_ascii_digit()) =>
            {
                text.parse().map_err(|_| response::invalid_config(""))?
            }
            _ => return Err(response::invalid_config("")),
        };
        if number < 0 {
            return Err(response::invalid_config(""));
        }
        result.insert(key.to_owned(), Value::from(number));
    }
    Ok(result)
}
pub fn offload(value: &Map<String, Value>) -> Result<(i64, i64), Response> {
    let number = |key| {
        value
            .get(key)
            .and_then(Value::as_i64)
            .filter(|value| *value > 0)
            .ok_or_else(|| response::invalid_config(""))
    };
    Ok((number("budget_bytes")?, number("floor_bytes")?))
}
pub fn restore_day(value: &Map<String, Value>) -> Result<(), Response> {
    if value.get("all") == Some(&Value::Bool(true)) {
        return if value.contains_key("day") {
            Err(response::error(
                axum::http::StatusCode::BAD_REQUEST,
                "I couldn't use one of those values.",
                "invalid_request_value",
                "",
            ))
        } else {
            Ok(())
        };
    }
    let Some(day) = value.get("day").and_then(Value::as_str) else {
        return Err(response::error(
            axum::http::StatusCode::BAD_REQUEST,
            "I couldn't use that day.",
            "invalid_day",
            "",
        ));
    };
    let valid = day.len() == 8
        && day.bytes().all(|byte| byte.is_ascii_digit())
        && chrono::NaiveDate::parse_from_str(day, "%Y%m%d").is_ok();
    valid.then_some(()).ok_or_else(|| {
        response::error(
            axum::http::StatusCode::BAD_REQUEST,
            "I couldn't use that day.",
            "invalid_day",
            "",
        )
    })
}
