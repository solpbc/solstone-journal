// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Optional RFC 8628 OAuth 2.0 Device Authorization Grant client primitives.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::client::{TokenResponse, parse_token_response_body};
use crate::ssrf::{
    DEFAULT_REQUEST_TIMEOUT, HttpRequest, OAuthHttpClient, SsrfFetchError, validate_https_url,
};
use crate::{encode_form, extract_oauth_error_code};

pub const DEVICE_CODE_GRANT_TYPE: &str = "urn:ietf:params:oauth:grant-type:device_code";
pub const DEFAULT_DEVICE_POLL_INTERVAL_SECS: u64 = 5;
pub const SLOW_DOWN_INCREMENT_SECS: u64 = 5;

/// Response from the RFC 8628 device authorization endpoint.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceAuthorization {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verification_uri_complete: Option<String>,
    pub expires_in: u64,
    pub interval: u64,
}

impl fmt::Debug for DeviceAuthorization {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DeviceAuthorization")
            .field("user_code", &self.user_code)
            .field("verification_uri", &self.verification_uri)
            .field("expires_in", &self.expires_in)
            .field("interval", &self.interval)
            .finish_non_exhaustive()
    }
}

/// Outcome of one RFC 8628 token-endpoint poll attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DevicePollStep {
    Authorized(TokenResponse),
    Pending,
    SlowDown,
    AccessDenied,
    Expired,
}

/// Errors from RFC 8628 Device Authorization Grant operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceCodeError {
    InvalidEndpoint,
    Fetch(SsrfFetchError),
    Rejected { status: u16, error: Option<String> },
    MalformedResponse(&'static str),
    AccessDenied,
    Expired,
    Cancelled,
}

impl fmt::Display for DeviceCodeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidEndpoint => {
                formatter.write_str("invalid HTTPS device authorization endpoint")
            }
            Self::Fetch(err) => err.fmt(formatter),
            Self::Rejected { status, error } => match error {
                Some(code) => write!(
                    formatter,
                    "device authorization rejected ({status}: {code})"
                ),
                None => write!(formatter, "device authorization rejected ({status})"),
            },
            Self::MalformedResponse(field) => {
                write!(
                    formatter,
                    "malformed device authorization response field: {field}"
                )
            }
            Self::AccessDenied => formatter.write_str("device authorization denied by user"),
            Self::Expired => formatter.write_str("device authorization code expired"),
            Self::Cancelled => formatter.write_str("device authorization polling cancelled"),
        }
    }
}

impl std::error::Error for DeviceCodeError {}

/// Initiate an RFC 8628 device authorization flow.
pub fn start_device_authorization(
    http: &dyn OAuthHttpClient,
    device_authorization_endpoint: &str,
    client_id: &str,
    scope: Option<&str>,
    resource: Option<&str>,
) -> Result<DeviceAuthorization, DeviceCodeError> {
    validate_https_url(device_authorization_endpoint)
        .map_err(|_| DeviceCodeError::InvalidEndpoint)?;

    let mut form = BTreeMap::new();
    form.insert("client_id".to_owned(), client_id.to_owned());
    if let Some(scope) = scope.filter(|s| !s.is_empty()) {
        form.insert("scope".to_owned(), scope.to_owned());
    }
    if let Some(resource) = resource.filter(|r| !r.is_empty()) {
        form.insert("resource".to_owned(), resource.to_owned());
    }

    let req = HttpRequest::post(
        device_authorization_endpoint,
        "application/x-www-form-urlencoded",
        encode_form(&form).into_bytes(),
    )
    .with_timeout(DEFAULT_REQUEST_TIMEOUT);
    let resp = http.send(&req).map_err(DeviceCodeError::Fetch)?;
    if resp.status != 200 {
        return Err(DeviceCodeError::Rejected {
            status: resp.status,
            error: extract_oauth_error_code(&resp.body),
        });
    }

    let json: Value = serde_json::from_slice(&resp.body)
        .map_err(|_| DeviceCodeError::MalformedResponse("json"))?;
    let device_code = json
        .get("device_code")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or(DeviceCodeError::MalformedResponse("device_code"))?
        .to_owned();
    let user_code = json
        .get("user_code")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or(DeviceCodeError::MalformedResponse("user_code"))?
        .to_owned();
    let verification_uri = json
        .get("verification_uri")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or(DeviceCodeError::MalformedResponse("verification_uri"))?
        .to_owned();
    validate_https_url(&verification_uri)
        .map_err(|_| DeviceCodeError::MalformedResponse("verification_uri"))?;

    let verification_uri_complete = match json
        .get("verification_uri_complete")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
    {
        Some(uri) => {
            validate_https_url(uri)
                .map_err(|_| DeviceCodeError::MalformedResponse("verification_uri_complete"))?;
            Some(uri.to_owned())
        }
        None => None,
    };

    let expires_in = json
        .get("expires_in")
        .and_then(Value::as_u64)
        .filter(|v| *v > 0)
        .ok_or(DeviceCodeError::MalformedResponse("expires_in"))?;
    let interval = json
        .get("interval")
        .and_then(Value::as_u64)
        .filter(|v| *v > 0)
        .unwrap_or(DEFAULT_DEVICE_POLL_INTERVAL_SECS);

    Ok(DeviceAuthorization {
        device_code,
        user_code,
        verification_uri,
        verification_uri_complete,
        expires_in,
        interval,
    })
}

