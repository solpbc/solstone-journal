// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fmt;
use std::time::Duration;

pub const DEFAULT_RELAY_TIMEOUT: Duration = Duration::from_secs(60);
pub const DEFAULT_WEB_PUSH_TIMEOUT: Duration = Duration::from_secs(30);

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

    fn post_bytes(
        &self,
        url: &str,
        headers: &[(&str, &str)],
        body: &[u8],
    ) -> Result<u16, RelayFault>;
}

pub(crate) struct UreqRelay {
    relay_agent: ureq::Agent,
    web_push_agent: ureq::Agent,
}

impl Default for UreqRelay {
    fn default() -> Self {
        Self::new(DEFAULT_RELAY_TIMEOUT, DEFAULT_WEB_PUSH_TIMEOUT)
    }
}

impl UreqRelay {
    pub(crate) fn new(relay_timeout: Duration, web_push_timeout: Duration) -> Self {
        let relay_config = ureq::config::Config::builder()
            .timeout_global(Some(relay_timeout))
            .timeout_connect(Some(relay_timeout))
            .timeout_recv_response(Some(relay_timeout))
            .timeout_send_body(Some(relay_timeout))
            .timeout_recv_body(Some(relay_timeout))
            .http_status_as_error(false)
            .build();

        let web_push_config = ureq::config::Config::builder()
            .timeout_global(Some(web_push_timeout))
            .max_redirects(0)
            .user_agent("")
            .http_status_as_error(false)
            .build();

        Self {
            relay_agent: relay_config.into(),
            web_push_agent: web_push_config.into(),
        }
    }

    #[cfg(test)]
    pub(crate) fn web_push_agent_config(&self) -> &ureq::config::Config {
        self.web_push_agent.config()
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
            .relay_agent
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

    fn post_bytes(
        &self,
        url: &str,
        headers: &[(&str, &str)],
        body: &[u8],
    ) -> Result<u16, RelayFault> {
        let mut request = self.web_push_agent.post(url);
        for (name, val) in headers {
            request = request.header(*name, *val);
        }
        let response = request.send(body).map_err(map_ureq_error)?;
        Ok(response.status().as_u16())
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
        let relay = UreqRelay::new(Duration::from_millis(100), DEFAULT_WEB_PUSH_TIMEOUT);
        let url = format!("http://127.0.0.1:{port}/");
        let result = relay.post_json(&url, b"{}", None);
        assert_eq!(result, Err(RelayFault::Timeout));
    }

    #[test]
    fn ureq_relay_connect_failure_behavior() {
        let listener = bind_local().unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let relay = UreqRelay::new(Duration::from_secs(5), DEFAULT_WEB_PUSH_TIMEOUT);
        let url = format!("http://127.0.0.1:{port}/");
        let result = relay.post_json(&url, b"{}", None);
        assert_eq!(result, Err(RelayFault::Connect));
    }

    #[test]
    fn ureq_relay_default_timeout_is_60_seconds() {
        assert_eq!(DEFAULT_RELAY_TIMEOUT, Duration::from_secs(60));
    }

    #[test]
    fn ureq_relay_web_push_agent_config() {
        let relay = UreqRelay::default();
        let config = relay.web_push_agent_config();
        assert_eq!(config.timeouts().global, Some(DEFAULT_WEB_PUSH_TIMEOUT));
        assert_eq!(config.max_redirects(), 0);
        assert!(!config.http_status_as_error());
    }

    #[test]
    fn post_bytes_request_headers_and_no_user_agent() {
        use std::io::{Read, Write};
        use std::thread;

        let listener = bind_local().unwrap();
        let port = listener.local_addr().unwrap().port();

        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buf = [0u8; 4096];
            let n = stream.read(&mut buf).unwrap();
            let req = String::from_utf8_lossy(&buf[..n]);

            // Ensure exact headers and no user-agent
            let lines: Vec<&str> = req.lines().collect();
            let content_type_count = lines
                .iter()
                .filter(|l| l.to_ascii_lowercase().starts_with("content-type:"))
                .count();
            assert_eq!(content_type_count, 1, "req was:\n{req}");

            assert!(
                lines
                    .iter()
                    .any(|l| l.eq_ignore_ascii_case("content-type: application/octet-stream")),
                "missing content-type in:\n{req}"
            );
            assert!(
                lines
                    .iter()
                    .any(|l| l.eq_ignore_ascii_case("content-encoding: aes128gcm")),
                "missing content-encoding in:\n{req}"
            );
            assert!(
                lines.iter().any(|l| l.eq_ignore_ascii_case("ttl: 86400")),
                "missing ttl in:\n{req}"
            );
            assert!(
                lines
                    .iter()
                    .any(|l| l.eq_ignore_ascii_case("urgency: high")),
                "missing urgency in:\n{req}"
            );
            assert!(
                lines
                    .iter()
                    .any(|l| l.eq_ignore_ascii_case("authorization: vapid t=jwt_val, k=key_val")),
                "missing authorization in:\n{req}"
            );
            assert!(
                !lines
                    .iter()
                    .any(|l| l.to_ascii_lowercase().starts_with("user-agent:")),
                "must not contain user-agent in:\n{req}"
            );

            stream
                .write_all(b"HTTP/1.1 201 Created\r\nContent-Length: 0\r\n\r\n")
                .unwrap();
        });

