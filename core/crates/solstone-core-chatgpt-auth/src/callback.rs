// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Local loopback listener and callback parsing for ChatGPT OAuth flow.

use std::collections::HashMap;
use std::fmt;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::time::Duration;
use subtle::ConstantTimeEq;

use crate::credential::ClosedOutcome;

pub const SUCCESS_BODY: &str = "signed in. you can close this tab and go back to your journal";
pub const FAILURE_BODY: &str = "sign-in didn't finish. go back to your journal and try again";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallbackError {
    MissingCode,
    StateMismatch,
    Denied,
    CallbackInvalid,
    InvalidRequest,
    Timeout,
    Io(String),
}

impl fmt::Display for CallbackError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingCode => formatter.write_str("authorization code missing from callback"),
            Self::StateMismatch => formatter.write_str("OAuth state parameter mismatch"),
            Self::Denied => formatter.write_str("authorization was denied by the user"),
            Self::CallbackInvalid => formatter.write_str("invalid callback parameter"),
            Self::InvalidRequest => formatter.write_str("invalid callback request"),
            Self::Timeout => formatter.write_str("timed out waiting for browser callback"),
            Self::Io(msg) => write!(formatter, "I/O error in callback listener: {msg}"),
        }
    }
}

impl std::error::Error for CallbackError {}

pub struct CallbackRequest {
    pub code: String,
    pub client_id: Option<String>,
    pub stream: Option<TcpStream>,
}

pub fn bind_loopback_listener() -> Result<(TcpListener, String), std::io::Error> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    let redirect_uri = format!("http://127.0.0.1:{port}/auth/callback");
    Ok((listener, redirect_uri))
}

pub fn parse_query_pairs_refusing_duplicates(
    query: &str,
) -> Result<HashMap<String, String>, CallbackError> {
    let mut params = HashMap::new();
    for pair in query.split('&') {
        if pair.is_empty() {
            continue;
        }
        let (k, v) = match pair.split_once('=') {
            Some((key, val)) => (key, val),
            None => (pair, ""),
        };
        let decoded_k = solstone_core_auth_flow::percent_decode(k)
            .map_err(|_| CallbackError::InvalidRequest)?;
        let decoded_v = solstone_core_auth_flow::percent_decode(v)
            .map_err(|_| CallbackError::InvalidRequest)?;
        if params.contains_key(&decoded_k) {
            return Err(CallbackError::InvalidRequest);
        }
        params.insert(decoded_k, decoded_v);
    }
    Ok(params)
}

pub fn validate_callback_params(
    params: &HashMap<String, String>,
    expected_state: &str,
    expected_client_id: Option<&str>,
) -> Result<(String, Option<String>), Result<ClosedOutcome, CallbackError>> {
    let Some(state) = params.get("state") else {
        return Err(Err(CallbackError::StateMismatch));
    };

    let state_match: bool = state.as_bytes().ct_eq(expected_state.as_bytes()).into();
    if !state_match {
        return Err(Err(CallbackError::StateMismatch));
    }

    if let Some(err) = params.get("error") {
        if err == "access_denied" {
            return Err(Ok(ClosedOutcome::Denied));
        } else {
            return Err(Ok(ClosedOutcome::CallbackInvalid));
        }
    }

    let Some(code) = params.get("code") else {
        return Err(Ok(ClosedOutcome::CallbackInvalid));
    };

    if code.is_empty() {
        return Err(Ok(ClosedOutcome::CallbackInvalid));
    }

    let is_first_registration = match expected_client_id {
        None => true,
        Some(cid) => cid == "dynamic_agent_client",
    };

    let client_id = params.get("client_id").cloned();

    if is_first_registration {
        let Some(ref cid) = client_id else {
            return Err(Ok(ClosedOutcome::CallbackInvalid));
        };
        if cid.is_empty() || cid == "dynamic_agent_client" {
            return Err(Ok(ClosedOutcome::CallbackInvalid));
        }
    } else {
        let expected = expected_client_id.unwrap();
        if let Some(ref cid) = client_id
            && cid != expected
        {
            return Err(Ok(ClosedOutcome::CallbackInvalid));
        }
    }

    Ok((code.clone(), client_id))
}

