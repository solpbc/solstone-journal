// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde_json::Value;
use sha2::{Digest, Sha256};
use solstone_core_sol_link::pairing::RelayAccessSnapshot;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

pub const DEFAULT_RELAY_ISSUER: &str = "link.solstone.app";
pub const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_RESPONSE_BYTES: u64 = 65536;

#[derive(Debug, Clone, thiserror::Error)]
pub enum RelayAccessError {
    #[error("not configured")]
    NotConfigured,
    #[error("relay access unavailable: {0}")]
    Unavailable(String),
}

pub(crate) fn build_isolated_ureq_agent(timeout: Duration) -> ureq::Agent {
    let config = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .max_redirects(0)
        .proxy(None)
        .timeout_global(Some(timeout))
        .build();
    ureq::Agent::new_with_config(config)
}

fn decode_base64url(input: &str) -> Option<Vec<u8>> {
    let mut clean = input.trim_end_matches('=').to_string();
    while !clean.len().is_multiple_of(4) {
        clean.push('=');
    }
    let mut out = Vec::with_capacity(clean.len() * 3 / 4);
    let bytes = clean.as_bytes();
    for chunk in bytes.chunks_exact(4) {
        let mut b = [0u32; 4];
        for i in 0..4 {
            b[i] = match chunk[i] {
                b'A'..=b'Z' => chunk[i] - b'A',
                b'a'..=b'z' => chunk[i] - b'a' + 26,
                b'0'..=b'9' => chunk[i] - b'0' + 52,
                b'-' | b'+' => 62,
                b'_' | b'/' => 63,
                b'=' => 0,
                _ => return None,
            } as u32;
        }
        let triple = (b[0] << 18) | (b[1] << 12) | (b[2] << 6) | b[3];
        out.push(((triple >> 16) & 0xFF) as u8);
        if chunk[2] != b'=' {
            out.push(((triple >> 8) & 0xFF) as u8);
        }
        if chunk[3] != b'=' {
            out.push((triple & 0xFF) as u8);
        }
    }
    Some(out)
}