        let relay = UreqRelay::new(DEFAULT_RELAY_TIMEOUT, Duration::from_secs(5));
        let url = format!("http://127.0.0.1:{port}/push");
        let headers = [
            ("Content-Encoding", "aes128gcm"),
            ("Content-Type", "application/octet-stream"),
            ("TTL", "86400"),
            ("Urgency", "high"),
            ("Authorization", "vapid t=jwt_val, k=key_val"),
        ];

        let result = relay.post_bytes(&url, &headers, b"encrypted_payload");
        assert_eq!(result, Ok(201));
        handle.join().unwrap();
    }

    #[test]
    fn post_bytes_status_404_and_410_returned_as_statuses() {
        use std::io::{Read, Write};
        use std::thread;

        for status in [404, 410] {
            let listener = bind_local().unwrap();
            let port = listener.local_addr().unwrap().port();

            let handle = thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                let mut buf = [0u8; 1024];
                let _ = stream.read(&mut buf).unwrap();
                let response = format!("HTTP/1.1 {status} Error\r\nContent-Length: 0\r\n\r\n");
                stream.write_all(response.as_bytes()).unwrap();
            });

            let relay = UreqRelay::new(DEFAULT_RELAY_TIMEOUT, Duration::from_secs(5));
            let url = format!("http://127.0.0.1:{port}/push");
            let result = relay.post_bytes(&url, &[], b"data");
            assert_eq!(result, Ok(status));
            handle.join().unwrap();
        }
    }

    #[test]
    fn post_bytes_301_with_location_returns_301_without_redirect() {
        use std::io::{Read, Write};
        use std::thread;

        let listener_target = bind_local().unwrap();
        listener_target.set_nonblocking(true).unwrap();
        let target_port = listener_target.local_addr().unwrap().port();

        let listener1 = bind_local().unwrap();
        let port1 = listener1.local_addr().unwrap().port();

        let handle1 = thread::spawn(move || {
            let (mut stream, _) = listener1.accept().unwrap();
            let mut buf = [0u8; 1024];
            let _ = stream.read(&mut buf).unwrap();
            let resp = format!(
                "HTTP/1.1 301 Moved Permanently\r\nLocation: http://127.0.0.1:{target_port}/target\r\nContent-Length: 0\r\n\r\n"
            );
            stream.write_all(resp.as_bytes()).unwrap();
        });

        let relay = UreqRelay::new(DEFAULT_RELAY_TIMEOUT, Duration::from_secs(5));
        let url = format!("http://127.0.0.1:{port1}/source");
        let result = relay.post_bytes(&url, &[], b"data");
        assert_eq!(result, Ok(301));
        handle1.join().unwrap();

        // Target listener accepts nothing
        match listener_target.accept() {
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
            other => panic!("expected WouldBlock on target listener, got {other:?}"),
        }
    }

    #[test]
    fn post_bytes_201_huge_or_stalled_body_returns_ok_before_finish() {
        use std::io::{Read, Write};
        use std::thread;

        let listener = bind_local().unwrap();
        let port = listener.local_addr().unwrap().port();

        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buf = [0u8; 1024];
            let _ = stream.read(&mut buf).unwrap();
            // Declare huge content length but only write a few bytes of body
            stream
                .write_all(b"HTTP/1.1 201 Created\r\nContent-Length: 1000000\r\n\r\npartial")
                .unwrap();
        });

        let relay = UreqRelay::new(DEFAULT_RELAY_TIMEOUT, Duration::from_secs(5));
        let url = format!("http://127.0.0.1:{port}/push");
        let result = relay.post_bytes(&url, &[], b"data");
        assert_eq!(result, Ok(201));
        handle.join().unwrap();
    }

    #[test]
    fn post_bytes_timeout_under_short_web_push_timeout() {
        use std::thread;

        let listener = bind_local().unwrap();
        let port = listener.local_addr().unwrap().port();

        let handle = thread::spawn(move || {
            let (_stream, _) = listener.accept().unwrap();
            // Accept and never reply
            thread::sleep(Duration::from_millis(500));
        });

        let relay = UreqRelay::new(DEFAULT_RELAY_TIMEOUT, Duration::from_millis(100));
        let url = format!("http://127.0.0.1:{port}/push");
        let result = relay.post_bytes(&url, &[], b"data");
        assert_eq!(result, Err(RelayFault::Timeout));
        handle.join().unwrap();
    }
}