pub fn parse_pasted_callback(
    input: &str,
    expected_state: &str,
    expected_client_id: Option<&str>,
) -> Result<(String, Option<String>), ClosedOutcome> {
    let mut trimmed = input.trim();
    if let Some(stripped) = trimmed.strip_prefix("http://") {
        trimmed = stripped;
    } else if let Some(stripped) = trimmed.strip_prefix("https://") {
        trimmed = stripped;
    }

    let (path_part, query_part) = match trimmed.split_once('?') {
        Some((path, query)) => (path, query),
        None => ("", trimmed),
    };

    if !path_part.is_empty() {
        let (host, path) = match path_part.split_once('/') {
            Some((h, p)) => (h, format!("/{p}")),
            None => (path_part, "/".to_string()),
        };
        let host_name = host.split_once(':').map(|(h, _)| h).unwrap_or(host);
        if host_name != "127.0.0.1" {
            return Err(ClosedOutcome::CallbackInvalid);
        }
        if path != "/auth/callback" {
            return Err(ClosedOutcome::CallbackInvalid);
        }
    }

    let params = parse_query_pairs_refusing_duplicates(query_part)
        .map_err(|_| ClosedOutcome::CallbackInvalid)?;

    match validate_callback_params(&params, expected_state, expected_client_id) {
        Ok(res) => Ok(res),
        Err(Ok(outcome)) => Err(outcome),
        Err(Err(_)) => Err(ClosedOutcome::CallbackInvalid),
    }
}

pub fn write_http_response(
    stream: &mut TcpStream,
    status_code: u16,
    status_text: &str,
    body: &str,
) -> std::io::Result<()> {
    let response = format!(
        "HTTP/1.1 {status_code} {status_text}\r\n\
         Content-Type: text/plain; charset=utf-8\r\n\
         Content-Length: {}\r\n\
         Connection: close\r\n\
         \r\n\
         {body}",
        body.len()
    );
    stream.write_all(response.as_bytes())?;
    stream.flush()
}

pub fn respond_to_callback_stream(stream: Option<&mut TcpStream>, outcome: ClosedOutcome) {
    if let Some(s) = stream {
        if outcome == ClosedOutcome::SignedIn {
            let _ = write_http_response(s, 200, "OK", SUCCESS_BODY);
        } else {
            let _ = write_http_response(s, 400, "Bad Request", FAILURE_BODY);
        }
    }
}

