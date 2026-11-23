// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Hardened loopback callback listener, paste/brokered callback URL validation,
//! and single-use atomic attempt claiming.

use std::collections::BTreeMap;
use std::fmt;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use subtle::ConstantTimeEq;

pub const SUCCESS_BODY: &str = "signed in. you can close this tab and go back to your journal";
pub const FAILURE_BODY: &str = "sign-in didn't finish. go back to your journal and try again";

/// Caller-supplied plain-text bodies written to the browser tab on loopback callback.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallbackCopy {
    pub success: String,
    pub failure: String,
}

impl CallbackCopy {
    pub fn english() -> Self {
        Self {
            success: SUCCESS_BODY.to_owned(),
            failure: FAILURE_BODY.to_owned(),
        }
    }
}

impl Default for CallbackCopy {
    fn default() -> Self {
        Self::english()
    }
}
pub const MAX_CALLBACK_PATH_BYTES: usize = 256;
pub const MAX_CALLBACK_HEADER_BYTES: usize = 8 * 1024;
pub const SOCKET_IO_TIMEOUT: Duration = Duration::from_secs(5);
pub const DEFAULT_ATTEMPT_EXPIRATION_SECS: u64 = 600;

/// Loopback hostname advertised in `redirect_uri` and enforced on the HTTP `Host` header.
///
/// Regardless of host spelling, the TCP socket always binds `127.0.0.1:<port>` (never `0.0.0.0`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoopbackHost {
    Ipv4Loopback,
    Localhost,
}

impl LoopbackHost {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ipv4Loopback => "127.0.0.1",
            Self::Localhost => "localhost",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "127.0.0.1" => Some(Self::Ipv4Loopback),
            "localhost" => Some(Self::Localhost),
            _ => None,
        }
    }
}

/// Specification for binding a local loopback callback listener.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoopbackBindSpec {
    pub host: LoopbackHost,
    pub path: String,
    /// `0` selects an OS-assigned ephemeral port; non-zero must be in `1024..=65535`.
    pub port: u16,
}

impl LoopbackBindSpec {
    pub fn ephemeral(host: LoopbackHost, path: impl Into<String>) -> Self {
        Self {
            host,
            path: path.into(),
            port: 0,
        }
    }

    pub fn fixed(host: LoopbackHost, path: impl Into<String>, port: u16) -> Self {
        Self {
            host,
            path: path.into(),
            port,
        }
    }

    pub fn validate(&self) -> Result<(), CallbackError> {
        if self.port != 0 && self.port < 1024 {
            return Err(CallbackError::InvalidBindSpec);
        }
        if !is_valid_callback_path(&self.path) {
            return Err(CallbackError::InvalidBindSpec);
        }
        Ok(())
    }
}

/// Validate a loopback callback path (`/`-prefixed, visible ASCII, no `?`, `#`, or `..`, <= 256 B).
pub fn is_valid_callback_path(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= MAX_CALLBACK_PATH_BYTES
        && path.starts_with('/')
        && path.bytes().all(|b| b.is_ascii_graphic())
        && !path.contains('?')
        && !path.contains('#')
        && !path.contains("..")
}

/// Errors encountered while binding, parsing, or validating an OAuth callback.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallbackError {
    InvalidBindSpec,
    DuplicateParameter,
    MalformedQuery,
    InvalidRedirectUri,
    StateMismatch,
    MissingCode,
    MissingIssuer,
    IssuerMismatch,
    Timeout,
    Cancelled,
    Io(String),
}

impl fmt::Display for CallbackError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidBindSpec => formatter.write_str("invalid loopback callback bind spec"),
            Self::DuplicateParameter => formatter.write_str("duplicate callback query parameter"),
            Self::MalformedQuery => formatter.write_str("malformed callback query string"),
            Self::InvalidRedirectUri => {
                formatter.write_str("callback redirect URI did not match expected target")
            }
            Self::StateMismatch => formatter.write_str("OAuth state parameter mismatch"),
            Self::MissingCode => formatter.write_str("authorization code missing from callback"),
            Self::MissingIssuer => {
                formatter.write_str("required authorization server iss parameter missing")
            }
            Self::IssuerMismatch => formatter
                .write_str("authorization server iss parameter did not match expected issuer"),
            Self::Timeout => formatter.write_str("timed out waiting for OAuth callback"),
            Self::Cancelled => formatter.write_str("OAuth callback wait was cancelled"),
            Self::Io(message) => write!(formatter, "callback listener I/O error: {message}"),
        }
    }
}

