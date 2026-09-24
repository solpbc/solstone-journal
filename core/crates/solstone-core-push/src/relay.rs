// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fmt;
use std::time::Duration;

pub const DEFAULT_RELAY_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RelayReply {
    pub status: u16,
    pub body: Vec<u8>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RelayFault {
    Connect,
    Timeout,
    Transport,
}

impl fmt::Display for RelayFault {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Connect => formatter.write_str("could not connect to push relay origin"),
            Self::Timeout => formatter.write_str("push relay request timed out"),
            Self::Transport => formatter.write_str("push relay transport failure"),
        }
    }
}

impl std::error::Error for RelayFault {}

pub(crate) trait RelayTransport: Send + Sync {
    fn post_json(
        &self,
        url: &str,
        body: &[u8],
        bearer_token: Option<&str>,
    ) -> Result<RelayReply, RelayFault>;
}

pub(crate) struct UreqRelay {
    agent: ureq::Agent,
}

impl Default for UreqRelay {
    fn default() -> Self {
        Self::new(DEFAULT_RELAY_TIMEOUT)
    }
}

impl UreqRelay {
    pub(crate) fn new(timeout: Duration) -> Self {
        let config = ureq::config::Config::builder()
            .timeout_global(Some(timeout))
            .timeout_connect(Some(timeout))
            .timeout_recv_response(Some(timeout))
            .timeout_send_body(Some(timeout))
            .timeout_recv_body(Some(timeout))
            .http_status_as_error(false)
            .build();
        Self {
            agent: config.into(),
        }
    }
}

impl RelayTransport for UreqRelay {
    fn post_json(
        &self,
        url: &str,
        body: &[u8],
        bearer_token: Option<&str>,
    ) -> Result<RelayReply, RelayFault> {
        let mut request = self
            .agent
            .post(url)
            .header("Content-Type", "application/json");
        if let Some(token) = bearer_token {
            request = request.header("Authorization", &format!("Bearer {token}"));
        }
        let response = request.send(body).map_err(map_ureq_error)?;

        let status = response.status().as_u16();
        let mut body_reader = response.into_body();
        let body_bytes = body_reader.read_to_vec().map_err(map_ureq_error)?;

        Ok(RelayReply {
            status,
            body: body_bytes,
        })
    }
}

fn map_ureq_error(error: ureq::Error) -> RelayFault {
    match error {
        ureq::Error::Timeout(_) => RelayFault::Timeout,
        ureq::Error::ConnectionFailed => RelayFault::Connect,
        ureq::Error::Io(io_err) => {
            if io_err.kind() == std::io::ErrorKind::ConnectionRefused
                || io_err.kind() == std::io::ErrorKind::ConnectionReset
                || io_err.kind() == std::io::ErrorKind::ConnectionAborted
                || io_err.kind() == std::io::ErrorKind::NotConnected
            {
                RelayFault::Connect
            } else if io_err.kind() == std::io::ErrorKind::TimedOut {
                RelayFault::Timeout
            } else {
                RelayFault::Transport
            }
        }
        _ => RelayFault::Transport,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bind_local() -> std::io::Result<std::net::TcpListener> {
        let bind_fn = std::net::TcpListener::bind;
        bind_fn("127.0.0.1:0")
    }

    #[test]
    fn ureq_relay_timeout_behavior() {
        let listener = bind_local().unwrap();
        let port = listener.local_addr().unwrap().port();
        let relay = UreqRelay::new(Duration::from_millis(100));
        let url = format!("http://127.0.0.1:{port}/");
        let result = relay.post_json(&url, b"{}", None);
        assert_eq!(result, Err(RelayFault::Timeout));
    }

    #[test]
    fn ureq_relay_connect_failure_behavior() {
        let listener = bind_local().unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let relay = UreqRelay::new(Duration::from_secs(5));
        let url = format!("http://127.0.0.1:{port}/");
        let result = relay.post_json(&url, b"{}", None);
        assert_eq!(result, Err(RelayFault::Connect));
    }

    #[test]
    fn ureq_relay_default_timeout_is_60_seconds() {
        assert_eq!(DEFAULT_RELAY_TIMEOUT, Duration::from_secs(60));
    }
}
