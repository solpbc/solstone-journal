// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::PathBuf;

use axum::response::Response;
use serde_json::{Map, Value, json};

use crate::http::json_response;

pub async fn get(journal_root: PathBuf) -> Response {
    let config = solstone_core_journal_config::read_journal_config(&journal_root)
        .expect("session gate handled corrupt config")
        .config
        .unwrap_or_default();
    json_response(Value::Object(project_public_config(config)))
}

pub fn project_public_config(mut config: Map<String, Value>) -> Map<String, Value> {
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
    if let Value::Object(convey) = convey {
        convey.remove("secret");
        convey.remove("password_hash");
        convey.remove("password");
    }
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
    config
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

    use super::project_public_config;

    #[test]
    fn ac4_rich_config_preserves_future_section_and_exact_keys() {
        let config = project_public_config(
            serde_json::from_value(json!({
                "setup": {"completed_at": 1}, "some_future_section": {"kept": true, "n": 7},
                "providers": {"private": true}, "env": {"PLAUD_ACCESS_TOKEN": ""},
                "service_key_validation": {"plaud": {"valid": false}, "bogus": {"valid": true}},
                "convey": {"secret": "no", "bind": "127.0.0.1"},
            }))
            .expect("map"),
        );
        assert_eq!(config["some_future_section"], json!({"kept": true, "n": 7}));
        let mut keys = config.keys().cloned().collect::<Vec<_>>();
        keys.sort();
        assert_eq!(
            keys,
            vec![
                "convey",
                "env",
                "key_validation",
                "runtime_env",
                "setup",
                "some_future_section"
            ]
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
