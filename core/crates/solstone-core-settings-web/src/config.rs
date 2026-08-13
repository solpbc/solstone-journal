// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::PathBuf;

use axum::{body::Bytes, response::Response};
use serde_json::{Map, Value, json};
use solstone_core_journal_config_write::{
    JournalConfigMutation, LockError, LockOptions, mutate_journal_config,
};

use crate::{
    http::{
        config_busy, invalid_config_value, json_response, missing_request_body,
        missing_required_field, settings_operation_failed,
    },
    request_body::{JsonBody, json_body},
};

pub async fn get(journal_root: PathBuf) -> Response {
    let config = solstone_core_journal_config::read_journal_config(&journal_root)
        .expect("session gate handled corrupt config")
        .config
        .unwrap_or_default();
    match project_public_config(config) {
        Ok(config) => json_response(Value::Object(config)),
        Err(()) => settings_operation_failed(),
    }
}

pub async fn update(journal_root: PathBuf, lock_options: LockOptions, body: Bytes) -> Response {
    let JsonBody::Value(request) = json_body(body) else {
        return missing_request_body();
    };
    let Some(request) = request.as_object() else {
        return invalid_config_value("request must be an object");
    };
    let mut section = request
        .get("section")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    let mut data = request
        .get("data")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    if let (Some(section), Some(key), Some(value)) = (
        section.as_deref(),
        request.get("key").and_then(Value::as_str),
        request.get("value"),
    ) && data.is_empty()
    {
        data.insert(key.to_owned(), value.clone());
        let _ = section;
    }
    if section.is_none()
        && let Some(identity) = request.get("identity").and_then(Value::as_object)
    {
        section = Some("identity".to_owned());
        data = identity.clone();
    }
    let Some(section) = section else {
        return missing_required_field("No section specified");
    };
    let allowed: &[&str] = match section.as_str() {
        "identity" => &[
            "name",
            "preferred",
            "bio",
            "pronouns",
            "aliases",
            "email_addresses",
            "timezone",
        ],
        "journal" => &["name"],
        "transcribe" => &["backend", "preserve_all", "confidential_audio"],
        "support" => &["enabled", "proactive", "anonymous_feedback", "portal_url"],
        "agent" => &["name", "name_status", "named_date"],
        "env" => &["PLAUD_ACCESS_TOKEN"],
        "processing" => &[],
        _ => return invalid_config_value(format!("Unknown section: {section}")),
    };
    if section == "journal"
        && data
            .get("name")
            .and_then(Value::as_str)
            .is_some_and(|name| name.trim().is_empty())
    {
        return invalid_config_value("Journal name cannot be empty");
    }
    if section == "transcribe" {
        if data
            .get("backend")
            .is_some_and(|value| !matches!(value.as_str(), Some("parakeet" | "parakeet-cpp")))
        {
            let backend = data["backend"].as_str().unwrap_or_default();
            return invalid_config_value(format!(
                "Invalid backend: {backend}. Must be one of: parakeet, parakeet-cpp"
            ));
        }
        for key in ["preserve_all", "confidential_audio"] {
            if data.contains_key(key) && !data[key].is_boolean() {
                return invalid_config_value(format!("transcribe.{key} must be a boolean"));
            }
        }
    }
    let data_for_write = data.clone();
    let section_for_write = section.clone();
    let result = mutate_journal_config(&journal_root, lock_options, move |config| {
        let old = config
            .get(&section_for_write)
            .cloned()
            .unwrap_or_else(|| Value::Object(Map::new()));
        let target = config
            .entry(section_for_write.clone())
            .or_insert_with(|| Value::Object(Map::new()));
        let Some(target) = target.as_object_mut() else {
            return JournalConfigMutation {
                changed: false,
                value: Err(()),
            };
        };
        let mut changed = false;
        if section_for_write == "processing" {
            let mut next = Map::new();
            next.insert(
                "mode".to_owned(),
                data_for_write
                    .get("mode")
                    .cloned()
                    .or_else(|| old.get("mode").cloned())
                    .unwrap_or_else(|| json!("realtime")),
            );
            let gate = old
                .get("gate")
                .cloned()
                .unwrap_or_else(|| {
                    json!({"time_window":{"enabled":true,"start":"02:00","end":"06:00"},"display_powersave":{"enabled":false}})
                });
            next.insert("gate".to_owned(), gate);
            if old != Value::Object(next.clone()) {
                *target = next;
                changed = true;
            }
        } else {
            for key in allowed {
                if let Some(value) = data_for_write.get(*key) {
                    changed |= target.get(*key) != Some(value);
                    target.insert((*key).to_owned(), value.clone());
                }
            }
            if section_for_write == "transcribe" {
                for backend in ["parakeet", "parakeet-cpp"] {
                    if let Some(nested) = data_for_write.get(backend).and_then(Value::as_object) {
                        let inner = target
                            .entry(backend.to_owned())
                            .or_insert_with(|| Value::Object(Map::new()))
                            .as_object_mut()
                            .expect("created object");
                        for (key, value) in nested {
                            if ["model_version", "device", "timeout_sec"].contains(&key.as_str()) {
                                changed |= inner.get(key) != Some(value);
                                inner.insert(key.clone(), value.clone());
                            }
                        }
                    }
                }
            }
        }
        if section_for_write == "env"
            && data_for_write.contains_key("PLAUD_ACCESS_TOKEN")
            && let Some(validation) = config
                .get_mut("service_key_validation")
                .and_then(Value::as_object_mut)
        {
            validation.remove("plaud");
        }
        JournalConfigMutation {
            changed,
            value: Ok(config.clone()),
        }
    });
    let config = match result {
        Ok(value) => match value.value {
            Ok(config) => config,
            Err(()) => return invalid_config_value("section must be an object"),
        },
        Err(error) => {
            return match error {
                solstone_core_journal_config_write::ConfigMutationError::Lock(
                    LockError::Timeout(_),
                ) => config_busy(),
                _ => settings_operation_failed(),
            };
        }
    };
    if section == "env" && data.contains_key("PLAUD_ACCESS_TOKEN") {
        let _ = solstone_core_facets::append_action_log(
            &journal_root,
            None,
            "app",
            "settings",
            "env_update",
            json!({"changed_fields":{"PLAUD_ACCESS_TOKEN":{"old":"***","new":"***"}}}),
        );
    }
    let key_validation = config
        .get("service_key_validation")
        .and_then(Value::as_object)
        .and_then(|values| values.get("plaud"))
        .cloned()
        .map(|value| json!({"plaud": value}))
        .unwrap_or_else(|| json!({}));
    match project_public_config(config.clone()) {
        Ok(public) => json_response(
            json!({"config": public, "key_validation": key_validation, "success": true}),
        ),
        Err(()) => settings_operation_failed(),
    }
}

