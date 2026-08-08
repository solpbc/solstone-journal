// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::{Map, Value};
use solstone_core_local::GenerateFailure;

#[derive(Debug, Clone, PartialEq)]
pub enum LaneOutcome {
    NoEngine,
    BundledLocal,
    AttestationNotVerified,
    UnimplementedLane,
    BundledFailure(Box<GenerateFailure>),
}

pub fn resolve_lane(config: &Map<String, Value>) -> (String, LaneOutcome) {
    let provider = string_at(config, &["providers", "active", "provider"])
        .filter(|provider| !provider.is_empty())
        .unwrap_or("none")
        .to_owned();
    if provider == "none" {
        return (provider, LaneOutcome::NoEngine);
    }
    if provider != "local" {
        return (provider, LaneOutcome::UnimplementedLane);
    }

    let endpoint_url = string_at(config, &["providers", "local", "endpoint_url"]).unwrap_or("");
    let served_model_id =
        string_at(config, &["providers", "local", "served_model_id"]).unwrap_or("");
    if endpoint_url.is_empty() || served_model_id.is_empty() {
        return (provider, LaneOutcome::BundledLocal);
    }
    let confidential = config
        .get("services")
        .and_then(Value::as_object)
        .and_then(|services| services.get("confidential"))
        .is_some_and(Value::is_object);
    (
        provider,
        if confidential {
            LaneOutcome::AttestationNotVerified
        } else {
            LaneOutcome::UnimplementedLane
        },
    )
}

fn string_at<'a>(config: &'a Map<String, Value>, path: &[&str]) -> Option<&'a str> {
    let (first, rest) = path.split_first()?;
    let mut value = config.get(*first)?;
    for key in rest {
        value = value.as_object()?.get(*key)?;
    }
    value.as_str().map(str::trim)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn config(value: Value) -> Map<String, Value> {
        value.as_object().unwrap().clone()
    }

    #[test]
    fn resolves_every_n1a_lane() {
        let cases = [
            (json!({}), "none", LaneOutcome::NoEngine),
            (
                json!({"providers": {"active": {"provider": "local"}}, "services": {"confidential": {}}}),
                "local",
                LaneOutcome::BundledLocal,
            ),
            (
                json!({"providers": {"active": {"provider": "local"}, "local": {"endpoint_url": "https://endpoint", "served_model_id": "served"}}, "services": {"confidential": {}}}),
                "local",
                LaneOutcome::AttestationNotVerified,
            ),
            (
                json!({"providers": {"active": {"provider": "local"}, "local": {"endpoint_url": "https://endpoint", "served_model_id": "served"}}}),
                "local",
                LaneOutcome::UnimplementedLane,
            ),
            (
                json!({"providers": {"active": {"provider": "openai"}}}),
                "openai",
                LaneOutcome::UnimplementedLane,
            ),
        ];
        for (value, provider, expected) in cases {
            assert_eq!(
                resolve_lane(&config(value)),
                (provider.to_owned(), expected)
            );
        }
    }
}