pub fn validate_device_token(
    token: &str,
    current_instance_id: &str,
    expected_issuer: &str,
    body_expires_at: &str,
    now: i64,
) -> Result<i64, RelayAccessError> {
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 3 || parts.iter().any(|part| part.is_empty()) {
        return Err(RelayAccessError::Unavailable(
            "malformed token: expected 3 nonempty parts".into(),
        ));
    }
    let payload_bytes = decode_base64url(parts[1]).ok_or_else(|| {
        RelayAccessError::Unavailable("malformed token: invalid base64url payload".into())
    })?;
    let payload_val: Value = serde_json::from_slice(&payload_bytes)
        .map_err(|e| RelayAccessError::Unavailable(format!("malformed token payload: {e}")))?;
    let obj = payload_val
        .as_object()
        .ok_or_else(|| RelayAccessError::Unavailable("token payload is not an object".into()))?;

    // Exact keys: {iss, sub, aud, scope, ver, instance_id, iat, exp, jti}
    let mut keys: Vec<&str> = obj.keys().map(String::as_str).collect();
    keys.sort_unstable();
    let expected_keys = [
        "aud",
        "exp",
        "iat",
        "instance_id",
        "iss",
        "jti",
        "scope",
        "sub",
        "ver",
    ];
    if keys != expected_keys {
        return Err(RelayAccessError::Unavailable(
            "token payload keys do not match exact schema".into(),
        ));
    }

    // ver == 2
    match obj.get("ver").and_then(Value::as_i64) {
        Some(2) => {}
        _ => {
            return Err(RelayAccessError::Unavailable(
                "invalid ver: expected 2".into(),
            ));
        }
    }

    // aud == "spl-relay"
    match obj.get("aud").and_then(Value::as_str) {
        Some("spl-relay") => {}
        _ => {
            return Err(RelayAccessError::Unavailable(
                "invalid aud: expected spl-relay".into(),
            ));
        }
    }

    // scope == "session.dial"
    match obj.get("scope").and_then(Value::as_str) {
        Some("session.dial") => {}
        _ => {
            return Err(RelayAccessError::Unavailable(
                "invalid scope: expected session.dial".into(),
            ));
        }
    }

    // instance_id == current_instance_id
    match obj.get("instance_id").and_then(Value::as_str) {
        Some(id) if id == current_instance_id => {}
        _ => {
            return Err(RelayAccessError::Unavailable(
                "token instance_id mismatch".into(),
            ));
        }
    }

    // sub == format!("instance:{current_instance_id}")
    let expected_sub = format!("instance:{current_instance_id}");
    match obj.get("sub").and_then(Value::as_str) {
        Some(sub) if sub == expected_sub => {}
        _ => return Err(RelayAccessError::Unavailable("token sub mismatch".into())),
    }

    // HTTPS provenance authenticates the response. An explicitly configured
    // issuer may narrow acceptance; an empty expectation requires only nonempty
    // iss, because a self-hosted issuer need not equal its relay hostname.
    match obj.get("iss").and_then(Value::as_str) {
        Some(iss) if !iss.is_empty() && (expected_issuer.is_empty() || iss == expected_issuer) => {}
        _ => return Err(RelayAccessError::Unavailable("token iss mismatch".into())),
    }

    // jti nonempty
    match obj.get("jti").and_then(Value::as_str) {
        Some(jti) if !jti.is_empty() => {}
        _ => {
            return Err(RelayAccessError::Unavailable(
                "token jti missing or empty".into(),
            ));
        }
    }

    // iat integer
    let iat = obj.get("iat").and_then(Value::as_i64).ok_or_else(|| {
        RelayAccessError::Unavailable("token iat missing or invalid integer".into())
    })?;

    // exp integer
    let exp = obj.get("exp").and_then(Value::as_i64).ok_or_else(|| {
        RelayAccessError::Unavailable("token exp missing or invalid integer".into())
    })?;

    if exp <= iat {
        return Err(RelayAccessError::Unavailable("token exp <= iat".into()));
    }

    if iat > now.saturating_add(60) {
        return Err(RelayAccessError::Unavailable(
            "token issued in the future".into(),
        ));
    }

    if exp <= now {
        return Err(RelayAccessError::Unavailable("token expired".into()));
    }

    // Check body_expires_at parses as RFC3339 and unix timestamp matches exp
    let parsed_dt = OffsetDateTime::parse(body_expires_at, &Rfc3339)
        .map_err(|e| RelayAccessError::Unavailable(format!("invalid expires_at format: {e}")))?;
    if parsed_dt.unix_timestamp() != exp || parsed_dt.nanosecond() != 0 {
        return Err(RelayAccessError::Unavailable(
            "body expires_at does not match token exp claim".into(),
        ));
    }

    Ok(exp)
}