impl std::error::Error for CallbackError {}

/// Bound local TCP listener on `127.0.0.1:<port>`.
pub struct LoopbackListener {
    listener: TcpListener,
    host: LoopbackHost,
    port: u16,
    path: String,
    redirect_uri: String,
    expected_host: String,
}

impl LoopbackListener {
    pub fn bind(spec: &LoopbackBindSpec) -> Result<Self, CallbackError> {
        spec.validate()?;
        let listener = TcpListener::bind(("127.0.0.1", spec.port))
            .map_err(|err| CallbackError::Io(err.to_string()))?;
        let port = listener
            .local_addr()
            .map_err(|err| CallbackError::Io(err.to_string()))?
            .port();
        let expected_host = format!("{}:{port}", spec.host.as_str());
        let redirect_uri = format!("http://{expected_host}{}", spec.path);
        Ok(Self {
            listener,
            host: spec.host,
            port,
            path: spec.path.clone(),
            redirect_uri,
            expected_host,
        })
    }

    pub fn host(&self) -> LoopbackHost {
        self.host
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    pub fn redirect_uri(&self) -> &str {
        &self.redirect_uri
    }

    pub fn expected_host(&self) -> &str {
        &self.expected_host
    }

    pub fn listener(&self) -> &TcpListener {
        &self.listener
    }
}

/// Validation policy applied to both loopback HTTP requests and pasted/brokered callback URLs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallbackPolicy {
    pub expected_state: String,
    pub expected_issuer: Option<String>,
    /// When `true`, `expected_issuer` MUST be `Some` and callback `iss` MUST be present and match
    /// (RFC 9207). When `false`, an absent `iss` is permitted, but if `iss` IS present and
    /// `expected_issuer` is `Some`, it still MUST match.
    pub require_issuer: bool,
    /// Accepted `scheme://authority/path` prefixes for pasted or brokered callback URLs.
    pub accepted_redirect_uris: Vec<String>,
    /// Allow bare `code=...&state=...` pasted strings with no URI prefix.
    pub allow_bare_query: bool,
}

impl CallbackPolicy {
    pub fn new(expected_state: impl Into<String>, redirect_uri: impl Into<String>) -> Self {
        Self {
            expected_state: expected_state.into(),
            expected_issuer: None,
            require_issuer: false,
            accepted_redirect_uris: vec![redirect_uri.into()],
            allow_bare_query: false,
        }
    }

    pub fn with_expected_issuer(mut self, issuer: impl Into<String>, required: bool) -> Self {
        self.expected_issuer = Some(issuer.into());
        self.require_issuer = required;
        self
    }

    pub fn with_allow_bare_query(mut self, allow: bool) -> Self {
        self.allow_bare_query = allow;
        self
    }
}

/// Validated authorization callback payload bound to this attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedCallback {
    pub code: String,
    pub state: String,
    pub issuer: Option<String>,
    pub params: BTreeMap<String, String>,
}

/// Reason a request/input was ignored without consuming the active attempt (CSRF-DoS defense).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IgnoredReason {
    MethodNotAllowed,
    HostMismatch,
    PathMismatch,
    RedirectUriMismatch,
    MalformedQuery,
    DuplicateParameter,
    StateMismatch,
}

/// Outcome of validating a callback request or pasted/brokered URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallbackOutcome {
    /// Valid `state` (+ `iss`) and non-empty `code`: consumes the attempt.
    Accepted(ValidatedCallback),
    /// Valid `state` (+ `iss`) carrying an OAuth provider `error`: consumes the attempt.
    ProviderError {
        error: String,
        error_description: Option<String>,
    },
    /// Valid `state`, but callback payload violates RFC 9207 `iss` or lacks `code`: consumes attempt.
    InvalidForAttempt(CallbackError),
    /// Stray / forged / wrong-state / wrong-host request: MUST NOT consume the attempt.
    Ignored(IgnoredReason),
}

