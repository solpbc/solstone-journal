// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::io::Read;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use serde_json::Value;
use sha2::{Digest, Sha256};
use solstone_core_sol_link::pairing::RelayAccessSnapshot;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

pub const DEFAULT_RELAY_ISSUER: &str = "spl-relay-auth";
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
    if parts.len() != 3 {
        return Err(RelayAccessError::Unavailable(
            "malformed token: expected 3 parts".into(),
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

    // iss nonempty and matches expected_issuer
    match obj.get("iss").and_then(Value::as_str) {
        Some(iss) if !iss.is_empty() && iss == expected_issuer => {}
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

    if exp <= now {
        return Err(RelayAccessError::Unavailable("token expired".into()));
    }

    // Check body_expires_at parses as RFC3339 and unix timestamp matches exp
    let parsed_dt = OffsetDateTime::parse(body_expires_at, &Rfc3339)
        .map_err(|e| RelayAccessError::Unavailable(format!("invalid expires_at format: {e}")))?;
    if parsed_dt.unix_timestamp() != exp {
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
    let agent = build_isolated_ureq_agent(timeout);
    let url = format!("{}/token/access", relay_origin.trim_end_matches('/'));

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
        now,
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

struct CacheEntry {
    snapshot: RelayAccessSnapshot,
    exp_unix: i64,
    identity: CacheIdentity,
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
        Err(_) => return ResolvedServiceState::NotConfigured,
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

struct InFlightChannel {
    attempt_id: u64,
    _tx: tokio::sync::watch::Sender<Option<Result<RelayAccessSnapshot, RelayAccessError>>>,
    rx: tokio::sync::watch::Receiver<Option<Result<RelayAccessSnapshot, RelayAccessError>>>,
}

struct CacheState {
    entry: Option<CacheEntry>,
    in_flight: Option<InFlightChannel>,
}

#[derive(Clone)]
pub struct RelayAccessCache {
    inner: std::sync::Arc<RelayAccessCacheInner>,
}

struct RelayAccessCacheInner {
    epoch: AtomicU64,
    next_attempt_id: AtomicU64,
    state: std::sync::Mutex<CacheState>,
    expected_issuer: String,
    request_timeout: Duration,
}

impl RelayAccessCache {
    pub fn new() -> Self {
        Self::with_options(DEFAULT_RELAY_ISSUER, DEFAULT_REQUEST_TIMEOUT)
    }

    pub fn with_options(expected_issuer: &str, request_timeout: Duration) -> Self {
        Self {
            inner: std::sync::Arc::new(RelayAccessCacheInner {
                epoch: AtomicU64::new(0),
                next_attempt_id: AtomicU64::new(0),
                state: std::sync::Mutex::new(CacheState {
                    entry: None,
                    in_flight: None,
                }),
                expected_issuer: expected_issuer.to_owned(),
                request_timeout,
            }),
        }
    }

    pub fn epoch(&self) -> u64 {
        self.inner.epoch.load(Ordering::SeqCst)
    }

    pub fn bump_epoch(&self) -> u64 {
        let new_epoch = self.inner.epoch.fetch_add(1, Ordering::SeqCst) + 1;
        let mut lock = self.inner.state.lock().expect("lock not poisoned");
        lock.entry = None;
        lock.in_flight = None;
        new_epoch
    }

    pub fn try_current(&self, journal_root: &Path, now: i64) -> Option<RelayAccessSnapshot> {
        let resolved = resolve_journal_relay_state(journal_root);
        let ResolvedServiceState::Configured { identity, .. } = resolved else {
            return None;
        };
        let mut lock = self.inner.state.lock().expect("lock not poisoned");
        if let Some(entry) = &lock.entry {
            if entry.exp_unix > now && entry.identity == identity {
                return Some(entry.snapshot.clone());
            }
            lock.entry = None;
        }
        None
    }

    pub async fn acquire(
        &self,
        journal_root: &Path,
        now: i64,
    ) -> Result<RelayAccessSnapshot, RelayAccessError> {
        let resolved = resolve_journal_relay_state(journal_root);
        let (instance_id, relay_origin, service_token, identity) = match resolved {
            ResolvedServiceState::NotConfigured => return Err(RelayAccessError::NotConfigured),
            ResolvedServiceState::Invalid(msg) => return Err(RelayAccessError::Unavailable(msg)),
            ResolvedServiceState::Configured {
                instance_id,
                relay_origin,
                service_token,
                identity,
            } => (instance_id, relay_origin, service_token, identity),
        };

        let (mut rx, attempt_id) = {
            let mut lock = self.inner.state.lock().expect("lock not poisoned");
            if let Some(entry) = &lock.entry {
                if entry.exp_unix > now && entry.identity == identity {
                    return Ok(entry.snapshot.clone());
                }
                lock.entry = None;
            }

            let cur_epoch = self.inner.epoch.load(Ordering::SeqCst);
            if let Some(ref in_flight) = lock.in_flight {
                (in_flight.rx.clone(), in_flight.attempt_id)
            } else {
                let attempt_id = self.inner.next_attempt_id.fetch_add(1, Ordering::SeqCst) + 1;
                let (tx, rx) = tokio::sync::watch::channel(None);
                lock.in_flight = Some(InFlightChannel {
                    attempt_id,
                    _tx: tx.clone(),
                    rx: rx.clone(),
                });

                let inner = std::sync::Arc::clone(&self.inner);
                let expected_issuer = self.inner.expected_issuer.clone();
                let timeout = self.inner.request_timeout;
                let origin = relay_origin.clone();
                let tok = service_token.clone();
                let i_id = instance_id.clone();
                let id_copy = identity.clone();

                tokio::spawn(async move {
                    let result = tokio::task::spawn_blocking(move || {
                        fetch_relay_access(&origin, &tok, &i_id, &expected_issuer, now, timeout)
                    })
                    .await;
                    let result = match result {
                        Ok(res) => res,
                        Err(e) => Err(RelayAccessError::Unavailable(format!(
                            "task join error: {e}"
                        ))),
                    };

                    {
                        let mut lock = inner.state.lock().expect("lock not poisoned");
                        let slot_matches = lock
                            .in_flight
                            .as_ref()
                            .is_some_and(|inf| inf.attempt_id == attempt_id);
                        let epoch_matches = inner.epoch.load(Ordering::SeqCst) == cur_epoch;

                        if slot_matches && epoch_matches {
                            if let Ok(ref snapshot) = result
                                && let Ok(dt) =
                                    OffsetDateTime::parse(&snapshot.expires_at, &Rfc3339)
                            {
                                lock.entry = Some(CacheEntry {
                                    snapshot: snapshot.clone(),
                                    exp_unix: dt.unix_timestamp(),
                                    identity: id_copy,
                                });
                            }
                            lock.in_flight = None;
                        }
                    }

                    let _ = tx.send(Some(result));
                });

                (rx, attempt_id)
            }
        };

        let wait_result = tokio::time::timeout(self.inner.request_timeout, async {
            loop {
                if rx.changed().await.is_err() {
                    return Err(RelayAccessError::Unavailable(
                        "in-flight fetch dropped".into(),
                    ));
                }
                if let Some(res) = rx.borrow().clone() {
                    return res;
                }
            }
        })
        .await;

        match wait_result {
            Ok(res) => res,
            Err(_) => {
                let mut lock = self.inner.state.lock().expect("lock not poisoned");
                if let Some(ref in_flight) = lock.in_flight
                    && in_flight.attempt_id == attempt_id
                {
                    lock.in_flight = None;
                }
                Err(RelayAccessError::Unavailable(
                    "relay access request timed out".into(),
                ))
            }
        }
    }
}

impl Default for RelayAccessCache {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct TempJournal(PathBuf);
    impl TempJournal {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "solstone-spl-relay-access-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for TempJournal {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn create_test_jwt(
        iss: &str,
        sub: &str,
        aud: &str,
        scope: &str,
        ver: i64,
        instance_id: &str,
        iat: i64,
        exp: i64,
        jti: &str,
    ) -> String {
        let header = "eyJhbGciOiJub25lIiwidHlwIjoiSldUIn0";
        let payload = json!({
            "iss": iss,
            "sub": sub,
            "aud": aud,
            "scope": scope,
            "ver": ver,
            "instance_id": instance_id,
            "iat": iat,
            "exp": exp,
            "jti": jti,
        });
        let payload_str = serde_json::to_string(&payload).unwrap();
        let payload_b64 = base64_url_encode(payload_str.as_bytes());
        format!("{header}.{payload_b64}.signature")
    }

    fn base64_url_encode(bytes: &[u8]) -> String {
        const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
        let mut out = String::new();
        for chunk in bytes.chunks(3) {
            let mut b = (chunk[0] as u32) << 16;
            if chunk.len() > 1 {
                b |= (chunk[1] as u32) << 8;
            }
            if chunk.len() > 2 {
                b |= chunk[2] as u32;
            }
            out.push(CHARS[((b >> 18) & 0x3F) as usize] as char);
            out.push(CHARS[((b >> 12) & 0x3F) as usize] as char);
            if chunk.len() > 1 {
                out.push(CHARS[((b >> 6) & 0x3F) as usize] as char);
            }
            if chunk.len() > 2 {
                out.push(CHARS[(b & 0x3F) as usize] as char);
            }
        }
        out
    }

    #[test]
    fn test_jwt_validation_strict() {
        let now = 1000;
        let exp = 2000;
        let exp_rfc3339 = "1970-01-01T00:33:20Z";
        let inst = "inst-abc";
        let iss = "my-issuer";

        let valid_token = create_test_jwt(
            iss,
            &format!("instance:{inst}"),
            "spl-relay",
            "session.dial",
            2,
            inst,
            500,
            exp,
            "jti-1",
        );

        let validated_exp =
            validate_device_token(&valid_token, inst, iss, exp_rfc3339, now).unwrap();
        assert_eq!(validated_exp, exp);

        // Mismatched issuer
        assert!(validate_device_token(&valid_token, inst, "other-iss", exp_rfc3339, now).is_err());

        // Mismatched instance_id
        assert!(validate_device_token(&valid_token, "other-inst", iss, exp_rfc3339, now).is_err());

        // Wrong sub format
        let bad_sub = create_test_jwt(
            iss,
            "user:123",
            "spl-relay",
            "session.dial",
            2,
            inst,
            500,
            exp,
            "jti-1",
        );
        assert!(validate_device_token(&bad_sub, inst, iss, exp_rfc3339, now).is_err());

        // Wrong aud
        let bad_aud = create_test_jwt(
            iss,
            &format!("instance:{inst}"),
            "wrong-aud",
            "session.dial",
            2,
            inst,
            500,
            exp,
            "jti-1",
        );
        assert!(validate_device_token(&bad_aud, inst, iss, exp_rfc3339, now).is_err());

        // Wrong scope
        let bad_scope = create_test_jwt(
            iss,
            &format!("instance:{inst}"),
            "spl-relay",
            "session.admin",
            2,
            inst,
            500,
            exp,
            "jti-1",
        );
        assert!(validate_device_token(&bad_scope, inst, iss, exp_rfc3339, now).is_err());

        // Empty jti
        let empty_jti = create_test_jwt(
            iss,
            &format!("instance:{inst}"),
            "spl-relay",
            "session.dial",
            2,
            inst,
            500,
            exp,
            "",
        );
        assert!(validate_device_token(&empty_jti, inst, iss, exp_rfc3339, now).is_err());

        // Expired (exp <= now)
        assert!(validate_device_token(&valid_token, inst, iss, exp_rfc3339, 3000).is_err());

        // exp <= iat
        let exp_le_iat = create_test_jwt(
            iss,
            &format!("instance:{inst}"),
            "spl-relay",
            "session.dial",
            2,
            inst,
            2500,
            2000,
            "jti-1",
        );
        assert!(validate_device_token(&exp_le_iat, inst, iss, exp_rfc3339, now).is_err());

        // Wrong ver
        let bad_ver_token = create_test_jwt(
            iss,
            &format!("instance:{inst}"),
            "spl-relay",
            "session.dial",
            1,
            inst,
            500,
            exp,
            "jti-1",
        );
        assert!(validate_device_token(&bad_ver_token, inst, iss, exp_rfc3339, now).is_err());

        // Extra key rejected
        let header = "eyJhbGciOiJub25lIiwidHlwIjoiSldUIn0";
        let payload_extra = json!({
            "iss": iss,
            "sub": format!("instance:{inst}"),
            "aud": "spl-relay",
            "scope": "session.dial",
            "ver": 2,
            "instance_id": inst,
            "iat": 500,
            "exp": exp,
            "jti": "jti-1",
            "extra": "bad",
        });
        let payload_b64 =
            base64_url_encode(serde_json::to_string(&payload_extra).unwrap().as_bytes());
        let bad_key_token = format!("{header}.{payload_b64}.sig");
        assert!(validate_device_token(&bad_key_token, inst, iss, exp_rfc3339, now).is_err());

        // Non-integer iat / exp
        let payload_str_iat = json!({
            "iss": iss,
            "sub": format!("instance:{inst}"),
            "aud": "spl-relay",
            "scope": "session.dial",
            "ver": 2,
            "instance_id": inst,
            "iat": "500",
            "exp": exp,
            "jti": "jti-1",
        });
        let bad_iat_token = format!(
            "{header}.{}.sig",
            base64_url_encode(serde_json::to_string(&payload_str_iat).unwrap().as_bytes())
        );
        assert!(validate_device_token(&bad_iat_token, inst, iss, exp_rfc3339, now).is_err());

        // RFC3339 mismatch
        assert!(
            validate_device_token(&valid_token, inst, iss, "1970-01-01T00:33:21Z", now).is_err()
        );
    }

    #[test]
    fn test_fetch_relay_access_refuses_oversized_request_body() {
        let oversized_token = "a".repeat(17000);
        let err = fetch_relay_access(
            "http://127.0.0.1:1",
            &oversized_token,
            "inst-1",
            DEFAULT_RELAY_ISSUER,
            0,
            Duration::from_millis(100),
        )
        .unwrap_err();
        assert!(matches!(err, RelayAccessError::Unavailable(_)));
    }

    #[test]
    fn test_redirect_location_never_followed() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();

        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let response = "HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:9999/leak\r\nContent-Length: 0\r\n\r\n";
                let _ = stream.write_all(response.as_bytes());
            }
        });

        let err = fetch_relay_access(
            &format!("http://127.0.0.1:{port}"),
            "tok-1",
            "inst-1",
            DEFAULT_RELAY_ISSUER,
            0,
            Duration::from_millis(500),
        )
        .unwrap_err();
        assert!(matches!(err, RelayAccessError::Unavailable(_)));
    }

    fn setup_test_journal(
        journal_root: &std::path::Path,
        instance_id: &str,
        service_token: &str,
        relay_origin: &str,
    ) {
        std::fs::create_dir_all(journal_root.join("config")).unwrap();
        std::fs::create_dir_all(journal_root.join("link/tokens")).unwrap();

        std::fs::write(
            journal_root.join("config/journal.json"),
            json!({
                "link": {
                    "posture": "spl",
                    "relay_url": relay_origin,
                }
            })
            .to_string(),
        )
        .unwrap();

        std::fs::write(
            journal_root.join("link/tokens/account.json"),
            json!({
                "service_token": service_token,
            })
            .to_string(),
        )
        .unwrap();

        std::fs::write(
            journal_root.join("link/state.json"),
            json!({
                "instance_id": instance_id,
                "home_label": "test-home",
            })
            .to_string(),
        )
        .unwrap();
    }

    #[tokio::test]
    async fn test_stalling_listener_timeout_and_late_overtake() {
        let stall_listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let stall_port = stall_listener.local_addr().unwrap().port();

        let (stall_tx, stall_rx) = tokio::sync::oneshot::channel();
        let (finish_tx, finish_rx) = std::sync::mpsc::channel();

        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = stall_listener.accept() {
                let mut buf = [0u8; 1024];
                let _ = stream.read(&mut buf);
                let _ = stall_tx.send(());
                let _ = finish_rx.recv_timeout(Duration::from_secs(2));
                let exp = 2000;
                let exp_rfc3339 = "1970-01-01T00:33:20Z";
                let token_a = create_test_jwt(
                    DEFAULT_RELAY_ISSUER,
                    "instance:inst-test",
                    "spl-relay",
                    "session.dial",
                    2,
                    "inst-test",
                    500,
                    exp,
                    "jti-a",
                );
                let body = json!({
                    "protocol_version": 2,
                    "device_token": token_a,
                    "expires_at": exp_rfc3339,
                })
                .to_string();
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(response.as_bytes());
            }
        });

        let journal = TempJournal::new();
        let journal_root = &journal.0;
        setup_test_journal(
            journal_root,
            "inst-test",
            "token-xyz",
            &format!("http://127.0.0.1:{stall_port}"),
        );

        let cache = RelayAccessCache::with_options(DEFAULT_RELAY_ISSUER, Duration::from_millis(50));

        // Caller 1 starts acquire and times out after 50ms
        let res1 = cache.acquire(journal_root, 1000).await;
        assert!(res1.is_err(), "caller 1 should time out");
        tokio::time::timeout(Duration::from_secs(2), stall_rx)
            .await
            .expect("not timed out")
            .expect("stall listener got connection");

        // Setup fast server B
        let fast_listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let fast_port = fast_listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = fast_listener.accept() {
                let mut buf = [0u8; 1024];
                let _ = stream.read(&mut buf);
                let exp = 3000;
                let exp_rfc3339 = "1970-01-01T00:50:00Z";
                let token_b = create_test_jwt(
                    DEFAULT_RELAY_ISSUER,
                    "instance:inst-test",
                    "spl-relay",
                    "session.dial",
                    2,
                    "inst-test",
                    500,
                    exp,
                    "jti-b",
                );
                let body = json!({
                    "protocol_version": 2,
                    "device_token": token_b,
                    "expires_at": exp_rfc3339,
                })
                .to_string();
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(response.as_bytes());
            }
        });

        // Update config to point to fast server B
        std::fs::write(
            journal_root.join("config/journal.json"),
            json!({
                "link": {
                    "posture": "spl",
                    "relay_url": format!("http://127.0.0.1:{fast_port}"),
                }
            })
            .to_string(),
        )
        .unwrap();

        // Caller 2 starts acquire with successor attempt and succeeds
        let res2 = cache.acquire(journal_root, 1000).await.unwrap();
        assert_eq!(res2.expires_at, "1970-01-01T00:50:00Z");

        // Now let stalling worker A finish
        let _ = finish_tx.send(());
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Current cache entry must still be snapshot B
        let current = cache.try_current(journal_root, 1000).unwrap();
        assert_eq!(current.expires_at, "1970-01-01T00:50:00Z");
    }

    #[tokio::test]
    async fn test_epoch_bump_rejects_late_post() {
        let stall_listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let stall_port = stall_listener.local_addr().unwrap().port();

        let (stall_tx, stall_rx) = tokio::sync::oneshot::channel();
        let (finish_tx, finish_rx) = std::sync::mpsc::channel();

        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = stall_listener.accept() {
                let mut buf = [0u8; 1024];
                let _ = stream.read(&mut buf);
                let _ = stall_tx.send(());
                let _ = finish_rx.recv_timeout(Duration::from_secs(2));
                let exp = 2000;
                let exp_rfc3339 = "1970-01-01T00:33:20Z";
                let token_a = create_test_jwt(
                    DEFAULT_RELAY_ISSUER,
                    "instance:inst-test",
                    "spl-relay",
                    "session.dial",
                    2,
                    "inst-test",
                    500,
                    exp,
                    "jti-a",
                );
                let body = json!({
                    "protocol_version": 2,
                    "device_token": token_a,
                    "expires_at": exp_rfc3339,
                })
                .to_string();
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(response.as_bytes());
            }
        });

        let journal = TempJournal::new();
        let journal_root = &journal.0;
        setup_test_journal(
            journal_root,
            "inst-test",
            "token-xyz",
            &format!("http://127.0.0.1:{stall_port}"),
        );

        let cache = RelayAccessCache::with_options(DEFAULT_RELAY_ISSUER, Duration::from_millis(50));

        let cache_clone = cache.clone();
        let root_clone = journal_root.to_path_buf();
        let handle = tokio::spawn(async move { cache_clone.acquire(&root_clone, 1000).await });

        tokio::time::timeout(Duration::from_secs(2), stall_rx)
            .await
            .expect("not timed out")
            .expect("stall listener got connection");
        cache.bump_epoch();

        let _ = finish_tx.send(());
        let _ = handle.await;

        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(cache.try_current(journal_root, 1000).is_none());
    }
}