pub fn fetch_relay_access(
    relay_origin: &str,
    service_token: &str,
    current_instance_id: &str,
    expected_issuer: &str,
    now: i64,
    timeout: Duration,
) -> Result<RelayAccessSnapshot, RelayAccessError> {
    let started = Instant::now();
    let agent = build_isolated_ureq_agent(timeout.min(Duration::from_secs(15)));
    // Reuse the maintained origin policy, including local transport fixtures.
    let window_url = crate::pair_window_client::pair_window_registration_url(relay_origin)
        .map_err(|_| RelayAccessError::Unavailable("invalid relay origin".into()))?;
    let base = window_url
        .strip_suffix("/session/pair-window")
        .ok_or_else(|| RelayAccessError::Unavailable("invalid relay origin".into()))?;
    let url = format!(
        "{}/token/access",
        base.replacen("wss://", "https://", 1)
            .replacen("ws://", "http://", 1)
    );

    let request_body = serde_json::to_string(&serde_json::json!({
        "service_token": service_token
    }))
    .map_err(|e| RelayAccessError::Unavailable(e.to_string()))?;

    if request_body.len() > 16384 {
        return Err(RelayAccessError::Unavailable(
            "request body exceeds 16KiB limit".into(),
        ));
    }

    let response = agent
        .post(&url)
        .header("Content-Type", "application/json")
        .header("User-Agent", "")
        .send(request_body);

    let response = match response {
        Ok(res) => res,
        Err(e) => {
            return Err(RelayAccessError::Unavailable(format!(
                "relay request error: {e}"
            )));
        }
    };

    let status = response.status();
    let mut body_bytes = Vec::new();
    let reader = response.into_body().into_reader();
    let mut limited_reader = reader.take(MAX_RESPONSE_BYTES + 1);
    limited_reader
        .read_to_end(&mut body_bytes)
        .map_err(|e| RelayAccessError::Unavailable(format!("relay read error: {e}")))?;

    if body_bytes.len() as u64 > MAX_RESPONSE_BYTES {
        return Err(RelayAccessError::Unavailable(
            "relay response exceeded 64KiB limit".into(),
        ));
    }

    if status.as_u16() != 200 {
        return Err(RelayAccessError::Unavailable(format!(
            "relay returned status {}",
            status.as_u16()
        )));
    }

    let parsed: Value = serde_json::from_slice(&body_bytes)
        .map_err(|e| RelayAccessError::Unavailable(format!("relay response json error: {e}")))?;
    let obj = parsed
        .as_object()
        .ok_or_else(|| RelayAccessError::Unavailable("relay response is not an object".into()))?;

    let protocol_version = obj
        .get("protocol_version")
        .and_then(Value::as_u64)
        .ok_or_else(|| {
            RelayAccessError::Unavailable("missing or invalid protocol_version".into())
        })?;
    if protocol_version != 2 {
        return Err(RelayAccessError::Unavailable(format!(
            "unsupported protocol_version {protocol_version}"
        )));
    }

    let device_token = obj
        .get("device_token")
        .and_then(Value::as_str)
        .ok_or_else(|| RelayAccessError::Unavailable("missing or invalid device_token".into()))?
        .to_string();

    let expires_at = obj
        .get("expires_at")
        .and_then(Value::as_str)
        .ok_or_else(|| RelayAccessError::Unavailable("missing or invalid expires_at".into()))?
        .to_string();

    validate_device_token(
        &device_token,
        current_instance_id,
        expected_issuer,
        &expires_at,
        now.saturating_add(started.elapsed().as_secs() as i64),
    )?;

    Ok(RelayAccessSnapshot {
        protocol_version: 2,
        status: "ready".to_string(),
        relay_origin: relay_origin.to_string(),
        instance_id: current_instance_id.to_string(),
        device_token,
        expires_at,
    })
}

#[derive(Clone, PartialEq, Eq)]
pub struct CacheIdentity {
    pub instance_id: String,
    pub relay_origin: String,
    pub token_fingerprint: [u8; 32],
}

pub enum ResolvedServiceState {
    NotConfigured,
    Invalid(String),
    Configured {
        instance_id: String,
        relay_origin: String,
        service_token: String,
        identity: CacheIdentity,
    },
}