/// Parse URL query pairs (`k=v&...`), refusing duplicate keys or invalid percent escapes.
pub fn parse_query_pairs_refusing_duplicates(
    query: &str,
) -> Result<BTreeMap<String, String>, CallbackError> {
    let mut params = BTreeMap::new();
    for pair in query.split('&') {
        if pair.is_empty() {
            continue;
        }
        let (key, value) = match pair.split_once('=') {
            Some((k, v)) => (k, v),
            None => (pair, ""),
        };
        let decoded_key = solstone_core_auth_flow::percent_decode(key)
            .map_err(|_| CallbackError::MalformedQuery)?;
        let decoded_value = solstone_core_auth_flow::percent_decode(value)
            .map_err(|_| CallbackError::MalformedQuery)?;
        if params.contains_key(&decoded_key) {
            return Err(CallbackError::DuplicateParameter);
        }
        params.insert(decoded_key, decoded_value);
    }
    Ok(params)
}

/// Validate parsed callback query parameters against `policy`.
///
/// Uses constant-time `state` comparison (`subtle::ConstantTimeEq`). Missing or mismatched
/// `state` returns [`CallbackOutcome::Ignored`] so an attacker cannot DoS a pending attempt.
pub fn validate_callback_params(
    params: &BTreeMap<String, String>,
    policy: &CallbackPolicy,
) -> CallbackOutcome {
    if policy.expected_state.is_empty() {
        return CallbackOutcome::Ignored(IgnoredReason::StateMismatch);
    }
    let Some(state) = params.get("state") else {
        return CallbackOutcome::Ignored(IgnoredReason::StateMismatch);
    };
    let matches_state: bool = state
        .as_bytes()
        .ct_eq(policy.expected_state.as_bytes())
        .into();
    if !matches_state {
        return CallbackOutcome::Ignored(IgnoredReason::StateMismatch);
    }

    // RFC 9207: verify `iss` whenever required or present alongside an expected issuer.
    let issuer = params.get("iss").cloned();
    if let Some(expected_iss) = &policy.expected_issuer {
        match issuer.as_deref() {
            Some(actual_iss) if actual_iss == expected_iss => {}
            Some(_) => return CallbackOutcome::InvalidForAttempt(CallbackError::IssuerMismatch),
            None if policy.require_issuer => {
                return CallbackOutcome::InvalidForAttempt(CallbackError::MissingIssuer);
            }
            None => {}
        }
    } else if policy.require_issuer {
        return CallbackOutcome::InvalidForAttempt(CallbackError::MissingIssuer);
    }

    if let Some(error) = params.get("error")
        && !error.is_empty()
    {
        return CallbackOutcome::ProviderError {
            error: error.clone(),
            error_description: params
                .get("error_description")
                .filter(|d| !d.is_empty())
                .cloned(),
        };
    }

    let Some(code) = params.get("code") else {
        return CallbackOutcome::InvalidForAttempt(CallbackError::MissingCode);
    };
    if code.is_empty() {
        return CallbackOutcome::InvalidForAttempt(CallbackError::MissingCode);
    }

    CallbackOutcome::Accepted(ValidatedCallback {
        code: code.clone(),
        state: state.clone(),
        issuer,
        params: params.clone(),
    })
}

/// Parse and validate a brokered or user-pasted `callback_url`.
pub fn parse_callback_url(input: &str, policy: &CallbackPolicy) -> CallbackOutcome {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return CallbackOutcome::Ignored(IgnoredReason::RedirectUriMismatch);
    }
    // Strip any URL fragment (RFC 3986); fragments are never part of authorization code queries.
    let without_fragment = trimmed.split('#').next().unwrap_or(trimmed);

    let (base_part, query_part) = match without_fragment.split_once('?') {
        Some((base, query)) => (base, query),
        None if policy.allow_bare_query && !without_fragment.contains("://") => {
            ("", without_fragment)
        }
        None => return CallbackOutcome::Ignored(IgnoredReason::RedirectUriMismatch),
    };

    if base_part.is_empty() {
        if !policy.allow_bare_query {
            return CallbackOutcome::Ignored(IgnoredReason::RedirectUriMismatch);
        }
    } else if !redirect_uri_matches_any(base_part, &policy.accepted_redirect_uris) {
        return CallbackOutcome::Ignored(IgnoredReason::RedirectUriMismatch);
    }

    let params = match parse_query_pairs_refusing_duplicates(query_part) {
        Ok(params) => params,
        Err(CallbackError::DuplicateParameter) => {
            return CallbackOutcome::Ignored(IgnoredReason::DuplicateParameter);
        }
        Err(_) => return CallbackOutcome::Ignored(IgnoredReason::MalformedQuery),
    };

    validate_callback_params(&params, policy)
}

