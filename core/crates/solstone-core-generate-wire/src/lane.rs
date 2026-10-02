// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::{Map, Value};
use solstone_core_local::{
    ByoEndpoint, GenerateFailure, LocalEndpointResolution, resolve_local_endpoint,
};

use crate::ValidationFailure;
use crate::anthropic::AnthropicFailure;
use crate::chatgpt::ChatGptFailure;
use crate::endpoint::EndpointFailure;
use crate::google::GoogleFailure;
use crate::openai::OpenAiFailure;
use crate::overrides::{configured_provider_with, non_blank_process_env};

#[derive(Debug, Clone, PartialEq)]
pub enum LaneOutcome {
    NoEngine,
    BundledLocal,
    AttestationNotVerified,
    AttestationFailed(&'static str),
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
    ChatGpt,
    ChatGptFailure(ChatGptFailure),
    ValidationFailure(ValidationFailure),
}

pub fn resolve_lane(config: &Map<String, Value>) -> (String, LaneOutcome) {
    resolve_lane_with(config, non_blank_process_env)
}

pub(crate) fn resolve_lane_with(
    config: &Map<String, Value>,
    env: impl Fn(&str) -> Option<String>,
) -> (String, LaneOutcome) {
    let provider = configured_provider_with(config, env);
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
    if provider == "chatgpt" {
        return (provider, LaneOutcome::ChatGpt);
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

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::overrides::PROVIDER_OVERRIDE_ENV;

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
                    is_bundled: false,
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
                    is_bundled: false,
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
            (
                json!({"providers": {"active": {"provider": "chatgpt"}}}),
                "chatgpt",
                LaneOutcome::ChatGpt,
            ),
        ];
        for (value, provider, expected) in cases {
            assert_eq!(
                resolve_lane(&config(value)),
                (provider.to_owned(), expected)
            );
        }
    }

    #[test]
    fn resolve_lane_with_respects_exact_casing_and_unimplemented_lanes() {
        let empty_env = |_name: &str| -> Option<String> { None };

        assert_eq!(
            resolve_lane_with(
                &config(json!({"providers": {"active": {"provider": "OpenAI"}}})),
                empty_env,
            ),
            ("OpenAI".to_owned(), LaneOutcome::UnimplementedLane)
        );

        assert_eq!(
            resolve_lane_with(
                &config(json!({"providers": {"active": {"provider": "some-unknown"}}})),
                empty_env,
            ),
            ("some-unknown".to_owned(), LaneOutcome::UnimplementedLane)
        );

        assert_eq!(
            resolve_lane_with(
                &config(json!({
                    "env": {"OPENAI_API_KEY": "sk-test"},
                    "providers": {"active": {"provider": "some-unknown"}},
                    "services": {
                        "confidential": {
                            "prior_active": {"provider": "openai", "model": "gpt-5"}
                        }
                    }
                })),
                empty_env,
            ),
            ("some-unknown".to_owned(), LaneOutcome::UnimplementedLane)
        );

        let confidential_cfg = config(json!({
            "env": {"OPENAI_API_KEY": "sk-test"},
            "providers": {
                "active": {"provider": "local"},
                "local": {
                    "endpoint_url": "https://service.example/v1",
                    "served_model_id": "served"
                }
            },
            "services": {
                "confidential": {
                    "prior_active": {"provider": "openai", "model": "gpt-5"}
                }
            }
        }));

        assert_eq!(
            resolve_lane_with(&confidential_cfg, empty_env),
            (
                "local".to_owned(),
                LaneOutcome::ConfidentialEndpoint(ByoEndpoint {
                    base_url: "https://service.example".into(),
                    served_model_id: "served".into(),
                    credential: None,
                    parallel_slots: None,
                    is_confidential: true,
                    is_bundled: false,
                })
            )
        );

        let override_env = |name: &str| -> Option<String> {
            if name == PROVIDER_OVERRIDE_ENV {
                Some("openai".to_owned())
            } else {
                None
            }
        };

        assert_eq!(
            resolve_lane_with(&confidential_cfg, override_env),
            ("openai".to_owned(), LaneOutcome::OpenAi)
        );
    }
}