/// Perform one poll against `token_endpoint` for an active RFC 8628 `device_code`.
pub fn poll_device_token_once(
    http: &dyn OAuthHttpClient,
    token_endpoint: &str,
    client_id: &str,
    device_code: &str,
    resource: Option<&str>,
) -> Result<DevicePollStep, DeviceCodeError> {
    validate_https_url(token_endpoint).map_err(|_| DeviceCodeError::InvalidEndpoint)?;

    let mut form = BTreeMap::new();
    form.insert("grant_type".to_owned(), DEVICE_CODE_GRANT_TYPE.to_owned());
    form.insert("device_code".to_owned(), device_code.to_owned());
    form.insert("client_id".to_owned(), client_id.to_owned());
    if let Some(resource) = resource.filter(|r| !r.is_empty()) {
        form.insert("resource".to_owned(), resource.to_owned());
    }

    let req = HttpRequest::post(
        token_endpoint,
        "application/x-www-form-urlencoded",
        encode_form(&form).into_bytes(),
    )
    .with_timeout(DEFAULT_REQUEST_TIMEOUT);
    let resp = http.send(&req).map_err(DeviceCodeError::Fetch)?;

    if resp.status == 200 {
        let tokens = parse_token_response_body(&resp.body, None)
            .map_err(|_| DeviceCodeError::MalformedResponse("token"))?;
        return Ok(DevicePollStep::Authorized(tokens));
    }

    let code = extract_oauth_error_code(&resp.body);
    match code.as_deref() {
        Some("authorization_pending") => Ok(DevicePollStep::Pending),
        Some("slow_down") => Ok(DevicePollStep::SlowDown),
        Some("access_denied") => Ok(DevicePollStep::AccessDenied),
        Some("expired_token") => Ok(DevicePollStep::Expired),
        _ => Err(DeviceCodeError::Rejected {
            status: resp.status,
            error: code,
        }),
    }
}

