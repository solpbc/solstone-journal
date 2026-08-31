// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Exact, bounded journal-to-bridge proof-of-possession framing.
//!
//! This unit owns only the raw registration and challenge messages. It neither
//! opens sockets nor retains an account authority, so a later carrier may use
//! these bytes without turning a token or key into an exported capability.

use base64::Engine as _;
use ring::signature::Ed25519KeyPair;
use serde::{Deserialize, Serialize};

pub(crate) const BRIDGE_CONTROL_MAX_BYTES: usize = 65_536;
const BRIDGE_NONCE_BYTES: usize = 16;
const ED25519_SIGNATURE_BYTES: usize = 64;

/// A payload-free failure while constructing or checking bridge control bytes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum McpBridgePopError {
    MessageSize,
    Json,
    Nonce,
    BridgeId,
    Timestamp,
    Expired,
    Signature,
}

impl std::fmt::Display for McpBridgePopError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::MessageSize => "MCP bridge control message exceeds its size limit",
            Self::Json => "MCP bridge control message is invalid",
            Self::Nonce => "MCP bridge challenge nonce is invalid",
            Self::BridgeId => "MCP bridge challenge identity is invalid",
            Self::Timestamp => "MCP bridge challenge timestamp is invalid",
            Self::Expired => "MCP bridge registration has expired",
            Self::Signature => "MCP bridge proof signature is invalid",
        })
    }
}

impl std::error::Error for McpBridgePopError {}

#[derive(Serialize)]
struct InitialRegistration<'a> {
    token: &'a str,
    hostname: &'a str,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawChallenge {
    nonce: String,
    bridge_id: String,
    timestamp: i64,
}

#[derive(Serialize)]
struct ChallengeResponse<'a> {
    signature: &'a str,
}

/// The checked bridge challenge, deliberately without token or hostname data.
pub(crate) struct McpBridgeChallenge {
    nonce: [u8; BRIDGE_NONCE_BYTES],
    bridge_id: String,
    timestamp: i64,
}

/// Serialize the first journal registration frame exactly once.
pub(crate) fn initial_registration_frame(
    token: &str,
    hostname: &str,
) -> Result<Vec<u8>, McpBridgePopError> {
    frame_json(&InitialRegistration { token, hostname })
}

/// Parse and bind one bridge challenge to the already-admitted registration.
pub(crate) fn parse_challenge_frame(
    frame: &[u8],
    expected_bridge_id: &str,
    issued_at: i64,
    expires_at: i64,
    wall_now: i64,
) -> Result<McpBridgeChallenge, McpBridgePopError> {
    if wall_now >= expires_at {
        return Err(McpBridgePopError::Expired);
    }
    let raw: RawChallenge = parse_frame_json(frame)?;
    if raw.bridge_id != expected_bridge_id {
        return Err(McpBridgePopError::BridgeId);
    }
    if !(issued_at..expires_at).contains(&raw.timestamp) {
        return Err(McpBridgePopError::Timestamp);
    }
    let nonce = decode_canonical_nonce(&raw.nonce)?;
    Ok(McpBridgeChallenge {
        nonce,
        bridge_id: raw.bridge_id,
        timestamp: raw.timestamp,
    })
}

/// Produce the complete, framed signature-only response to a checked challenge.
pub(crate) fn proof_response_frame(
    keypair: &Ed25519KeyPair,
    challenge: &McpBridgeChallenge,
) -> Result<Vec<u8>, McpBridgePopError> {
    let mut signed = Vec::with_capacity(BRIDGE_NONCE_BYTES + challenge.bridge_id.len() + 8);
    signed.extend_from_slice(&challenge.nonce);
    signed.extend_from_slice(challenge.bridge_id.as_bytes());
    signed.extend_from_slice(&challenge.timestamp.to_be_bytes());
    let signature = keypair.sign(&signed);
    let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(signature.as_ref());
    if signature.as_ref().len() != ED25519_SIGNATURE_BYTES {
        return Err(McpBridgePopError::Signature);
    }
    frame_json(&ChallengeResponse {
        signature: &encoded,
    })
}

fn frame_json<T: Serialize>(value: &T) -> Result<Vec<u8>, McpBridgePopError> {
    let body = serde_json::to_vec(value).map_err(|_| McpBridgePopError::Json)?;
    if body.len() > BRIDGE_CONTROL_MAX_BYTES {
        return Err(McpBridgePopError::MessageSize);
    }
    let length = u32::try_from(body.len()).map_err(|_| McpBridgePopError::MessageSize)?;
    let mut frame = Vec::with_capacity(4 + body.len());
    frame.extend_from_slice(&length.to_be_bytes());
    frame.extend_from_slice(&body);
    Ok(frame)
}

