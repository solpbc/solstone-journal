// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::{Map, Value};
use solstone_core_local::{
    ByoEndpoint, GenerateFailure, LocalEndpointResolution, resolve_local_endpoint,
};

use crate::ValidationFailure;
use crate::anthropic::AnthropicFailure;
use crate::endpoint::EndpointFailure;
use crate::google::GoogleFailure;
use crate::openai::OpenAiFailure;

#[derive(Debug, Clone, PartialEq)]
pub enum LaneOutcome {
    NoEngine,
    BundledLocal,
    AttestationNotVerified,
    AttestationFailed,
    AttestationStale,
    ByoEndpoint(ByoEndpoint),
    ConfidentialEndpoint(ByoEndpoint),
    UnimplementedLane,
    BundledFailure(Box<GenerateFailure>),
    EndpointFailure(EndpointFailure),
    Anthropic,
    AnthropicFailure(AnthropicFailure),
    OpenAi,
    OpenAiFailure(OpenAiFailure),
    Google,
    GoogleFailure(GoogleFailure),
    ValidationFailure(ValidationFailure),
}

pub fn resolve_lane(config: &Map<String, Value>) -> (String, LaneOutcome) {
    let provider = string_at(config, &["providers", "active", "provider"])
        .filter(|provider| !provider.is_empty())
        .unwrap_or("none")
        .to_owned();
    if provider == "none" {
        return (provider, LaneOutcome::NoEngine);
    }
    if provider == "anthropic" {
        return (provider, LaneOutcome::Anthropic);
    }
    if provider == "openai" {
        return (provider, LaneOutcome::OpenAi);
    }
    if provider == "google" {
        return (provider, LaneOutcome::Google);
    }
    if provider != "local" {
        return (provider, LaneOutcome::UnimplementedLane);
    }

    (
        provider,
        match resolve_local_endpoint(config) {
            LocalEndpointResolution::Bundled => LaneOutcome::BundledLocal,
            LocalEndpointResolution::Byo(endpoint) if endpoint.is_confidential => {
                LaneOutcome::ConfidentialEndpoint(endpoint)
            }
            LocalEndpointResolution::Byo(endpoint) => LaneOutcome::ByoEndpoint(endpoint),
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
                LaneOutcome::ConfidentialEndpoint(ByoEndpoint {
                    base_url: "https://endpoint".into(),
                    served_model_id: "served".into(),
                    credential: None,
                    parallel_slots: None,
                    is_confidential: true,
                }),
            ),
            (
                json!({"providers": {"active": {"provider": "local"}, "local": {"endpoint_url": "https://endpoint", "served_model_id": "served"}}}),
                "local",
                LaneOutcome::ByoEndpoint(ByoEndpoint {
                    base_url: "https://endpoint".into(),
                    served_model_id: "served".into(),
                    credential: None,
                    parallel_slots: Some(2),
                    is_confidential: false,
                }),
            ),
            (
                json!({"providers": {"active": {"provider": "openai"}}}),
                "openai",
                LaneOutcome::OpenAi,
            ),
            (
                json!({"providers": {"active": {"provider": "anthropic"}}}),
                "anthropic",
                LaneOutcome::Anthropic,
            ),
            (
                json!({"providers": {"active": {"provider": "google"}}}),
                "google",
                LaneOutcome::Google,
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