/// Poll `token_endpoint` until authorized, denied, expired, or cancelled.
///
/// Honors RFC 8628 § 3.5 `slow_down` by increasing the polling interval by 5 seconds.
/// Takes injectable `now_secs` and `sleep` functions so unit tests run deterministically
/// with zero wall-clock wait.
#[allow(clippy::too_many_arguments)]
pub fn poll_device_token_until_complete<FNow, FSleep>(
    http: &dyn OAuthHttpClient,
    token_endpoint: &str,
    client_id: &str,
    auth: &DeviceAuthorization,
    resource: Option<&str>,
    cancel: &AtomicBool,
    mut now_secs: FNow,
    mut sleep: FSleep,
) -> Result<TokenResponse, DeviceCodeError>
where
    FNow: FnMut() -> u64,
    FSleep: FnMut(Duration),
{
    let deadline_unix = now_secs().saturating_add(auth.expires_in);
    let mut interval_secs = auth.interval.max(1);

    loop {
        if cancel.load(Ordering::Acquire) {
            return Err(DeviceCodeError::Cancelled);
        }
        if now_secs() >= deadline_unix {
            return Err(DeviceCodeError::Expired);
        }

        match poll_device_token_once(http, token_endpoint, client_id, &auth.device_code, resource)?
        {
            DevicePollStep::Authorized(tokens) => return Ok(tokens),
            DevicePollStep::Pending => {}
            DevicePollStep::SlowDown => {
                interval_secs = interval_secs.saturating_add(SLOW_DOWN_INCREMENT_SECS);
            }
            DevicePollStep::AccessDenied => return Err(DeviceCodeError::AccessDenied),
            DevicePollStep::Expired => return Err(DeviceCodeError::Expired),
        }

        if cancel.load(Ordering::Acquire) {
            return Err(DeviceCodeError::Cancelled);
        }
        sleep(Duration::from_secs(interval_secs));
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::collections::VecDeque;
    use std::sync::Mutex;

    use crate::ssrf::HttpResponse;

    use super::*;

    struct ScriptedHttp {
        responses: Mutex<VecDeque<HttpResponse>>,
    }

    impl ScriptedHttp {
        fn new(responses: Vec<HttpResponse>) -> Self {
            Self {
                responses: Mutex::new(VecDeque::from(responses)),
            }
        }
    }

    impl OAuthHttpClient for ScriptedHttp {
        fn send(&self, _request: &HttpRequest) -> Result<HttpResponse, SsrfFetchError> {
            self.responses
                .lock()
                .unwrap()
                .pop_front()
                .ok_or(SsrfFetchError::Connect)
        }
    }

    #[test]
    fn device_code_handles_pending_slow_down_backoff_and_authorization() {
        let http = ScriptedHttp::new(vec![
            HttpResponse {
                status: 200,
                headers: Vec::new(),
                body: br#"{"device_code":"dev-1","user_code":"WXYZ-1234","verification_uri":"https://auth.example.com/device","verification_uri_complete":"https://auth.example.com/device?user_code=WXYZ-1234","expires_in":600,"interval":5}"#.to_vec(),
            },
            HttpResponse {
                status: 400,
                headers: Vec::new(),
                body: br#"{"error":"authorization_pending"}"#.to_vec(),
            },
            HttpResponse {
                status: 400,
                headers: Vec::new(),
                body: br#"{"error":"slow_down"}"#.to_vec(),
            },
            HttpResponse {
                status: 200,
                headers: Vec::new(),
                body: br#"{"access_token":"dev-at","refresh_token":"dev-rt","token_type":"Bearer","expires_in":3600}"#.to_vec(),
            },
        ]);

        let auth = start_device_authorization(
            &http,
            "https://auth.example.com/oauth2/device_authorization",
            "client-1",
            Some("read"),
            None,
        )
        .expect("start device auth");
        assert_eq!(auth.user_code, "WXYZ-1234");
        assert_eq!(auth.interval, 5);

        let clock = Cell::new(1000_u64);
        let mut sleeps = Vec::new();
        let cancel = AtomicBool::new(false);
        let tokens = poll_device_token_until_complete(
            &http,
            "https://auth.example.com/oauth2/token",
            "client-1",
            &auth,
            None,
            &cancel,
            || clock.get(),
            |d| {
                sleeps.push(d.as_secs());
                clock.set(clock.get() + d.as_secs());
            },
        )
        .expect("device polling completes");

        assert_eq!(tokens.access_token, "dev-at");
        assert_eq!(tokens.refresh_token.as_deref(), Some("dev-rt"));
        // First wait 5s (Pending), second wait 10s (SlowDown adds +5s per RFC 8628 § 3.5).
        assert_eq!(sleeps, vec![5, 10]);
    }

    #[test]
    fn device_code_handles_denied_and_cancelled() {
        let http = ScriptedHttp::new(vec![HttpResponse {
            status: 400,
            headers: Vec::new(),
            body: br#"{"error":"access_denied"}"#.to_vec(),
        }]);
        let auth = DeviceAuthorization {
            device_code: "dev-2".to_owned(),
            user_code: "ABCD".to_owned(),
            verification_uri: "https://auth.example.com/device".to_owned(),
            verification_uri_complete: None,
            expires_in: 60,
            interval: 5,
        };
        let cancel = AtomicBool::new(false);
        assert_eq!(
            poll_device_token_until_complete(
                &http,
                "https://auth.example.com/oauth2/token",
                "client-1",
                &auth,
                None,
                &cancel,
                || 1000,
                |_| {},
            ),
            Err(DeviceCodeError::AccessDenied)
        );

        cancel.store(true, Ordering::Release);
        assert_eq!(
            poll_device_token_until_complete(
                &http,
                "https://auth.example.com/oauth2/token",
                "client-1",
                &auth,
                None,
                &cancel,
                || 1000,
                |_| {},
            ),
            Err(DeviceCodeError::Cancelled)
        );
    }
}