pub fn resolve_journal_relay_state(journal_root: &Path) -> ResolvedServiceState {
    let config_read = match solstone_core_journal_config::read_journal_config(journal_root) {
        Ok(read) => read,
        Err(_) => return ResolvedServiceState::Invalid("invalid journal configuration".into()),
    };
    let config_map = match config_read.config {
        Some(map) => map,
        None => return ResolvedServiceState::NotConfigured,
    };
    let posture = config_map
        .get("link")
        .and_then(Value::as_object)
        .and_then(|link| link.get("posture"))
        .and_then(Value::as_str);

    if posture != Some("spl") {
        return ResolvedServiceState::NotConfigured;
    }

    let relay_origin = crate::private_link::relay_url(journal_root);

    let service_token = match crate::link_state_files::load_link_service_token(journal_root) {
        crate::link_state_files::LinkServiceTokenRead::Present(token) => token.as_str().to_owned(),
        crate::link_state_files::LinkServiceTokenRead::Missing
        | crate::link_state_files::LinkServiceTokenRead::Unreadable
        | crate::link_state_files::LinkServiceTokenRead::Malformed => {
            return ResolvedServiceState::Invalid("missing or invalid service token".to_string());
        }
    };

    let instance_id = match crate::link_state_files::load_link_state(journal_root, "solstone") {
        crate::link_state_files::LinkStateRead::Present(state) if !state.instance_id.is_empty() => {
            state.instance_id
        }
        _ => {
            return ResolvedServiceState::Invalid("missing or invalid instance id".to_string());
        }
    };

    let mut hasher = Sha256::new();
    hasher.update(service_token.as_bytes());
    let token_fingerprint: [u8; 32] = hasher.finalize().into();

    let identity = CacheIdentity {
        instance_id: instance_id.clone(),
        relay_origin: relay_origin.clone(),
        token_fingerprint,
    };

    ResolvedServiceState::Configured {
        instance_id,
        relay_origin,
        service_token,
        identity,
    }
}

// One ordering boundary per journal, shared by service writers, capability
// publication and protected pairing commitment. Network I/O never holds it.
fn configuration_boundary(root: &Path) -> Arc<Mutex<u64>> {
    static BOUNDARIES: OnceLock<Mutex<HashMap<PathBuf, Arc<Mutex<u64>>>>> = OnceLock::new();
    let key = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    BOUNDARIES
        .get_or_init(Default::default)
        .lock()
        .expect("configuration registry")
        .entry(key)
        .or_default()
        .clone()
}

pub(crate) fn mutate_service_configuration<T>(root: &Path, operation: impl FnOnce() -> T) -> T {
    let boundary = configuration_boundary(root);
    let mut generation = boundary.lock().expect("service configuration");
    // Invalidate even a partially failed write, and distinguish disable/re-enable.
    *generation = generation.wrapping_add(1);
    operation()
}

#[derive(Clone, PartialEq, Eq)]
pub struct ServiceConfigurationVersion {
    generation: u64,
    identity: CacheIdentity,
}

fn version_unlocked(
    root: &Path,
    generation: u64,
) -> Result<ServiceConfigurationVersion, RelayAccessError> {
    match resolve_journal_relay_state(root) {
        ResolvedServiceState::Configured { identity, .. } => Ok(ServiceConfigurationVersion {
            generation,
            identity,
        }),
        ResolvedServiceState::NotConfigured => Err(RelayAccessError::NotConfigured),
        ResolvedServiceState::Invalid(message) => Err(RelayAccessError::Unavailable(message)),
    }
}

pub fn current_service_configuration(
    root: &Path,
) -> Result<ServiceConfigurationVersion, RelayAccessError> {
    let boundary = configuration_boundary(root);
    let generation = boundary.lock().expect("service configuration");
    version_unlocked(root, *generation)
}

pub fn while_service_configuration_current<T>(
    root: &Path,
    expected: &ServiceConfigurationVersion,
    operation: impl FnOnce() -> T,
) -> Option<T> {
    let boundary = configuration_boundary(root);
    let generation = boundary.lock().expect("service configuration");
    (version_unlocked(root, *generation).as_ref().ok() == Some(expected)).then(operation)
}

#[derive(Clone)]
pub struct CurrentRelayAccess {
    pub snapshot: RelayAccessSnapshot,
    pub configuration: ServiceConfigurationVersion,
}

struct CacheEntry {
    access: CurrentRelayAccess,
    exp_unix: i64,
}

struct InFlightChannel {
    attempt_id: u64,
    configuration: ServiceConfigurationVersion,
    rx: tokio::sync::watch::Receiver<Option<Result<CurrentRelayAccess, RelayAccessError>>>,
}

