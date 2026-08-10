// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::PathBuf;
use std::time::Duration;

use serde_json::{Map, Value, json};
use solstone_core_cogitate_runtime::{ConverseProvider, ProviderResponse};
use solstone_core_generate::GenerateRequest;
use solstone_core_generate_wire::{
    ConverseFailure, ConverseMessage, ConverseToolSpec, EndpointRuntime, endpoint_converse,
};
use solstone_core_local::ByoEndpoint;
use thiserror::Error;

use crate::CogitateRequest;

pub const COGITATE_ENDPOINT_URL_OVERRIDE_ENV: &str = "SOLSTONE_COGITATE_ENDPOINT_URL_OVERRIDE";
pub const COGITATE_API_KEY_OVERRIDE_ENV: &str = "SOLSTONE_COGITATE_API_KEY_OVERRIDE";

/// Dedicated child-process endpoint overrides, read once while constructing
/// the cogitate provider. Credentials never enter the request record.
pub struct EndpointOverrides {
    endpoint_url: Option<String>,
    api_key: Option<String>,
}

impl EndpointOverrides {
    pub fn from_process() -> Self {
        Self::from_values(
            non_blank_process_env(COGITATE_ENDPOINT_URL_OVERRIDE_ENV),
            non_blank_process_env(COGITATE_API_KEY_OVERRIDE_ENV),
        )
    }

    pub fn from_values(endpoint_url: Option<String>, api_key: Option<String>) -> Self {
        Self {
            endpoint_url: endpoint_url.filter(|value| !value.trim().is_empty()),
            api_key: api_key.filter(|value| !value.trim().is_empty()),
        }
    }

    pub fn endpoint_url(&self) -> Option<&str> {
        self.endpoint_url.as_deref()
    }

    pub fn api_key(&self) -> Option<&str> {
        self.api_key.as_deref()
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum EndpointConfigurationError {
    #[error(
        "missing {COGITATE_ENDPOINT_URL_OVERRIDE_ENV}; native cogitate requires a dedicated endpoint override"
    )]
    MissingEndpointUrl,
}

/// A single BYO-endpoint provider lane for native cogitate.
///
/// It wraps generate-wire's tested HTTP implementation rather than owning a
/// second transport. It intentionally does not derive `Debug`, because its
/// endpoint contains a bearer credential.
pub struct EndpointConverseProvider {
    endpoint: ByoEndpoint,
    endpoint_config: Map<String, Value>,
    endpoint_runtime: EndpointRuntime,
    journal_root: PathBuf,
    request_id: String,
    next_response_id: u64,
}

impl EndpointConverseProvider {
    pub fn from_request(request: &CogitateRequest) -> Result<Self, EndpointConfigurationError> {
        Self::new(request, EndpointOverrides::from_process())
    }

    pub fn new(
        request: &CogitateRequest,
        overrides: EndpointOverrides,
    ) -> Result<Self, EndpointConfigurationError> {
        let endpoint_url = overrides
            .endpoint_url
            .ok_or(EndpointConfigurationError::MissingEndpointUrl)?;
        let endpoint_config = request.context_window.map_or_else(Map::new, |window| {
            json!({"providers": {"local": {"served_context_window": window}}})
                .as_object()
                .expect("literal configuration is an object")
                .clone()
        });
        Ok(Self {
            endpoint: ByoEndpoint {
                base_url: normalize_endpoint_url(&endpoint_url),
                served_model_id: request.model.clone(),
                credential: overrides.api_key,
                parallel_slots: Some(2),
                is_confidential: false,
            },
            endpoint_config,
            endpoint_runtime: EndpointRuntime::default(),
            journal_root: request.journal_root.clone(),
            request_id: request.correlation_id.clone(),
            next_response_id: 0,
        })
    }
}

impl ConverseProvider for EndpointConverseProvider {
    fn converse(
        &mut self,
        model: &str,
        system_instruction: Option<&str>,
        messages: &[ConverseMessage],
        tools: &[ConverseToolSpec],
        deadline: Duration,
    ) -> Result<ProviderResponse, ConverseFailure> {
        self.endpoint.served_model_id = model.to_owned();
        let request = GenerateRequest {
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
        };
        let turn = endpoint_converse(
            &request,
            messages,
            tools,
            &self.journal_root,
            &self.endpoint,
            &self.endpoint_config,
            &self.endpoint_runtime,
        )?;
        self.next_response_id = self.next_response_id.saturating_add(1);
        Ok(ProviderResponse {
            turn: *turn,
            response_id: format!("endpoint-{}", self.next_response_id),
        })
    }
}

fn non_blank_process_env(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

fn normalize_endpoint_url(value: &str) -> String {
    let trimmed = value.trim_end_matches('/');
    trimmed
        .strip_suffix("/v1")
        .unwrap_or(trimmed)
        .trim_end_matches('/')
        .to_owned()
}
