// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! HTTP client and transport abstraction for ChatGPT auth and API requests.

use std::collections::BTreeMap;
use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpResponse {
    pub status: u16,
    pub body: String,
    pub headers: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransportError {
    Network,
    Timeout,
}

pub trait ChatGptTransport: Send + Sync {
    fn post_form(
        &self,
        url: &str,
        form: &BTreeMap<String, String>,
        timeout: Duration,
    ) -> Result<HttpResponse, TransportError>;

    fn get_json(
        &self,
        url: &str,
        bearer_token: &str,
        timeout: Duration,
    ) -> Result<HttpResponse, TransportError>;
}

pub struct UreqTransport;

impl ChatGptTransport for UreqTransport {
    fn post_form(
        &self,
        url: &str,
        form: &BTreeMap<String, String>,
        timeout: Duration,
    ) -> Result<HttpResponse, TransportError> {
        let config = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_connect(Some(timeout))
            .timeout_recv_response(Some(timeout))
            .timeout_recv_body(Some(timeout))
            .timeout_global(Some(timeout))
            .build();
        let agent = ureq::Agent::new_with_config(config);
        let form_body = form
            .iter()
            .map(|(key, val)| {
                format!(
                    "{}={}",
                    solstone_core_auth_flow::percent_encode(key),
                    solstone_core_auth_flow::percent_encode(val)
                )
            })
            .collect::<Vec<_>>()
            .join("&");

        let response = agent
            .post(url)
            .header("Content-Type", "application/x-www-form-urlencoded")
            .send(form_body)
            .map_err(|_| TransportError::Network)?;

        let status = response.status().as_u16();
        let mut headers = BTreeMap::new();
        for (name, val) in response.headers() {
            if let Ok(val_str) = val.to_str() {
                headers.insert(name.as_str().to_ascii_lowercase(), val_str.to_owned());
            }
        }
        let body = response
            .into_body()
            .read_to_string()
            .map_err(|_| TransportError::Network)?;
        Ok(HttpResponse {
            status,
            body,
            headers,
        })
    }

    fn get_json(
        &self,
        url: &str,
        bearer_token: &str,
        timeout: Duration,
    ) -> Result<HttpResponse, TransportError> {
        let config = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_connect(Some(timeout))
            .timeout_recv_response(Some(timeout))
            .timeout_recv_body(Some(timeout))
            .timeout_global(Some(timeout))
            .build();
        let agent = ureq::Agent::new_with_config(config);
        let response = agent
            .get(url)
            .header("Authorization", &format!("Bearer {bearer_token}"))
            .header("Accept", "application/json")
            .call()
            .map_err(|_| TransportError::Network)?;

        let status = response.status().as_u16();
        let mut headers = BTreeMap::new();
        for (name, val) in response.headers() {
            if let Ok(val_str) = val.to_str() {
                headers.insert(name.as_str().to_ascii_lowercase(), val_str.to_owned());
            }
        }
        let body = response
            .into_body()
            .read_to_string()
            .map_err(|_| TransportError::Network)?;
        Ok(HttpResponse {
            status,
            body,
            headers,
        })
    }
}