struct CacheState {
    entry: Option<CacheEntry>,
    in_flight: Option<InFlightChannel>,
}

#[derive(Clone)]
pub struct RelayAccessCache {
    inner: Arc<RelayAccessCacheInner>,
}

struct RelayAccessCacheInner {
    epoch: AtomicU64,
    next_attempt_id: AtomicU64,
    state: Mutex<CacheState>,
    expected_issuer: String,
    request_timeout: Duration,
}

fn unavailable_change() -> RelayAccessError {
    RelayAccessError::Unavailable("relay service configuration changed".into())
}

fn expires(snapshot: &RelayAccessSnapshot) -> Option<i64> {
    OffsetDateTime::parse(&snapshot.expires_at, &Rfc3339)
        .ok()
        .map(|dt| dt.unix_timestamp())
}

impl RelayAccessCache {
    pub fn new() -> Self {
        Self::with_options("", DEFAULT_REQUEST_TIMEOUT)
    }

    /// An empty expected issuer accepts any nonempty issuer over the configured
    /// authenticated relay transport. It does not infer an issuer from its URL.
    pub fn with_options(expected_issuer: &str, request_timeout: Duration) -> Self {
        Self {
            inner: Arc::new(RelayAccessCacheInner {
                epoch: AtomicU64::new(0),
                next_attempt_id: AtomicU64::new(0),
                state: Mutex::new(CacheState {
                    entry: None,
                    in_flight: None,
                }),
                expected_issuer: expected_issuer.to_owned(),
                request_timeout: request_timeout.min(Duration::from_secs(15)),
            }),
        }
    }

    pub fn epoch(&self) -> u64 {
        self.inner.epoch.load(Ordering::SeqCst)
    }

    pub fn bump_epoch(&self) -> u64 {
        let mut state = self.inner.state.lock().expect("cache state");
        let epoch = self.inner.epoch.fetch_add(1, Ordering::SeqCst) + 1;
        state.entry = None;
        state.in_flight = None;
        epoch
    }

    pub fn try_current(&self, root: &Path, now: i64) -> Option<RelayAccessSnapshot> {
        let boundary = configuration_boundary(root);
        let generation = boundary.lock().expect("service configuration");
        let configuration = version_unlocked(root, *generation).ok()?;
        let mut state = self.inner.state.lock().expect("cache state");
        if let Some(entry) = &state.entry {
            if entry.exp_unix > now && entry.access.configuration == configuration {
                return Some(entry.access.snapshot.clone());
            }
            state.entry = None;
        }
        None
    }

    pub async fn acquire(
        &self,
        root: &Path,
        now: i64,
    ) -> Result<RelayAccessSnapshot, RelayAccessError> {
        self.acquire_current(root, now)
            .await
            .map(|access| access.snapshot)
    }