fn redirect_uri_matches_any(candidate: &str, accepted: &[String]) -> bool {
    accepted
        .iter()
        .any(|expected| redirect_uri_matches(candidate, expected))
}

fn redirect_uri_matches(candidate: &str, expected: &str) -> bool {
    let expected_base = expected.split(['?', '#']).next().unwrap_or(expected);
    if candidate == expected_base {
        return true;
    }
    // For http/https URIs, compare scheme and host ASCII-case-insensitively while keeping
    // port and path exact. Custom schemes (e.g. `solstone://oauth/callback`,
    // `app.solstone:/callback`) compare scheme case-insensitively and authority/path exact.
    let Some((c_scheme, c_rest)) = candidate.split_once(':') else {
        return false;
    };
    let Some((e_scheme, e_rest)) = expected_base.split_once(':') else {
        return false;
    };
    if !c_scheme.eq_ignore_ascii_case(e_scheme) {
        return false;
    }
    if c_scheme.eq_ignore_ascii_case("http") || c_scheme.eq_ignore_ascii_case("https") {
        let (Some(c_after), Some(e_after)) = (c_rest.strip_prefix("//"), e_rest.strip_prefix("//"))
        else {
            return false;
        };
        let (c_auth, c_path) = match c_after.find('/') {
            Some(idx) => (&c_after[..idx], &c_after[idx..]),
            None => (c_after, "/"),
        };
        let (e_auth, e_path) = match e_after.find('/') {
            Some(idx) => (&e_after[..idx], &e_after[idx..]),
            None => (e_after, "/"),
        };
        c_auth.eq_ignore_ascii_case(e_auth) && c_path == e_path
    } else {
        c_rest == e_rest
    }
}

/// Write a plain-text HTTP/1.1 callback response with `Cache-Control: no-store`.
pub fn write_http_response(
    stream: &mut TcpStream,
    status_code: u16,
    status_text: &str,
    body: &str,
) -> std::io::Result<()> {
    let response = format!(
        "HTTP/1.1 {status_code} {status_text}\r\n\
         Content-Type: text/plain; charset=utf-8\r\n\
         Cache-Control: no-store\r\n\
         Content-Length: {}\r\n\
         Connection: close\r\n\
         \r\n\
         {body}",
        body.len()
    );
    stream.write_all(response.as_bytes())?;
    stream.flush()
}

/// Write the terminal browser tab message on a held loopback callback stream.
pub fn respond_to_callback_stream(stream: Option<&mut TcpStream>, success: bool) {
    respond_to_callback_stream_with_copy(stream, success, &CallbackCopy::english());
}

/// Write a caller-localized browser tab message on a held loopback callback stream.
pub fn respond_to_callback_stream_with_copy(
    stream: Option<&mut TcpStream>,
    success: bool,
    copy: &CallbackCopy,
) {
    if let Some(stream) = stream {
        if success {
            let _ = write_http_response(stream, 200, "OK", &copy.success);
        } else {
            let _ = write_http_response(stream, 400, "Bad Request", &copy.failure);
        }
    }
}

/// Result received from the loopback HTTP listener for an attempt-bound callback.
pub struct LoopbackDelivery {
    pub outcome: CallbackOutcome,
    pub stream: TcpStream,
}

