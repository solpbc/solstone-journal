// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use solstone_core_local::ByoEndpoint;

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

impl EndpointOverrides {
    /// Apply the dedicated cogitate overrides only after a local endpoint lane
    /// has been selected from journal configuration.
    pub fn apply_to(&self, endpoint: &mut ByoEndpoint) {
        if let Some(endpoint_url) = self.endpoint_url() {
            endpoint.base_url = normalize_endpoint_url(endpoint_url);
        }
        if let Some(api_key) = self.api_key() {
            endpoint.credential = Some(api_key.to_owned());
        }
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