pub fn receive_browser_callback(
    listener: &TcpListener,
    expected_state: &str,
    expected_client_id: Option<&str>,
    timeout: Duration,
) -> Result<CallbackRequest, ClosedOutcome> {
    let port = listener.local_addr().map(|a| a.port()).unwrap_or_default();
    let expected_host = format!("127.0.0.1:{port}");

    let _ = listener.set_nonblocking(true);
    let start = std::time::Instant::now();

    loop {
        if start.elapsed() >= timeout {
            return Err(ClosedOutcome::Expired);
        }

        let mut stream = match listener.accept() {
            Ok((s, _)) => s,
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(50));
                continue;
            }
            Err(_) => return Err(ClosedOutcome::CallbackInvalid),
        };

        let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
        let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));
        let _ = stream.set_nonblocking(false);

        let mut buffer = [0u8; 8192];
        let n = match stream.read(&mut buffer) {
            Ok(n) if n > 0 => n,
            _ => continue,
        };
        let request = String::from_utf8_lossy(&buffer[..n]);

        let mut lines = request.lines();
        let first_line = lines.next().unwrap_or_default();
        let mut parts = first_line.split_whitespace();
        let method = parts.next().unwrap_or_default();
        let path_and_query = parts.next().unwrap_or_default();

        if method != "GET" {
            let _ = write_http_response(&mut stream, 400, "Bad Request", FAILURE_BODY);
            continue;
        }

        let mut host_header = None;
        for line in lines {
            if line.is_empty() {
                break;
            }
            if let Some((k, v)) = line.split_once(':')
                && k.trim().eq_ignore_ascii_case("host")
            {
                host_header = Some(v.trim().to_string());
            }
        }

        let Some(host) = host_header else {
            let _ = write_http_response(&mut stream, 404, "Not Found", "Not Found");
            continue; // Missing Host header -> 404, do not consume attempt
        };
        if host != expected_host {
            let _ = write_http_response(&mut stream, 404, "Not Found", "Not Found");
            continue; // Wrong Host header (e.g. localhost) -> 404, do not consume attempt
        }

        let (path, query) = match path_and_query.split_once('?') {
            Some((p, q)) => (p, q),
            None => (path_and_query, ""),
        };

        if path != "/auth/callback" {
            let _ = write_http_response(&mut stream, 404, "Not Found", "Not Found");
            continue; // Do not consume attempt
        }

        let params = match parse_query_pairs_refusing_duplicates(query) {
            Ok(p) => p,
            Err(_) => {
                let _ = write_http_response(&mut stream, 400, "Bad Request", FAILURE_BODY);
                continue; // Do not consume attempt on bad query structure
            }
        };

        match validate_callback_params(&params, expected_state, expected_client_id) {
            Ok((code, client_id)) => {
                return Ok(CallbackRequest {
                    code,
                    client_id,
                    stream: Some(stream),
                });
            }
            Err(Err(_)) => {
                // State mismatch or missing state -> HTTP 400, do not consume attempt
                let _ = write_http_response(&mut stream, 400, "Bad Request", FAILURE_BODY);
                continue;
            }
            Err(Ok(outcome)) => {
                let _ = write_http_response(&mut stream, 400, "Bad Request", FAILURE_BODY);
                return Err(outcome);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_pasted_valid_callback_url_first_registration() {
        let input = "http://127.0.0.1:5015/auth/callback?code=sample_auth_code&state=expected_state_123&client_id=client_app_1";
        let (code, cid) =
            parse_pasted_callback(input, "expected_state_123", None).expect("valid callback");
        assert_eq!(code, "sample_auth_code");
        assert_eq!(cid.as_deref(), Some("client_app_1"));
    }

    #[test]
    fn parse_pasted_first_registration_rejects_missing_or_dynamic_client_id() {
        let input1 =
            "http://127.0.0.1:5015/auth/callback?code=sample_auth_code&state=expected_state_123";
        let err1 = parse_pasted_callback(input1, "expected_state_123", None).unwrap_err();
        assert_eq!(err1, ClosedOutcome::CallbackInvalid);

        let input2 = "http://127.0.0.1:5015/auth/callback?code=sample_auth_code&state=expected_state_123&client_id=dynamic_agent_client";
        let err2 = parse_pasted_callback(input2, "expected_state_123", None).unwrap_err();
        assert_eq!(err2, ClosedOutcome::CallbackInvalid);
    }

    #[test]
    fn parse_pasted_valid_callback_url_reauth() {
        let input =
            "http://127.0.0.1:5015/auth/callback?code=sample_auth_code&state=expected_state_123";
        let (code, cid) = parse_pasted_callback(input, "expected_state_123", Some("client_app_1"))
            .expect("valid callback");
        assert_eq!(code, "sample_auth_code");
        assert!(cid.is_none());

        let input_matching = "http://127.0.0.1:5015/auth/callback?code=sample_auth_code&state=expected_state_123&client_id=client_app_1";
        let (code2, cid2) =
            parse_pasted_callback(input_matching, "expected_state_123", Some("client_app_1"))
                .expect("valid callback");
        assert_eq!(code2, "sample_auth_code");
        assert_eq!(cid2.as_deref(), Some("client_app_1"));

        let input_diff = "http://127.0.0.1:5015/auth/callback?code=sample_auth_code&state=expected_state_123&client_id=different_client";
        let err_diff =
            parse_pasted_callback(input_diff, "expected_state_123", Some("client_app_1"))
                .unwrap_err();
        assert_eq!(err_diff, ClosedOutcome::CallbackInvalid);
    }

    #[test]
    fn parse_pasted_refuses_duplicate_state() {
        let input = "127.0.0.1:5015/auth/callback?code=c1&state=s1&state=s2";
        let err = parse_pasted_callback(input, "s1", None).unwrap_err();
        assert_eq!(err, ClosedOutcome::CallbackInvalid);
    }

    #[test]
    fn parse_pasted_access_denied() {
        let input = "code=&error=access_denied&state=match_state";
        let err = parse_pasted_callback(input, "match_state", None).unwrap_err();
        assert_eq!(err, ClosedOutcome::Denied);
    }

    #[test]
    fn parse_pasted_state_mismatch() {
        let input = "code=abc&state=wrong_state";
        let err = parse_pasted_callback(input, "right_state", None).unwrap_err();
        assert_eq!(err, ClosedOutcome::CallbackInvalid);
    }
}