    pub async fn acquire_current(
        &self,
        root: &Path,
        now: i64,
    ) -> Result<CurrentRelayAccess, RelayAccessError> {
        let started = Instant::now();
        let (mut rx, attempt_id, epoch, configuration) = {
            let boundary = configuration_boundary(root);
            let generation = boundary.lock().expect("service configuration");
            let (instance_id, relay_origin, service_token, identity) =
                match resolve_journal_relay_state(root) {
                    ResolvedServiceState::NotConfigured => {
                        return Err(RelayAccessError::NotConfigured);
                    }
                    ResolvedServiceState::Invalid(message) => {
                        return Err(RelayAccessError::Unavailable(message));
                    }
                    ResolvedServiceState::Configured {
                        instance_id,
                        relay_origin,
                        service_token,
                        identity,
                    } => (instance_id, relay_origin, service_token, identity),
                };
            let configuration = ServiceConfigurationVersion {
                generation: *generation,
                identity,
            };
            let mut state = self.inner.state.lock().expect("cache state");
            if let Some(entry) = &state.entry {
                if entry.exp_unix > now && entry.access.configuration == configuration {
                    return Ok(entry.access.clone());
                }
                state.entry = None;
            }
            let epoch = self.epoch();
            if let Some(in_flight) = &state.in_flight
                && in_flight.configuration == configuration
            {
                (
                    in_flight.rx.clone(),
                    in_flight.attempt_id,
                    epoch,
                    configuration,
                )
            } else {
                let attempt_id = self.inner.next_attempt_id.fetch_add(1, Ordering::SeqCst) + 1;
                let (tx, rx) = tokio::sync::watch::channel(None);
                state.in_flight = Some(InFlightChannel {
                    attempt_id,
                    configuration: configuration.clone(),
                    rx: rx.clone(),
                });
                let inner = self.inner.clone();
                let expected_issuer = self.inner.expected_issuer.clone();
                let timeout = self.inner.request_timeout;
                let version = configuration.clone();
                let root = root.to_path_buf();
                tokio::spawn(async move {
                    let result = tokio::task::spawn_blocking(move || {
                        fetch_relay_access(
                            &relay_origin,
                            &service_token,
                            &instance_id,
                            &expected_issuer,
                            now,
                            timeout,
                        )
                    })
                    .await;
                    let mut result = match result {
                        Ok(result) => result.map(|snapshot| CurrentRelayAccess {
                            snapshot,
                            configuration: version.clone(),
                        }),
                        Err(_) => Err(RelayAccessError::Unavailable(
                            "relay access worker failed".into(),
                        )),
                    };
                    {
                        let boundary = configuration_boundary(&root);
                        let generation = boundary.lock().expect("service configuration");
                        let current = version_unlocked(&root, *generation);
                        let mut state = inner.state.lock().expect("cache state");
                        let owns_slot = state
                            .in_flight
                            .as_ref()
                            .is_some_and(|flight| flight.attempt_id == attempt_id);
                        let is_current = inner.epoch.load(Ordering::SeqCst) == epoch
                            && current.as_ref().ok() == Some(&version);
                        if !owns_slot || !is_current {
                            result = Err(unavailable_change());
                        }
                        if owns_slot {
                            if let Ok(access) = &result
                                && let Some(exp_unix) = expires(&access.snapshot)
                            {
                                state.entry = Some(CacheEntry {
                                    access: access.clone(),
                                    exp_unix,
                                });
                            }
                            state.in_flight = None;
                        }
                    }
                    let _ = tx.send(Some(result));
                });
                (rx, attempt_id, epoch, configuration)
            }
        };
        let result = tokio::time::timeout(self.inner.request_timeout, async {
            loop {
                // A coalescing subscriber may join after a result has arrived.
                if let Some(result) = rx.borrow_and_update().clone() {
                    return result;
                }
                rx.changed().await.map_err(|_| {
                    RelayAccessError::Unavailable("relay access worker dropped".into())
                })?;
            }
        })
        .await;
        let result = match result {
            Ok(result) => result,
            Err(_) => {
                let mut state = self.inner.state.lock().expect("cache state");
                if state
                    .in_flight
                    .as_ref()
                    .is_some_and(|flight| flight.attempt_id == attempt_id)
                {
                    state.in_flight = None;
                }
                return Err(RelayAccessError::Unavailable(
                    "relay access request timed out".into(),
                ));
            }
        };
        let boundary = configuration_boundary(root);
        let generation = boundary.lock().expect("service configuration");
        let current = version_unlocked(root, *generation)?;
        if current != configuration || self.epoch() != epoch {
            return Err(unavailable_change());
        }
        let access = result?;
        if expires(&access.snapshot)
            .is_none_or(|exp| exp <= now.saturating_add(started.elapsed().as_secs() as i64))
        {
            return Err(RelayAccessError::Unavailable("token expired".into()));
        }
        Ok(access)
    }
}

impl Default for RelayAccessCache {
    fn default() -> Self {
        Self::new()
    }
}
