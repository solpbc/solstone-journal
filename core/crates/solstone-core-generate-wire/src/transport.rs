// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Shared HTTP mechanics; provider protocols and retry policy stay with callers.

use std::time::Duration;

use serde_json::Value;
use solstone_core_local::HttpResponse;

use crate::endpoint::EndpointTransportError;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(120);
/// The most characters of a provider error body kept as refusal detail.
pub(crate) const PROVIDER_DETAIL_CAP_CHARS: usize = 512;

/// The caller's timeout, else the lane default plus time for any thinking room.
pub(crate) fn request_timeout(timeout_s: Option<f64>, thinking_room: u64) -> Duration {
    timeout_s
        .filter(|seconds| seconds.is_finite() && *seconds > 0.0)
        .map(Duration::from_secs_f64)
        .unwrap_or(DEFAULT_TIMEOUT + crate::thinking::thinking_time(thinking_room))
}

pub(crate) fn agent(timeout: Duration) -> ureq::Agent {
    let config = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_connect(Some(timeout))
        .timeout_recv_response(Some(timeout))
        .timeout_recv_body(Some(timeout))
        .timeout_global(Some(timeout))
        .build();
    ureq::Agent::new_with_config(config)
}

pub(crate) fn post_json(
    base_url: &str,
    path: &str,
    body: &Value,
    headers: &[(&str, &str)],
    timeout: Duration,
) -> Result<HttpResponse, EndpointTransportError> {
    let mut request = agent(timeout)
        .post(&format!("{base_url}{path}"))
        .header("Content-Type", "application/json");
    for &(name, value) in headers {
        request = request.header(name, value);
    }
    let response = request
        .send(serde_json::to_string(body).expect("JSON value serializes"))
        .map_err(classify_ureq_error)?;
    let status = response.status().as_u16();
    let body = response
        .into_body()
        .read_to_string()
        .map_err(|error| classify_body_error(status, error))?;
    Ok(HttpResponse { status, body })
}

/// A provider that sent a success status has done the work and may bill for it,
/// so a reply lost after that is no outage: it counts against the request, and
/// a repeat is set aside rather than paid for again without end.
fn classify_body_error(status: u16, error: ureq::Error) -> EndpointTransportError {
    if (200..300).contains(&status) {
        EndpointTransportError::Other
    } else {
        classify_ureq_error(error)
    }
}

pub(crate) fn classify_ureq_error(error: ureq::Error) -> EndpointTransportError {
    match error {
        ureq::Error::HostNotFound | ureq::Error::ConnectionFailed | ureq::Error::Io(_) => {
            EndpointTransportError::Connection
        }
        ureq::Error::Timeout(ureq::Timeout::Resolve | ureq::Timeout::Connect) => {
            EndpointTransportError::Connection
        }
        ureq::Error::Timeout(_) => EndpointTransportError::Capacity,
        _ => EndpointTransportError::Other,
    }
}

pub(crate) fn capture_provider_detail(body: &str, secret: &str) -> Option<String> {
    if body.is_empty() {
        return None;
    }
    let scrubbed = if secret.is_empty() {
        body.to_owned()
    } else {
        body.replace(secret, "[redacted]")
    };
    Some(scrubbed.chars().take(PROVIDER_DETAIL_CAP_CHARS).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_preserves_status_as_data_and_all_transport_timeouts() {
        let timeout = Duration::from_millis(250);
        let agent = agent(timeout);
        assert!(!agent.config().http_status_as_error());
        let timeouts = agent.config().timeouts();
        assert_eq!(timeouts.connect, Some(timeout));
        assert_eq!(timeouts.recv_response, Some(timeout));
        assert_eq!(timeouts.recv_body, Some(timeout));
        assert_eq!(timeouts.global, Some(timeout));
    }

    #[test]
    fn invalid_timeouts_use_the_default_and_explicit_timeouts_ignore_thinking_room() {
        for timeout in [
            None,
            Some(0.0),
            Some(-1.0),
            Some(f64::NAN),
            Some(f64::INFINITY),
        ] {
            assert_eq!(request_timeout(timeout, 0), Duration::from_secs(120));
            assert_eq!(
                request_timeout(timeout, 8192),
                Duration::from_secs(120) + crate::thinking::thinking_time(8192)
            );
        }
        assert_eq!(
            request_timeout(Some(0.25), 8192),
            Duration::from_millis(250)
        );
    }

    #[test]
    fn transport_errors_distinguish_connection_from_capacity() {
        use ureq::Timeout;
        for error in [
            ureq::Error::HostNotFound,
            ureq::Error::ConnectionFailed,
            ureq::Error::Io(std::io::Error::from(std::io::ErrorKind::ConnectionReset)),
            ureq::Error::Timeout(Timeout::Resolve),
            ureq::Error::Timeout(Timeout::Connect),
        ] {
            assert_eq!(
                classify_ureq_error(error),
                EndpointTransportError::Connection
            );
        }
        for timeout in [
            Timeout::Global,
            Timeout::PerCall,
            Timeout::SendRequest,
            Timeout::SendBody,
            Timeout::RecvResponse,
            Timeout::RecvBody,
        ] {
            assert_eq!(
                classify_ureq_error(ureq::Error::Timeout(timeout)),
                EndpointTransportError::Capacity
            );
        }
        assert_eq!(
            classify_ureq_error(ureq::Error::StatusCode(429)),
            EndpointTransportError::Other
        );
    }

    #[test]
    fn a_reply_lost_after_a_success_status_is_not_an_outage() {
        let reset = || ureq::Error::Io(std::io::Error::from(std::io::ErrorKind::ConnectionReset));
        assert_eq!(
            classify_body_error(200, reset()),
            EndpointTransportError::Other
        );
        assert_eq!(
            classify_body_error(200, ureq::Error::Timeout(ureq::Timeout::RecvBody)),
            EndpointTransportError::Other
        );
        assert_eq!(
            classify_body_error(503, reset()),
            EndpointTransportError::Connection
        );
    }

    #[test]
    fn detail_redacts_whole_secrets_before_capping_unicode_characters() {
        assert_eq!(capture_provider_detail("", "key"), None);
        assert_eq!(capture_provider_detail("é🙂", ""), Some("é🙂".into()));
        assert_eq!(
            capture_provider_detail("key,key", "key"),
            Some("[redacted],[redacted]".into())
        );
        let prefix = "é".repeat(PROVIDER_DETAIL_CAP_CHARS - 3);
        let detail = capture_provider_detail(&format!("{prefix}secret🙂"), "secret").unwrap();
        assert_eq!(detail, format!("{prefix}[re"));
        assert_eq!(detail.chars().count(), PROVIDER_DETAIL_CAP_CHARS);
        let detail =
            capture_provider_detail(&"🙂".repeat(PROVIDER_DETAIL_CAP_CHARS + 1), "").unwrap();
        assert_eq!(detail.chars().count(), PROVIDER_DETAIL_CAP_CHARS);
    }
}
