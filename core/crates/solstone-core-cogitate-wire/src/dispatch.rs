// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::PathBuf;
use std::time::Duration;

use serde_json::{Map, Value};
use solstone_core_cogitate_runtime::{ConverseProvider, ProviderResponse};
use solstone_core_generate::GenerateRequest;
use solstone_core_generate_wire::{
    ConverseFailure, ConverseMessage, ConverseToolSpec, EndpointRuntime, LaneOutcome,
    anthropic_converse, confidential_converse, endpoint_converse, google_converse, openai_converse,
};
use solstone_core_local::ByoEndpoint;

use crate::{CogitateRequest, EndpointOverrides};

/// The provider selected once from the journal configuration for one cogitate run.
pub struct DispatchConverseProvider {
    arm: ConverseArm,
    config: Map<String, Value>,
    endpoint_runtime: EndpointRuntime,
    journal_root: PathBuf,
    request_id: String,
    next_response_id: u64,
}

enum ConverseArm {
    Endpoint(ByoEndpoint),
    Confidential(ByoEndpoint),
    Google,
    Anthropic,
    OpenAi,
}

impl DispatchConverseProvider {
    pub fn from_lane(
        request: &CogitateRequest,
        mut config: Map<String, Value>,
        lane: LaneOutcome,
        overrides: EndpointOverrides,
    ) -> Option<Self> {
        set_local_context_window(&mut config, request.context_window);
        let arm = match lane {
            LaneOutcome::ByoEndpoint(mut endpoint) => {
                overrides.apply_to(&mut endpoint);
                ConverseArm::Endpoint(endpoint)
            }
            LaneOutcome::ConfidentialEndpoint(mut endpoint) => {
                overrides.apply_to(&mut endpoint);
                ConverseArm::Confidential(endpoint)
            }
            LaneOutcome::Google => ConverseArm::Google,
            LaneOutcome::Anthropic => ConverseArm::Anthropic,
            LaneOutcome::OpenAi => ConverseArm::OpenAi,
            _ => return None,
        };
        Some(Self {
            arm,
            config,
            endpoint_runtime: EndpointRuntime::default(),
            journal_root: request.journal_root.clone(),
            request_id: request.correlation_id.clone(),
            next_response_id: 0,
        })
    }

    fn request(&self, system_instruction: Option<&str>, deadline: Duration) -> GenerateRequest {
        GenerateRequest {
            id: Some(self.request_id.clone()),
            context: "solstone-cogitate".to_owned(),
            contents: Vec::new(),
            system_instruction: system_instruction.map(ToOwned::to_owned),
            temperature: 0.2,
            max_output_tokens: 1024,
            thinking_budget: None,
            timeout_s: Some(deadline.as_secs_f64()),
            json_output: false,
            json_schema: None,
            enforce_responsiveness: true,
            attempt_index: 0,
            exclusive_admission: false,
            transport_retries: None,
        }
    }

    fn response(
        &mut self,
        turn: Box<solstone_core_generate_wire::ConverseTurn>,
        arm: &str,
    ) -> ProviderResponse {
        self.next_response_id = self.next_response_id.saturating_add(1);
        ProviderResponse {
            turn: *turn,
            response_id: format!("{arm}-{}", self.next_response_id),
        }
    }

    #[cfg(test)]
    pub(crate) const fn arm_name(&self) -> &'static str {
        match self.arm {
            ConverseArm::Endpoint(_) => "endpoint",
            ConverseArm::Confidential(_) => "confidential",
            ConverseArm::Google => "google",
            ConverseArm::Anthropic => "anthropic",
            ConverseArm::OpenAi => "openai",
        }
    }
}

impl ConverseProvider for DispatchConverseProvider {
    fn converse(
        &mut self,
        model: &str,
        system_instruction: Option<&str>,
        messages: &[ConverseMessage],
        tools: &[ConverseToolSpec],
        deadline: Duration,
    ) -> Result<ProviderResponse, ConverseFailure> {
        let request = self.request(system_instruction, deadline);
        let (turn, arm) = match &mut self.arm {
            ConverseArm::Endpoint(endpoint) => {
                endpoint.served_model_id = model.to_owned();
                (
                    endpoint_converse(
                        &request,
                        messages,
                        tools,
                        &self.journal_root,
                        endpoint,
                        &self.config,
                        &self.endpoint_runtime,
                    )?,
                    "endpoint",
                )
            }
            ConverseArm::Confidential(endpoint) => {
                endpoint.served_model_id = model.to_owned();
                (
                    confidential_converse(
                        &request,
                        messages,
                        tools,
                        &self.journal_root,
                        endpoint,
                        &self.config,
                        &self.endpoint_runtime,
                    )?,
                    "confidential",
                )
            }
            ConverseArm::Google => {
                let mut config = self.config.clone();
                set_active_model(&mut config, model);
                match google_converse(&request, messages, tools, &config) {
                    solstone_core_generate_wire::GoogleConverseResult::Turn(turn) => {
                        (turn, "google")
                    }
                    solstone_core_generate_wire::GoogleConverseResult::Failed(failure) => {
                        return Err(failure);
                    }
                }
            }
            ConverseArm::Anthropic => {
                let mut config = self.config.clone();
                set_active_model(&mut config, model);
                match anthropic_converse(&request, messages, tools, &config) {
                    solstone_core_generate_wire::AnthropicConverseResult::Turn(turn) => {
                        (turn, "anthropic")
                    }
                    solstone_core_generate_wire::AnthropicConverseResult::Failed(failure) => {
                        return Err(failure);
                    }
                }
            }
            ConverseArm::OpenAi => {
                let mut config = self.config.clone();
                set_active_model(&mut config, model);
                match openai_converse(&request, messages, tools, &config) {
                    solstone_core_generate_wire::OpenAiConverseResult::Turn(turn) => {
                        (turn, "openai")
                    }
                    solstone_core_generate_wire::OpenAiConverseResult::Failed(failure) => {
                        return Err(failure);
                    }
                }
            }
        };
        Ok(self.response(turn, arm))
    }
}

fn set_active_model(config: &mut Map<String, Value>, model: &str) {
    let providers = object_at(config, "providers");
    object_at(providers, "active").insert("model".to_owned(), Value::String(model.to_owned()));
}

fn set_local_context_window(config: &mut Map<String, Value>, context_window: Option<u64>) {
    let Some(context_window) = context_window else {
        return;
    };
    let providers = object_at(config, "providers");
    object_at(providers, "local").insert("served_context_window".to_owned(), context_window.into());
}

fn object_at<'a>(object: &'a mut Map<String, Value>, key: &str) -> &'a mut Map<String, Value> {
    if !object.get(key).is_some_and(Value::is_object) {
        object.insert(key.to_owned(), Value::Object(Map::new()));
    }
    object
        .get_mut(key)
        .and_then(Value::as_object_mut)
        .expect("object inserted above")
}
