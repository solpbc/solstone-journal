// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The journal's own loopback routes, the same ones its browser pages and the
//! Mac app call: the journal's name, its identity and mark, the first-run mark
//! ritual, the device list and the running version. Loopback callers need no
//! credential; nothing here leaves this PC.

use std::time::Duration;

use serde_json::Value;
use ureq::Agent;

pub const DEFAULT_PORT: u16 = 5015;

pub struct Convey {
    base: String,
    agent: Agent,
}

/// What `GET /init` says about the first run.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InitProbe {
    Complete,
    Incomplete,
}

impl Convey {
    pub fn new(port: Option<u16>) -> Self {
        let agent: Agent = Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(5)))
            .http_status_as_error(false)
            .max_redirects(0)
            .max_redirects_will_error(false)
            .build()
            .into();
        Self {
            base: format!("http://127.0.0.1:{}", port.unwrap_or(DEFAULT_PORT)),
            agent,
        }
    }

    pub fn base(&self) -> &str {
        &self.base
    }

    fn json(
        &self,
        response: Result<ureq::http::Response<ureq::Body>, ureq::Error>,
    ) -> Result<Value, String> {
        let mut response = response.map_err(|error| error.to_string())?;
        let status = response.status().as_u16();
        let text = response
            .body_mut()
            .read_to_string()
            .map_err(|error| error.to_string())?;
        let value: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
        if (200..300).contains(&status) {
            return Ok(value);
        }
        // The journal names a refusal for the owner in `detail` or `error`.
        let reason = ["detail", "error", "message"]
            .iter()
            .find_map(|key| value.get(key).and_then(Value::as_str))
            .map_or_else(|| format!("your journal answered {status}"), str::to_owned);
        Err(reason)
    }

    pub fn get(&self, path: &str) -> Result<Value, String> {
        self.json(
            self.agent
                .get(format!("{}{path}", self.base))
                .header("Accept", "application/json")
                .call(),
        )
    }

    pub fn post(&self, path: &str, body: Option<&Value>) -> Result<Value, String> {
        let request = self
            .agent
            .post(format!("{}{path}", self.base))
            .header("Accept", "application/json");
        self.json(match body {
            Some(body) => request
                .header("Content-Type", "application/json")
                .send(body.to_string()),
            None => request.send_empty(),
        })
    }

    pub fn put(&self, path: &str, body: &Value) -> Result<Value, String> {
        self.json(
            self.agent
                .put(format!("{}{path}", self.base))
                .header("Accept", "application/json")
                .header("Content-Type", "application/json")
                .send(body.to_string()),
        )
    }

    /// Whether the journal answers, and its version when it says. A journal
    /// still in its first run answers every page with that first run, so an
    /// answer of any kind counts; only a finished journal states its version.
    pub fn answer(&self) -> (bool, Option<String>) {
        let Ok(mut response) = self
            .agent
            .get(format!("{}/api/system/status", self.base))
            .header("Accept", "application/json")
            .call()
        else {
            return (false, None);
        };
        let version = response
            .body_mut()
            .read_to_string()
            .ok()
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
            .and_then(|status| {
                status
                    .pointer("/version/current")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            });
        (true, version)
    }

    /// `/init` redirects once the first run is finished, as the Mac app reads it.
    pub fn init_probe(&self) -> Result<InitProbe, String> {
        let response = self
            .agent
            .get(format!("{}/init", self.base))
            .header("Accept", "text/html")
            .call()
            .map_err(|error| error.to_string())?;
        match response.status().as_u16() {
            300..=399 => Ok(InitProbe::Complete),
            200 => Ok(InitProbe::Incomplete),
            status => Err(format!("your journal answered {status}")),
        }
    }
}