fn parse_frame_json<T: for<'de> Deserialize<'de>>(frame: &[u8]) -> Result<T, McpBridgePopError> {
    let prefix: [u8; 4] = frame
        .get(..4)
        .ok_or(McpBridgePopError::MessageSize)?
        .try_into()
        .map_err(|_| McpBridgePopError::MessageSize)?;
    let body_length =
        usize::try_from(u32::from_be_bytes(prefix)).map_err(|_| McpBridgePopError::MessageSize)?;
    if body_length > BRIDGE_CONTROL_MAX_BYTES || frame.len() != 4 + body_length {
        return Err(McpBridgePopError::MessageSize);
    }
    serde_json::from_slice(&frame[4..]).map_err(|_| McpBridgePopError::Json)
}

fn decode_canonical_nonce(value: &str) -> Result<[u8; BRIDGE_NONCE_BYTES], McpBridgePopError> {
    let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(value)
        .map_err(|_| McpBridgePopError::Nonce)?;
    if decoded.len() != BRIDGE_NONCE_BYTES
        || base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&decoded) != value
    {
        return Err(McpBridgePopError::Nonce);
    }
    decoded.try_into().map_err(|_| McpBridgePopError::Nonce)
}

#[cfg(test)]
mod tests {
    use super::{McpBridgePopError, initial_registration_frame, parse_challenge_frame};

    const HOSTNAME: &str = "aaaqeaye.solstone.me";
    const BRIDGE_ID: &str = "bridge-fixture";
    const ISSUED_AT: i64 = 1_700_000_000;
    const EXPIRES_AT: i64 = 1_700_000_900;
    const CORPUS: &str = include_str!("../test-fixtures/mcp_bridge_pop_v1.md");

    fn challenge_frame(body: &[u8]) -> Vec<u8> {
        let mut frame = u32::try_from(body.len())
            .expect("fixture body is representable")
            .to_be_bytes()
            .to_vec();
        frame.extend_from_slice(body);
        frame
    }

    #[test]
    fn initial_registration_is_the_pinned_raw_corpus_literal() {
        let actual = initial_registration_frame("fixture-token", HOSTNAME)
            .expect("fixture registration serializes");
        let expected = [
            0, 0, 0, 59, b'{', b'\"', b't', b'o', b'k', b'e', b'n', b'\"', b':', b'\"', b'f', b'i',
            b'x', b't', b'u', b'r', b'e', b'-', b't', b'o', b'k', b'e', b'n', b'\"', b',', b'\"',
            b'h', b'o', b's', b't', b'n', b'a', b'm', b'e', b'\"', b':', b'\"', b'a', b'a', b'a',
            b'q', b'e', b'a', b'y', b'e', b'.', b's', b'o', b'l', b's', b't', b'o', b'n', b'e',
            b'.', b'm', b'e', b'\"', b'}',
        ];
        assert_eq!(actual, expected);
        assert!(CORPUS.contains("ea5de0136e4fa24f4ad092a2756178b197ed50e7"));
        assert!(CORPUS.contains("crates/spl-bridge/src/pop_auth.rs"));
        assert!(CORPUS.contains("crates/spl-bridge/tests/journal_lease_renewal.rs"));
        assert!(CORPUS.contains(
            "0000003b7b22746f6b656e223a22666978747572652d746f6b656e222c22686f73746e616d65223a2261616171656179652e736f6c73746f6e652e6d65227d"
        ));
    }

    #[test]
    fn challenge_requires_exact_frame_shape_canonical_nonce_and_token_time_window() {
        let valid = challenge_frame(
            br#"{"nonce":"AAECAwQFBgcICQoLDA0ODw","bridge_id":"bridge-fixture","timestamp":1700000000}"#,
        );
        assert!(
            parse_challenge_frame(&valid, BRIDGE_ID, ISSUED_AT, EXPIRES_AT, ISSUED_AT,).is_ok()
        );

        for (body, expected) in [
            (
                br#"{"nonce":"AAECAwQFBgcICQoLDA0ODw=","bridge_id":"bridge-fixture","timestamp":1700000000}"#
                    .as_slice(),
                McpBridgePopError::Nonce,
            ),
            (
                br#"{"nonce":"AAECAwQFBgcICQoLDA0ODw","bridge_id":"other","timestamp":1700000000}"#
                    .as_slice(),
                McpBridgePopError::BridgeId,
            ),
            (
                br#"{"nonce":"AAECAwQFBgcICQoLDA0ODw","bridge_id":"bridge-fixture","timestamp":1700000900}"#
                    .as_slice(),
                McpBridgePopError::Timestamp,
            ),
            (
                br#"{"nonce":"AAECAwQFBgcICQoLDA0ODw","bridge_id":"bridge-fixture","timestamp":1700000000,"extra":true}"#
                    .as_slice(),
                McpBridgePopError::Json,
            ),
        ] {
            let frame = challenge_frame(body);
            assert!(matches!(
                parse_challenge_frame(&frame, BRIDGE_ID, ISSUED_AT, EXPIRES_AT, ISSUED_AT),
                Err(actual) if actual == expected
            ));
        }
        assert!(matches!(
            parse_challenge_frame(&valid, BRIDGE_ID, ISSUED_AT, EXPIRES_AT, EXPIRES_AT),
            Err(McpBridgePopError::Expired)
        ));
    }
}
