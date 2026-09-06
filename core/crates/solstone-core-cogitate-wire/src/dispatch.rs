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
    max_output_tokens: u64,
    next_response_id: u64,
}

enum ConverseArm {
    Bundled,
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
            LaneOutcome::BundledLocal => ConverseArm::Bundled,
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
            // Long final tool submissions need a completion budget independent
            // of the number of tool turns. Reserve at most a quarter of a known
            // window by default; an explicit talent budget remains authoritative.
            max_output_tokens: request.max_output_tokens.map(u64::from).unwrap_or_else(|| {
                request
                    .context_window
                    .map_or(8192, |window| (window / 4).clamp(1, 8192))
            }),
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
            max_output_tokens: self.max_output_tokens,
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
            ConverseArm::Bundled => "bundled",
            ConverseArm::Endpoint(_) => "endpoint",
            ConverseArm::Confidential(_) => "confidential",
            ConverseArm::Google => "google",
            ConverseArm::Anthropic => "anthropic",
            ConverseArm::OpenAi => "openai",
        }
    }

    #[cfg(test)]
    pub(crate) fn converse_endpoint_with_transport<T>(
        &mut self,
        model: &str,
        system_instruction: Option<&str>,
        messages: &[ConverseMessage],
        tools: &[ConverseToolSpec],
        deadline: Duration,
        transport: &mut T,
    ) -> Result<ProviderResponse, ConverseFailure>
    where
        T: solstone_core_generate_wire::EndpointTransport,
    {
        let request = self.request(system_instruction, deadline);
        let ConverseArm::Endpoint(endpoint) = &mut self.arm else {
            panic!("endpoint test driver requires the endpoint arm");
        };
        endpoint.served_model_id = model.to_owned();
        let turn =
            solstone_core_generate_wire::endpoint_test_support::endpoint_converse_with_transport(
                &request,
                messages,
                tools,
                &self.journal_root,
                endpoint,
                &self.config,
                &self.endpoint_runtime,
                transport,
                dispatch_monotonic_now(),
            )?;
        Ok(self.response(turn, "endpoint"))
    }

    #[cfg(test)]
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn converse_confidential_with_controls<R, E>(
        &mut self,
        model: &str,
        system_instruction: Option<&str>,
        messages: &[ConverseMessage],
        tools: &[ConverseToolSpec],
        deadline: Duration,
        now: std::time::SystemTime,
        readiness: R,
        establish: E,
    ) -> Result<ProviderResponse, ConverseFailure>
    where
        R: FnOnce(&std::path::Path) -> solstone_core_spp_ratls::NvattestEnsureStatus,
        E: FnOnce(
            &solstone_core_spp_ratls::RatlsEndpoint,
            &std::path::Path,
        ) -> Result<
            (
                solstone_core_spp_ratls::CompositeVerdict,
                Box<dyn solstone_core_spp_ratls::AttestedIo>,
            ),
            &'static str,
        >,
    {
        let request = self.request(system_instruction, deadline);
        let ConverseArm::Confidential(endpoint) = &mut self.arm else {
            panic!("confidential test driver requires the confidential arm");
        };
        endpoint.served_model_id = model.to_owned();
        let turn = solstone_core_generate_wire::test_support::confidential_converse_with_controls(
            &request,
            messages,
            tools,
            &self.journal_root,
            endpoint,
            &self.config,
            &self.endpoint_runtime,
            now,
            readiness,
            establish,
        )?;
        Ok(self.response(turn, "confidential"))
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
            ConverseArm::Bundled => (
                solstone_core_generate_wire::bundled_converse(
                    &request,
                    messages,
                    tools,
                    &self.journal_root,
                    &self.config,
                    &self.endpoint_runtime,
                )?,
                "bundled",
            ),
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

#[allow(dead_code)]
fn dispatch_monotonic_now() -> std::time::Instant {
    std::time::Instant::now()
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
