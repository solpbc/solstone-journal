// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use super::{Api, Error};
use serde_json::Value;
use std::io::Read;
use std::time::{Duration, Instant};

pub struct LiveApi {
    key: String,
    root: std::path::PathBuf,
    agent: ureq::Agent,
    last_request: Option<Instant>,
}
impl LiveApi {
    pub fn new(root: std::path::PathBuf, key: String) -> Self {
        let config = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .max_redirects(0)
            .timeout_global(Some(Duration::from_secs(90)))
            .build();
        Self {
            key,
            root,
            agent: ureq::Agent::new_with_config(config),
            last_request: None,
        }
    }
}
impl Api for LiveApi {
    fn get(&mut self, path: &str, query: &[(&str, String)]) -> Result<(u16, Value), Error> {
        if let Some(last) = self.last_request
            && let Some(wait) = Duration::from_millis(250).checked_sub(last.elapsed())
        {
            std::thread::sleep(wait);
        }
        if super::key(&self.root)?.as_deref() != Some(self.key.as_str()) {
            return Err(Error::Stopped);
        }
        self.last_request = Some(Instant::now());
        // Paths are built from validated note IDs; no response URL is ever followed.
        let url = format!("https://public-api.granola.ai{path}");
        let mut request = self
            .agent
            .get(&url)
            .header("Authorization", format!("Bearer {}", self.key))
            .header("Accept", "application/json");
        for (key, value) in query {
            request = request.query(key, value);
        }
        let response = request.call().map_err(|_| Error::Unavailable)?;
        let status = response.status().as_u16();
        if status != 200 {
            return Ok((status, Value::Null));
        }
        const LIMIT: u64 = 16 * 1024 * 1024;
        let mut bytes = Vec::new();
        response
            .into_body()
            .into_reader()
            .take(LIMIT + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| Error::Unavailable)?;
        if bytes.len() as u64 > LIMIT {
            return Err(Error::Response);
        }
        Ok((
            status,
            serde_json::from_slice(&bytes).map_err(|_| Error::Response)?,
        ))
    }
}