pub fn project_public_config(mut config: Map<String, Value>) -> Result<Map<String, Value>, ()> {
    let validation = config
        .get("service_key_validation")
        .and_then(Value::as_object)
        .and_then(|values| values.get("plaud"))
        .cloned();
    config.remove("service_key_validation");
    if let Some(value) = validation {
        config.insert("key_validation".to_owned(), json!({"plaud": value}));
    }
    let env = config.get("env").and_then(Value::as_object);
    config.insert(
        "env".to_owned(),
        json!({"PLAUD_ACCESS_TOKEN": env.and_then(|values| values.get("PLAUD_ACCESS_TOKEN")).is_some_and(truthy)}),
    );
    config.remove("providers");
    let convey = config
        .entry("convey".to_owned())
        .or_insert_with(|| Value::Object(Map::new()));
    let Value::Object(convey) = convey else {
        return Err(());
    };
    convey.remove("secret");
    convey.remove("password_hash");
    convey.remove("password");
    if let Some(transcribe) = config.get("transcribe").cloned() {
        config.insert(
            "transcribe".to_owned(),
            project_transcribe(transcribe, false),
        );
    }
    config.insert(
        "runtime_env".to_owned(),
        json!({"PLAUD_ACCESS_TOKEN": std::env::var_os("PLAUD_ACCESS_TOKEN").is_some_and(|value| !value.is_empty())}),
    );
    Ok(config)
}

pub fn project_transcribe(value: Value, include_confidential_audio: bool) -> Value {
    let Some(values) = value.as_object() else {
        return json!({});
    };
    let mut projected = Map::new();
    for (key, value) in values {
        match key.as_str() {
            "parakeet" | "parakeet-cpp" => {
                if let Some(nested) = value.as_object() {
                    let allowed: &[&str] = if key == "parakeet" {
                        &["model_version", "device", "timeout_sec"]
                    } else {
                        &["device"]
                    };
                    projected.insert(
                        key.clone(),
                        Value::Object(
                            nested
                                .iter()
                                .filter(|(nested_key, _)| allowed.contains(&nested_key.as_str()))
                                .map(|(nested_key, nested_value)| {
                                    (nested_key.clone(), nested_value.clone())
                                })
                                .collect(),
                        ),
                    );
                }
            }
            "backend" | "preserve_all" | "confidential_audio" | "min_speech_seconds"
                if !value.is_object() =>
            {
                projected.insert(key.clone(), value.clone());
            }
            _ => {}
        }
    }
    if !matches!(
        projected.get("backend").and_then(Value::as_str),
        Some("parakeet" | "parakeet-cpp")
    ) {
        projected.insert("backend".to_owned(), Value::String("parakeet".to_owned()));
    }
    if include_confidential_audio {
        projected.insert(
            "confidential_audio".to_owned(),
            Value::Bool(values.get("confidential_audio").map(truthy).unwrap_or(true)),
        );
    }
    Value::Object(projected)
}