/// Accept loopback HTTP requests until a valid attempt-bound callback arrives, `timeout`
/// elapses, or `cancel` is signalled.
///
/// Wrong `Host`, wrong path, non-`GET`, duplicate query keys, and wrong/missing `state`
/// receive HTTP `400`/`404` and **do not** close the listener or consume the attempt.
pub fn receive_loopback_callback(
    bound: &LoopbackListener,
    policy: &CallbackPolicy,
    timeout: Duration,
    cancel: &AtomicBool,
) -> Result<LoopbackDelivery, CallbackError> {
    receive_loopback_callback_with_copy(bound, policy, timeout, cancel, &CallbackCopy::english())
}

/// [`receive_loopback_callback`] with caller-localized browser tab error copy.
pub fn receive_loopback_callback_with_copy(
    bound: &LoopbackListener,
    policy: &CallbackPolicy,
    timeout: Duration,
    cancel: &AtomicBool,
    copy: &CallbackCopy,
) -> Result<LoopbackDelivery, CallbackError> {
    bound
        .listener
        .set_nonblocking(true)
        .map_err(|err| CallbackError::Io(err.to_string()))?;
    let start = Instant::now();

    loop {
        if cancel.load(Ordering::Acquire) {
            return Err(CallbackError::Cancelled);
        }
        if start.elapsed() >= timeout {
            return Err(CallbackError::Timeout);
        }

        let mut stream = match bound.listener.accept() {
            Ok((stream, _)) => stream,
            Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(20));
                continue;
            }
            Err(err) => return Err(CallbackError::Io(err.to_string())),
        };

        let _ = stream.set_nonblocking(false);
        let _ = stream.set_read_timeout(Some(SOCKET_IO_TIMEOUT));
        let _ = stream.set_write_timeout(Some(SOCKET_IO_TIMEOUT));

        let Some(raw_request) = read_http_head(&mut stream) else {
            let _ = write_http_response(&mut stream, 400, "Bad Request", &copy.failure);
            continue;
        };

        let request = String::from_utf8_lossy(&raw_request);
        let mut lines = request.split("\r\n");
        let first_line = lines.next().unwrap_or_default();
        let mut parts = first_line.split_whitespace();
        let method = parts.next().unwrap_or_default();
        let target = parts.next().unwrap_or_default();

        if method != "GET" {
            let _ = write_http_response(&mut stream, 400, "Bad Request", &copy.failure);
            continue;
        }

        let mut host_headers = Vec::new();
        for line in lines {
            if line.is_empty() {
                break;
            }
            if let Some((name, value)) = line.split_once(':')
                && name.trim().eq_ignore_ascii_case("host")
            {
                host_headers.push(value.trim());
            }
        }
        if host_headers.len() != 1 || !host_headers[0].eq_ignore_ascii_case(bound.expected_host()) {
            let _ = write_http_response(&mut stream, 404, "Not Found", "Not Found");
            continue;
        }

        let (path, query) = match target.split_once('?') {
            Some((p, q)) => (p, q),
            None => (target, ""),
        };
        if path != bound.path() {
            let _ = write_http_response(&mut stream, 404, "Not Found", "Not Found");
            continue;
        }

        let params = match parse_query_pairs_refusing_duplicates(query) {
            Ok(params) => params,
            Err(_) => {
                let _ = write_http_response(&mut stream, 400, "Bad Request", &copy.failure);
                continue;
            }
        };

        match validate_callback_params(&params, policy) {
            CallbackOutcome::Ignored(_) => {
                let _ = write_http_response(&mut stream, 400, "Bad Request", &copy.failure);
                continue;
            }
            outcome @ (CallbackOutcome::Accepted(_)
            | CallbackOutcome::ProviderError { .. }
            | CallbackOutcome::InvalidForAttempt(_)) => {
                return Ok(LoopbackDelivery { outcome, stream });
            }
        }
    }
}

fn read_http_head(stream: &mut TcpStream) -> Option<Vec<u8>> {
    let mut buffer = Vec::new();
    let mut chunk = [0_u8; 1024];
    loop {
        let n = stream.read(&mut chunk).ok()?;
        if n == 0 {
            return (!buffer.is_empty()).then_some(buffer);
        }
        if buffer.len().saturating_add(n) > MAX_CALLBACK_HEADER_BYTES {
            return None;
        }
        buffer.extend_from_slice(&chunk[..n]);
        if buffer.windows(4).any(|w| w == b"\r\n\r\n") {
            return Some(buffer);
        }
    }
}

