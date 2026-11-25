// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Fixed-origin HTTP transport for Garmin Connect Personal API.

use std::io::Read;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use super::{API_ORIGIN, Error, GarminHttp, MIN_PACE_MS, RawResponse, TOKEN_PATH};

const USER_AGENT: &str = concat!("solstone-journal/", env!("CARGO_PKG_VERSION"));
const MAX_RESPONSE_BYTES: u64 = 16 * 1024 * 1024;
const MAX_RETRY_AFTER_SECS: u64 = 60;

/// Live direct-to-Garmin HTTP transport. Never follows redirects, never follows
/// response URLs, paces requests to stay within Garmin's per-key rate budget,
/// and stops immediately if the saved key is removed or replaced.
pub struct LiveGarminHttp {
    origin: String,
    /// When set, every request re-checks that this key is still the saved one.
    root: Option<(PathBuf, String)>,
    agent: ureq::Agent,
    last_request: Option<Instant>,
    pace: Duration,
}

impl LiveGarminHttp {
    /// Transport for an active sync tied to a journal's saved key.
    #[must_use]
    pub fn new(root: PathBuf, expected_key: String) -> Self {
        Self::build(
            API_ORIGIN.to_owned(),
            Some((root, expected_key)),
            Duration::from_secs(90),
            Duration::from_millis(MIN_PACE_MS),
        )
    }

    /// Short-timeout transport for a one-shot key probe during Connect / Replace.
    #[must_use]
    pub fn probe() -> Self {
        Self::build(
            API_ORIGIN.to_owned(),
            None,
            Duration::from_secs(20),
            Duration::ZERO,
        )
    }

    /// Loopback-only transport for mock-server integration tests.
    ///
    /// # Panics
    /// Panics if `origin` is not a local loopback HTTP origin (`127.0.0.1` or `[::1]`).
    #[must_use]
    pub fn with_loopback_origin_for_tests(
        origin: String,
        root: Option<(PathBuf, String)>,
        pace: Duration,
    ) -> Self {
        assert!(
            origin.starts_with("http://127.0.0.1:") || origin.starts_with("http://[::1]:"),
            "test origin must be loopback HTTP: {origin}"
        );
        Self::build(origin, root, Duration::from_secs(15), pace)
    }

    fn build(
        origin: String,
        root: Option<(PathBuf, String)>,
        timeout: Duration,
        pace: Duration,
    ) -> Self {
        let config = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .max_redirects(0)
            .timeout_global(Some(timeout))
            .build();
        Self {
            origin,
            root,
            agent: ureq::Agent::new_with_config(config),
            last_request: None,
            pace,
        }
    }

    fn before_request(&mut self) -> Result<(), Error> {
        if let Some(last) = self.last_request
            && let Some(wait) = self.pace.checked_sub(last.elapsed())
        {
            std::thread::sleep(wait);
        }
        if !crate::work_continues() {
            return Err(Error::Stopped);
        }
        if let Some((root, expected_key)) = &self.root
            && super::key(root)?.as_deref() != Some(expected_key.as_str())
        {
            return Err(Error::Stopped);
        }
        self.last_request = Some(Instant::now());
        Ok(())
    }

    fn read_response(response: ureq::http::Response<ureq::Body>) -> Result<RawResponse, Error> {
        let status = response.status().as_u16();
        let retry_after_secs = response
            .headers()
            .get("Retry-After")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.trim().parse::<u64>().ok())
            .map(|secs| secs.min(MAX_RETRY_AFTER_SECS));
        let mut bytes = Vec::new();
        response
            .into_body()
            .into_reader()
            .take(MAX_RESPONSE_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| Error::Unavailable)?;
        if bytes.len() as u64 > MAX_RESPONSE_BYTES {
            return Err(Error::Response);
        }
        Ok(RawResponse {
            status,
            body: bytes,
            retry_after_secs,
        })
    }
}

impl GarminHttp for LiveGarminHttp {
    fn exchange_token(&mut self, api_key: &str) -> Result<RawResponse, Error> {
        self.before_request()?;
        let url = format!("{}{TOKEN_PATH}", self.origin);
        // Garmin's token endpoint returns `200 text/plain` and rejects `Accept: application/json` with 406.
        let response = self
            .agent
            .post(&url)
            .header("Authorization", format!("ApiKey {api_key}"))
            .header("Accept", "*/*")
            .header("User-Agent", USER_AGENT)
            .send_empty()
            .map_err(|_| Error::Unavailable)?;
        Self::read_response(response)
    }

    fn get_json(
        &mut self,
        bearer: &str,
        path: &str,
        query: &[(&str, String)],
    ) -> Result<RawResponse, Error> {
        if !path.starts_with('/') || path.contains("://") || path.contains("..") {
            return Err(Error::Response);
        }
        self.before_request()?;
        let url = format!("{}{path}", self.origin);
        let mut request = self
            .agent
            .get(&url)
            .header("Authorization", format!("Bearer {bearer}"))
            .header("Accept", "application/json")
            .header("User-Agent", USER_AGENT);
        for (key, value) in query {
            request = request.query(key, value);
        }
        let response = request.call().map_err(|_| Error::Unavailable)?;
        Self::read_response(response)
    }
}
