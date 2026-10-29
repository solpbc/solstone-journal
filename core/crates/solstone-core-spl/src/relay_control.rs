// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Pure helpers for relay WebSocket control traffic.

use serde_json::Value;

/// Converts a relay HTTP endpoint to its WebSocket equivalent.
pub fn websocket_endpoint(endpoint: &str) -> String {
    if let Some(rest) = endpoint.strip_prefix("http://") {
        return format!("ws://{rest}");
    }
    if let Some(rest) = endpoint.strip_prefix("https://") {
        return format!("wss://{rest}");
    }
    endpoint.to_owned()
}

/// Builds a relay tunnel or listen URL.
///
/// The URL never carries the service token: URLs are what edge logs and error
/// reports keep. Send it with [`bearer_authorization_value`] when opening the
/// WebSocket.
pub fn relay_tunnel_url(endpoint: &str, path: &str, instance_id: &str) -> String {
    let websocket_endpoint = websocket_endpoint(endpoint);
    let endpoint_without_slash = websocket_endpoint.trim_end_matches('/');
    format!(
        "{endpoint_without_slash}{path}?instance={}",
        encode_query_component(instance_id),
    )
}

/// Builds the authorization header value that carries the service token.
pub fn bearer_authorization_value(token: &str) -> String {
    format!("Bearer {token}")
}

/// A listen-channel control message that is either actionable or safely ignored.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ListenControl {
    /// A validated tunnel identifier for a newly offered tunnel.
    Incoming { tunnel_id: String },
    /// A well-formed control message that does not offer a tunnel.
    Ignore,
    /// Malformed input or an incomplete/invalid incoming-tunnel offer. This is nonfatal.
    Invalid,
}

/// Parses text or binary WebSocket input as a relay listen control message.
pub fn parse_listen_control(message: impl AsRef<[u8]>) -> ListenControl {
    let text = match std::str::from_utf8(message.as_ref()) {
        Ok(text) => text,
        Err(_) => return ListenControl::Invalid,
    };
    let parsed: Value = match serde_json::from_str(text) {
        Ok(parsed) => parsed,
        Err(_) => return ListenControl::Invalid,
    };
    let object = match parsed.as_object() {
        Some(object) => object,
        None => return ListenControl::Invalid,
    };
    match object.get("type").and_then(Value::as_str) {
        Some("incoming") => {}
        Some(_) => return ListenControl::Ignore,
        None => return ListenControl::Invalid,
    }

    let tunnel_id = match object.get("tunnel_id") {
        Some(Value::String(value)) if !value.is_empty() => Some(value.clone()),
        Some(Value::Number(value)) => integer_tunnel_id(value),
        _ => None,
    };
    match tunnel_id {
        Some(tunnel_id) => ListenControl::Incoming { tunnel_id },
        None => ListenControl::Invalid,
    }
}

/// Converts only JSON integers whose decimal rendering exactly matches Python's `str()`.
///
/// JSON floating-point values are deliberately rejected: their exponent and precision
/// formatting cannot be guaranteed to match Python's `str()` across the full range.
fn integer_tunnel_id(value: &serde_json::Number) -> Option<String> {
    if let Some(integer) = value.as_i64() {
        return Some(integer.to_string());
    }
    value.as_u64().map(|integer| integer.to_string())
}

fn encode_query_component(value: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";

    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                encoded.push(char::from(byte));
            }
            b' ' => encoded.push('+'),
            _ => {
                encoded.push('%');
                encoded.push(char::from(HEX[usize::from(byte >> 4)]));
                encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
            }
        }
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::{
        ListenControl, bearer_authorization_value, parse_listen_control, relay_tunnel_url,
        websocket_endpoint,
    };

    #[test]
    fn converts_http_endpoints_and_preserves_websocket_endpoints() {
        assert_eq!(websocket_endpoint("http://relay.test"), "ws://relay.test");
        assert_eq!(websocket_endpoint("https://relay.test"), "wss://relay.test");
        assert_eq!(websocket_endpoint("ws://relay.test"), "ws://relay.test");
        assert_eq!(websocket_endpoint("wss://relay.test"), "wss://relay.test");
    }

    #[test]
    fn builds_a_url_and_header_through_separate_apis() {
        let url = relay_tunnel_url("https://relay.test/", "/session/listen", "home/a b");
        let authorization = bearer_authorization_value("token +/?");

        assert_eq!(url, "wss://relay.test/session/listen?instance=home%2Fa+b");
        assert_eq!(authorization, "Bearer token +/?");
    }

    #[test]
    fn parses_text_and_binary_incoming_controls() {
        assert_eq!(
            parse_listen_control("{\"type\":\"incoming\",\"tunnel_id\":\"abc\"}"),
            ListenControl::Incoming {
                tunnel_id: "abc".to_owned()
            }
        );
        assert_eq!(
            parse_listen_control(b"{\"type\":\"incoming\",\"tunnel_id\":42}"),
            ListenControl::Incoming {
                tunnel_id: "42".to_owned()
            }
        );
    }

    #[test]
    fn invalidates_malformed_and_incomplete_incoming_controls() {
        for message in [
            b"\xff".as_slice(),
            b"not json".as_slice(),
            b"[]".as_slice(),
            b"{\"type\":\"incoming\"}".as_slice(),
            b"{\"type\":\"incoming\",\"tunnel_id\":\"\"}".as_slice(),
            b"{\"type\":\"incoming\",\"tunnel_id\":1.5}".as_slice(),
            b"{\"type\":\"incoming\",\"tunnel_id\":false}".as_slice(),
        ] {
            assert_eq!(parse_listen_control(message), ListenControl::Invalid);
        }
    }

    #[test]
    fn ignores_well_formed_non_incoming_controls() {
        assert_eq!(
            parse_listen_control("{\"type\":\"connected\"}"),
            ListenControl::Ignore
        );
    }

    #[test]
    fn integer_tunnel_ids_use_python_decimal_rendering() {
        assert_eq!(
            parse_listen_control("{\"type\":\"incoming\",\"tunnel_id\":-42}"),
            ListenControl::Incoming {
                tunnel_id: "-42".to_owned()
            }
        );
        assert_eq!(
            parse_listen_control("{\"type\":\"incoming\",\"tunnel_id\":18446744073709551615}"),
            ListenControl::Incoming {
                tunnel_id: "18446744073709551615".to_owned()
            }
        );
    }
}