/// Which channel delivered the winning callback for an [`AttemptGate`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaimSource {
    Loopback,
    BrokerOrPaste,
}

/// Result of attempting to claim single-use ownership of an in-flight OAuth attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaimResult {
    Claimed,
    AlreadyClaimed { winner: ClaimSource },
    Cancelled,
    Expired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GateState {
    Pending,
    Claimed(ClaimSource),
    Cancelled,
    Expired,
}

/// Single-use atomic gate racing a loopback listener against a brokered or pasted `callback_url`.
#[derive(Clone)]
pub struct AttemptGate {
    inner: Arc<Mutex<GateState>>,
    cancel: Arc<AtomicBool>,
    created_at_unix: u64,
    ttl_secs: u64,
}

impl AttemptGate {
    pub fn new(created_at_unix: u64, ttl_secs: u64) -> Self {
        Self {
            inner: Arc::new(Mutex::new(GateState::Pending)),
            cancel: Arc::new(AtomicBool::new(false)),
            created_at_unix,
            ttl_secs,
        }
    }

    /// Cancellation flag shared with any loopback listener serving this attempt.
    pub fn cancel_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.cancel)
    }

    pub fn is_expired(&self, now_unix: u64) -> bool {
        now_unix.saturating_sub(self.created_at_unix) >= self.ttl_secs
    }

    /// Atomically claim the attempt for `source`. At most one caller ever observes `Claimed`.
    pub fn try_claim(&self, source: ClaimSource, now_unix: u64) -> ClaimResult {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        match *guard {
            GateState::Pending => {
                if self.is_expired(now_unix) {
                    *guard = GateState::Expired;
                    self.cancel.store(true, Ordering::Release);
                    ClaimResult::Expired
                } else {
                    *guard = GateState::Claimed(source);
                    self.cancel.store(true, Ordering::Release);
                    ClaimResult::Claimed
                }
            }
            GateState::Claimed(winner) => ClaimResult::AlreadyClaimed { winner },
            GateState::Cancelled => ClaimResult::Cancelled,
            GateState::Expired => ClaimResult::Expired,
        }
    }

    /// Cancel the attempt if still pending, waking any loopback listener.
    pub fn cancel(&self) {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if *guard == GateState::Pending {
            *guard = GateState::Cancelled;
        }
        self.cancel.store(true, Ordering::Release);
    }

    pub fn is_claimed(&self) -> bool {
        matches!(
            *self.inner.lock().unwrap_or_else(|e| e.into_inner()),
            GateState::Claimed(_)
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bind_spec_validates_path_and_port() {
        assert!(
            LoopbackBindSpec::ephemeral(LoopbackHost::Ipv4Loopback, "/auth/callback")
                .validate()
                .is_ok()
        );
        assert!(
            LoopbackBindSpec::fixed(LoopbackHost::Localhost, "/callback", 8765)
                .validate()
                .is_ok()
        );

        // Privileged port rejected.
        assert_eq!(
            LoopbackBindSpec::fixed(LoopbackHost::Ipv4Loopback, "/callback", 80).validate(),
            Err(CallbackError::InvalidBindSpec)
        );
        // Bad paths rejected.
        for bad in ["", "no-leading-slash", "/a/../b", "/a?b=1", "/a#b", "/a b"] {
            assert_eq!(
                LoopbackBindSpec::ephemeral(LoopbackHost::Ipv4Loopback, bad).validate(),
                Err(CallbackError::InvalidBindSpec),
                "expected reject for {bad:?}"
            );
        }
    }

    #[test]
    fn parse_callback_url_validates_redirect_duplicates_state_and_rfc9207_iss() {
        let policy =
            CallbackPolicy::new("expected-state-42", "http://127.0.0.1:54321/auth/callback")
                .with_expected_issuer("https://auth.example.com", true);

        // Valid callback with matching iss.
        let ok = parse_callback_url(
            "http://127.0.0.1:54321/auth/callback?code=abc123&state=expected-state-42&iss=https%3A%2F%2Fauth.example.com",
            &policy,
        );
        match ok {
            CallbackOutcome::Accepted(cb) => {
                assert_eq!(cb.code, "abc123");
                assert_eq!(cb.state, "expected-state-42");
                assert_eq!(cb.issuer.as_deref(), Some("https://auth.example.com"));
            }
            other => panic!("expected accepted, got {other:?}"),
        }

        // Wrong state -> Ignored (does not consume attempt).
        assert_eq!(
            parse_callback_url(
                "http://127.0.0.1:54321/auth/callback?code=abc123&state=wrong&iss=https%3A%2F%2Fauth.example.com",
                &policy,
            ),
            CallbackOutcome::Ignored(IgnoredReason::StateMismatch)
        );

        // Duplicate state -> Ignored.
        assert_eq!(
            parse_callback_url(
                "http://127.0.0.1:54321/auth/callback?code=abc123&state=expected-state-42&state=other",
                &policy,
            ),
            CallbackOutcome::Ignored(IgnoredReason::DuplicateParameter)
        );

        // Wrong host -> Ignored.
        assert_eq!(
            parse_callback_url(
                "http://localhost:54321/auth/callback?code=abc123&state=expected-state-42&iss=https%3A%2F%2Fauth.example.com",
                &policy,
            ),
            CallbackOutcome::Ignored(IgnoredReason::RedirectUriMismatch)
        );

        // Missing required iss with matching state -> InvalidForAttempt(MissingIssuer).
        assert_eq!(
            parse_callback_url(
                "http://127.0.0.1:54321/auth/callback?code=abc123&state=expected-state-42",
                &policy,
            ),
            CallbackOutcome::InvalidForAttempt(CallbackError::MissingIssuer)
        );

        // Wrong iss with matching state -> InvalidForAttempt(IssuerMismatch).
        assert_eq!(
            parse_callback_url(
                "http://127.0.0.1:54321/auth/callback?code=abc123&state=expected-state-42&iss=https%3A%2F%2Fevil.example",
                &policy,
            ),
            CallbackOutcome::InvalidForAttempt(CallbackError::IssuerMismatch)
        );

        // Provider error with matching state & iss -> ProviderError.
        assert_eq!(
            parse_callback_url(
                "http://127.0.0.1:54321/auth/callback?error=access_denied&error_description=nope&state=expected-state-42&iss=https%3A%2F%2Fauth.example.com",
                &policy,
            ),
            CallbackOutcome::ProviderError {
                error: "access_denied".to_owned(),
                error_description: Some("nope".to_owned()),
            }
        );
    }

    #[test]
    fn parse_callback_url_accepts_custom_schemes_and_strips_fragments() {
        let mut policy = CallbackPolicy::new("st-1", "solstone://oauth/callback");
        policy
            .accepted_redirect_uris
            .push("app.solstone:/callback".to_owned());

        assert!(matches!(
            parse_callback_url("solstone://oauth/callback?code=c1&state=st-1#_=_", &policy),
            CallbackOutcome::Accepted(cb) if cb.code == "c1"
        ));
        assert!(matches!(
            parse_callback_url("app.solstone:/callback?code=c2&state=st-1", &policy),
            CallbackOutcome::Accepted(cb) if cb.code == "c2"
        ));
        assert_eq!(
            parse_callback_url("solstone://wrong/callback?code=c1&state=st-1", &policy),
            CallbackOutcome::Ignored(IgnoredReason::RedirectUriMismatch)
        );
    }

    #[test]
    fn attempt_gate_is_single_use_and_signals_cancel_flag() {
        let gate = AttemptGate::new(1000, 600);
        let flag = gate.cancel_flag();
        assert!(!flag.load(Ordering::Acquire));

        assert_eq!(
            gate.try_claim(ClaimSource::BrokerOrPaste, 1050),
            ClaimResult::Claimed
        );
        assert!(flag.load(Ordering::Acquire));
        assert_eq!(
            gate.try_claim(ClaimSource::Loopback, 1051),
            ClaimResult::AlreadyClaimed {
                winner: ClaimSource::BrokerOrPaste
            }
        );

        let expired_gate = AttemptGate::new(1000, 600);
        assert_eq!(
            expired_gate.try_claim(ClaimSource::Loopback, 1600),
            ClaimResult::Expired
        );
    }
}