pub fn truthy(value: &Value) -> bool {
    match value {
        Value::Null | Value::Bool(false) => false,
        Value::Bool(true) => true,
        Value::Number(number) => number.as_f64().is_none_or(|number| number != 0.0),
        Value::String(value) => !value.is_empty(),
        Value::Array(values) => !values.is_empty(),
        Value::Object(values) => !values.is_empty(),
    }
}

#[cfg(test)]
mod tests {
    use axum::{
        body::{Body, to_bytes},
        http::Request,
    };
    use serde_json::json;
    use tower::ServiceExt;

    #[tokio::test]
    async fn ac4_rich_config_preserves_future_section_and_exact_keys() {
        let root = crate::test_support::phase_root("rich");
        let response = crate::test_support::shell_router(root.path())
            .oneshot(
                Request::get("/app/settings/api/config")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        let config: serde_json::Value = serde_json::from_slice(
            &to_bytes(response.into_body(), usize::MAX)
                .await
                .expect("body"),
        )
        .expect("JSON");
        assert_eq!(config["some_future_section"], json!({"kept": true, "n": 7}));
        let mut keys = config
            .as_object()
            .expect("config object")
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        keys.sort();
        let mut expected =
            crate::test_support::corpus()["phases"]["rich"]["GET api/config"]["normalized"]
                .as_object()
                .expect("recorded config")
                .keys()
                .cloned()
                .collect::<Vec<_>>();
        expected.sort();
        assert_eq!(keys, expected);
    }

    #[tokio::test]
    async fn non_object_convey_returns_the_settings_operation_failure() {
        let root = crate::test_support::established_root();
        std::fs::write(
            root.path().join("config/journal.json"),
            serde_json::to_vec(&json!({
                "setup": {"completed_at": 1},
                "convey": "not an object",
            }))
            .expect("config JSON"),
        )
        .expect("config writes");
        let response = crate::test_support::shell_router(root.path())
            .oneshot(
                Request::get("/app/settings/api/config")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(
            response.status(),
            axum::http::StatusCode::INTERNAL_SERVER_ERROR
        );
        let body: serde_json::Value = serde_json::from_slice(
            &to_bytes(response.into_body(), usize::MAX)
                .await
                .expect("body"),
        )
        .expect("JSON");
        assert_eq!(body["reason_code"], "settings_operation_failed");
        assert_eq!(
            body["detail"],
            "something went wrong — try again, and if it persists, check the health dashboard"
        );
    }

    #[tokio::test]
    async fn ac5_tokened_config_masks_token_and_live_responses_never_leak_it() {
        for (phase, expected) in [("tokened", true), ("rich", false)] {
            let root = crate::test_support::phase_root(phase);
            let router = crate::test_support::shell_router(root.path());
            let config = router
                .clone()
                .oneshot(
                    Request::get("/app/settings/api/config")
                        .body(Body::empty())
                        .expect("request"),
                )
                .await
                .expect("response");
            let body: serde_json::Value = serde_json::from_slice(
                &to_bytes(config.into_body(), usize::MAX)
                    .await
                    .expect("body"),
            )
            .expect("JSON");
            assert_eq!(body["env"]["PLAUD_ACCESS_TOKEN"], expected, "{phase}");
            for name in crate::test_support::corpus()["phases"][phase]
                .as_object()
                .expect("phase")
                .keys()
                .filter(|name| name.starts_with("GET "))
            {
                let response = router
                    .clone()
                    .oneshot(
                        Request::get(crate::test_support::request_path(name))
                            .body(Body::empty())
                            .expect("request"),
                    )
                    .await
                    .expect("response");
                let bytes = to_bytes(response.into_body(), usize::MAX)
                    .await
                    .expect("body");
                let text = String::from_utf8_lossy(&bytes);
                assert!(
                    !text.contains("plaud-token-MUST-NOT-LEAK"),
                    "{phase} {name}"
                );
                assert!(!text.contains("MUST-NOT-LEAK"), "{phase} {name}");
            }
        }
    }
}
