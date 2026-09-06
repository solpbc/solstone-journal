// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use axum::{body::Bytes, response::Response};
use serde_json::{Map, Value};
use solstone_core_backup::HostedBinding;

use crate::response;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HandoffFieldError {
    Missing(&'static str),
    InvalidValue,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RefusedReason {
    NoHostedBackup,
    HostedBackupExpired,
}

impl RefusedReason {
    pub(crate) fn code(self) -> &'static str {
        match self {
            Self::NoHostedBackup => "no_hosted_backup",
            Self::HostedBackupExpired => "hosted_backup_expired",
        }
    }
}

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

pub(crate) fn nonempty_string(
    object: &Map<String, Value>,
    key: &'static str,
) -> Result<String, HandoffFieldError> {
    object
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .ok_or(HandoffFieldError::Missing(key))
}

pub(crate) fn hosted_binding(
    object: &Map<String, Value>,
) -> Result<HostedBinding, HandoffFieldError> {
    Ok(HostedBinding {
        broker_endpoint: nonempty_string(object, "broker_endpoint")?,
        account_id: nonempty_string(object, "account_id")?,
        instance_id: nonempty_string(object, "instance_id")?,
        bucket: nonempty_string(object, "bucket")?,
        prefix: nonempty_string(object, "prefix")?,
        broker_token: nonempty_string(object, "broker_token")?,
    })
}

pub(crate) fn refused_reason(
    object: &Map<String, Value>,
) -> Result<RefusedReason, HandoffFieldError> {
    if object.len() != 2 || object.get("status").and_then(Value::as_str) != Some("refused") {
        return Err(HandoffFieldError::InvalidValue);
    }
    match object.get("reason_code").and_then(Value::as_str) {
        Some("no_hosted_backup") => Ok(RefusedReason::NoHostedBackup),
        Some("hosted_backup_expired") => Ok(RefusedReason::HostedBackupExpired),
        _ => Err(HandoffFieldError::InvalidValue),
    }
}

fn canonical_origin(value: &str) -> Option<String> {
    let (scheme, rest) = value.split_once("://")?;
    if scheme != "https" {
        return None;
    }
    if rest.contains(['@', '?', '#']) {
        return None;
    }
    let rest = rest.strip_suffix('/').unwrap_or(rest);
    if rest.contains('/') {
        return None;
    }
    let (host, port) = match rest.split_once(':') {
        Some((host, port)) => (host, Some(port)),
        None => (rest, None),
    };
    if host.is_empty()
        || !host
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-'))
    {
        return None;
    }
    if let Some(port) = port
        && (!(1..=5).contains(&port.len())
            || !port.bytes().all(|byte| byte.is_ascii_digit())
            || port.parse::<u16>().is_err())
    {
        return None;
    }
    Some(format!("{scheme}://{rest}"))
}

pub(crate) fn require_configured_portal_base(portal_base: &str) -> Result<(), HandoffFieldError> {
    canonical_origin(portal_base)
        .map(|_| ())
        .ok_or(HandoffFieldError::InvalidValue)
}

pub(crate) fn require_portal_origin(
    candidate: &str,
    portal_base: &str,
) -> Result<(), HandoffFieldError> {
    let Some(base) = canonical_origin(portal_base) else {
        return Err(HandoffFieldError::InvalidValue);
    };
    let Some(origin) = canonical_origin(candidate) else {
        return Err(HandoffFieldError::InvalidValue);
    };
    if origin == base {
        Ok(())
    } else {
        Err(HandoffFieldError::InvalidValue)
    }
}

pub(crate) fn require_https_portal_url(
    url: &str,
    portal_base: &str,
) -> Result<(), HandoffFieldError> {
    let Some(after) = url.strip_prefix("https://") else {
        return Err(HandoffFieldError::InvalidValue);
    };
    let authority_end = after.find(['/', '?', '#']).unwrap_or(after.len());
    let authority = &after[..authority_end];
    if authority.contains('@') {
        return Err(HandoffFieldError::InvalidValue);
    }
    require_portal_origin(&format!("https://{authority}"), portal_base)
}

pub fn destination(value: &Map<String, Value>) -> Result<(), Response> {
    let _ = required_string(value, "repository")?;
    let backend = required_string(value, "backend")?;
    if backend != "s3" && backend != "b2" {
        return Err(response::error(
            axum::http::StatusCode::BAD_REQUEST,
            "one of those values couldn't be used.",
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
                "one of those values couldn't be used.",
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
            "that day couldn't be used.",
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
            "that day couldn't be used.",
            "invalid_day",
            "",
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const PORTAL: &str = "https://services.solstone.app";
    const HOSTED_BINDING_FIELDS: [&str; 6] = [
        "broker_endpoint",
        "account_id",
        "instance_id",
        "bucket",
        "prefix",
        "broker_token",
    ];

    fn binding_object() -> Map<String, Value> {
        json!({
            "broker_endpoint": PORTAL,
            "account_id": "account",
            "instance_id": "instance",
            "bucket": "bucket",
            "prefix": "owner/prefix",
            "broker_token": "broker-token-secret"
        })
        .as_object()
        .unwrap()
        .clone()
    }

    #[test]
    fn hosted_binding_requires_each_nonempty_string_field() {
        for field in HOSTED_BINDING_FIELDS {
            let mut missing = binding_object();
            missing.remove(field);
            assert_eq!(
                hosted_binding(&missing),
                Err(HandoffFieldError::Missing(field)),
                "{field} absent"
            );
            let mut blank = binding_object();
            blank.insert(field.to_owned(), json!(" "));
            assert_eq!(
                hosted_binding(&blank),
                Err(HandoffFieldError::Missing(field)),
                "{field} blank"
            );
            let mut wrong_type = binding_object();
            wrong_type.insert(field.to_owned(), json!(1));
            assert_eq!(
                hosted_binding(&wrong_type),
                Err(HandoffFieldError::Missing(field)),
                "{field} non-string"
            );
        }
        assert!(hosted_binding(&binding_object()).is_ok());
    }

    #[test]
    fn portal_origin_accepts_exact_and_one_trailing_slash() {
        assert!(require_portal_origin(PORTAL, PORTAL).is_ok());
        assert!(require_portal_origin("https://services.solstone.app/", PORTAL).is_ok());
        assert!(require_portal_origin(PORTAL, "https://services.solstone.app/").is_ok());
    }

    #[test]
    fn portal_origin_rejects_path_query_fragment_userinfo_and_extra_slash() {
        for candidate in [
            "https://services.solstone.app//",
            "https://services.solstone.app/backup",
            "https://user:pass@services.solstone.app",
            "https://services.solstone.app?x=1",
            "https://services.solstone.app#f",
            "http://services.solstone.app",
            "https://broker.example",
            "HTTPS://services.solstone.app",
            "https://broker.solstone.app",
            "https://services.solstone.app.evil.example",
            "https://services-solstone.app",
        ] {
            assert_eq!(
                require_portal_origin(candidate, PORTAL),
                Err(HandoffFieldError::InvalidValue),
                "{candidate}"
            );
        }
    }

    #[test]
    fn misconfigured_portal_base_validates_nothing() {
        let bad = "https://services.solstone.app/foo";
        assert_eq!(
            require_portal_origin(bad, bad),
            Err(HandoffFieldError::InvalidValue)
        );
        assert_eq!(
            require_portal_origin(PORTAL, bad),
            Err(HandoffFieldError::InvalidValue)
        );
    }

    #[test]
    fn subscribe_url_must_be_https_at_portal_origin() {
        assert!(
            require_https_portal_url("https://services.solstone.app/services/backup", PORTAL)
                .is_ok()
        );
        assert!(require_https_portal_url("https://services.solstone.app/", PORTAL).is_ok());
        assert!(require_https_portal_url(PORTAL, PORTAL).is_ok());
        assert_eq!(
            require_https_portal_url("http://services.solstone.app/services/backup", PORTAL),
            Err(HandoffFieldError::InvalidValue)
        );
        assert_eq!(
            require_https_portal_url("https://evil.example/subscribe", PORTAL),
            Err(HandoffFieldError::InvalidValue)
        );
        assert_eq!(
            require_https_portal_url(
                "https://user:pass@services.solstone.app/services/backup",
                PORTAL
            ),
            Err(HandoffFieldError::InvalidValue)
        );
        assert_eq!(
            require_https_portal_url("https://broker.solstone.app/services/backup", PORTAL),
            Err(HandoffFieldError::InvalidValue)
        );
    }

    #[test]
    fn http_origin_does_not_match_http_portal_base() {
        assert_eq!(
            require_portal_origin(
                "http://services.solstone.app",
                "http://services.solstone.app"
            ),
            Err(HandoffFieldError::InvalidValue)
        );
    }

    #[test]
    fn path_is_rejected_as_broker_endpoint_and_accepted_as_subscribe_url() {
        let url = "https://services.solstone.app/services/backup";
        assert_eq!(
            require_portal_origin(url, PORTAL),
            Err(HandoffFieldError::InvalidValue)
        );
        assert!(require_https_portal_url(url, PORTAL).is_ok());
    }

    #[test]
    fn configured_portal_base_accepts_https_origin_and_rejects_the_invalid_shapes() {
        assert!(require_configured_portal_base(PORTAL).is_ok());
        assert!(require_configured_portal_base("https://services.solstone.app/").is_ok());
        assert!(require_configured_portal_base("https://services.solstone.app:65535").is_ok());
        for base in [
            "http://services.solstone.app",
            "https://",
            "https://user:pass@services.solstone.app",
            "https://services.solstone.app/backup",
            "https://services.solstone.app?x=1",
            "https://services.solstone.app#f",
            "https://services.solstone.app//",
            "https://services.solstone.app:65536",
            "https://services.solstone.app:99999",
        ] {
            assert_eq!(
                require_configured_portal_base(base),
                Err(HandoffFieldError::InvalidValue),
                "{base}"
            );
        }
    }

    #[test]
    fn refused_reason_requires_the_exact_refused_envelope() {
        let valid = Map::from_iter([
            ("status".into(), json!("refused")),
            ("reason_code".into(), json!("no_hosted_backup")),
        ]);
        assert_eq!(refused_reason(&valid), Ok(RefusedReason::NoHostedBackup));

        let expired = Map::from_iter([
            ("status".into(), json!("refused")),
            ("reason_code".into(), json!("hosted_backup_expired")),
        ]);
        assert_eq!(
            refused_reason(&expired),
            Ok(RefusedReason::HostedBackupExpired)
        );

        for invalid in [
            Map::from_iter([(String::from("status"), json!("refused"))]),
            Map::from_iter([
                ("status".into(), json!("approved")),
                ("reason_code".into(), json!("no_hosted_backup")),
            ]),
            Map::from_iter([
                ("status".into(), json!("refused")),
                ("reason_code".into(), json!("unknown")),
            ]),
            Map::from_iter([
                ("status".into(), json!("refused")),
                ("reason_code".into(), json!("no_hosted_backup")),
                ("extra".into(), json!(true)),
            ]),
        ] {
            assert_eq!(
                refused_reason(&invalid),
                Err(HandoffFieldError::InvalidValue)
            );
        }
    }
}
