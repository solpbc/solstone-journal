// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Compact account-registration wire construction from an admitted owner.

use std::fmt;

use base64::Engine as _;
use ring::rand::SystemRandom;
use ring::signature::{ECDSA_P256_SHA256_FIXED_SIGNING, EcdsaKeyPair, KeyPair as _};
use serde::Serialize;

use crate::McpEndpointOwnerContext;

const ASSERTION_CAP_BYTES: usize = 8_192;
const REQUEST_CAP_BYTES: usize = 16_384;
const ASSERTION_LIFETIME_SECONDS: i64 = 240;

pub(crate) struct McpAccountRequest {
    body: Vec<u8>,
}

impl McpAccountRequest {
    pub(crate) fn body_bytes(&self) -> &[u8] {
        &self.body
    }
}

#[derive(Debug)]
pub(crate) enum McpAccountWireError {
    ExpirationOverflow,
    SigningKeyLoad,
    EcdsaSign,
    JsonSerialization,
    AssertionLengthCap,
    RequestLengthCap,
}

impl fmt::Display for McpAccountWireError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::ExpirationOverflow => "MCP account registration expiration overflowed",
            Self::SigningKeyLoad => "MCP account registration signing key could not be loaded",
            Self::EcdsaSign => "MCP account registration assertion could not be signed",
            Self::JsonSerialization => "MCP account registration JSON could not be serialized",
            Self::AssertionLengthCap => "MCP account registration assertion exceeds its size limit",
            Self::RequestLengthCap => "MCP account registration request exceeds its size limit",
        })
    }
}

impl std::error::Error for McpAccountWireError {}

#[derive(Serialize)]
struct ProtectedHeader {
    alg: &'static str,
    typ: &'static str,
}

#[derive(Serialize)]
struct AssertionClaims<'a> {
    iss: String,
    aud: &'static str,
    scope: &'static str,
    instance_id: &'a str,
    iat: i64,
    exp: i64,
}

#[derive(Serialize)]
struct CnfJwk<'a> {
    kty: &'static str,
    crv: &'static str,
    x: &'a str,
}

#[derive(Serialize)]
struct RequestBody<'a> {
    instance_id: &'a str,
    assertion: &'a str,
    ca_pubkey: &'a str,
    cnf_jwk: CnfJwk<'a>,
}

/// Build the exact compact account-registration request from an admitted owner.
pub(crate) fn build_account_registration_request(
    owner: &McpEndpointOwnerContext,
    wall_unix_seconds: i64,
) -> Result<McpAccountRequest, McpAccountWireError> {
    let exp = wall_unix_seconds
        .checked_add(ASSERTION_LIFETIME_SECONDS)
        .ok_or(McpAccountWireError::ExpirationOverflow)?;
    let instance_id = owner.committed.instance_id();
    let header = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(serialize_header(
        &ProtectedHeader {
            alg: "ES256",
            typ: "home-reach",
        },
    )?);
    let claims = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(serialize_claims(
        &AssertionClaims {
            iss: format!("home:{instance_id}"),
            aud: "solstone-reach",
            scope: "mcp.bridge.register",
            instance_id,
            iat: wall_unix_seconds,
            exp,
        },
    )?);
    let signing_input = format!("{header}.{claims}");
    checkpoint(AccountWirePrimitive::SigningKeyLoad)?;
    let ca_key = rcgen::KeyPair::from_pem_and_sign_algo(
        &owner.committed.ca().private_key_pem(),
        &rcgen::PKCS_ECDSA_P256_SHA256,
    )
    .map_err(|_| McpAccountWireError::SigningKeyLoad)?;
    let signing_key = EcdsaKeyPair::from_pkcs8(
        &ECDSA_P256_SHA256_FIXED_SIGNING,
        &ca_key.serialize_der(),
        &SystemRandom::new(),
    )
    .map_err(|_| McpAccountWireError::SigningKeyLoad)?;
    checkpoint(AccountWirePrimitive::EcdsaSign)?;
    let signature = signing_key
        .sign(&SystemRandom::new(), signing_input.as_bytes())
        .map_err(|_| McpAccountWireError::EcdsaSign)?;
    let assertion = format!(
        "{signing_input}.{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(signature.as_ref())
    );
    enforce_len_cap(
        assertion.as_bytes(),
        ASSERTION_CAP_BYTES,
        McpAccountWireError::AssertionLengthCap,
    )?;

    let pop_key = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(owner.keypair.public_key().as_ref());
    let ca_pubkey = ca_public_key_pem(owner.committed.ca().spki_der());
    let body = serialize_request(&RequestBody {
        instance_id,
        assertion: &assertion,
        ca_pubkey: &ca_pubkey,
        cnf_jwk: CnfJwk {
            kty: "OKP",
            crv: "Ed25519",
            x: &pop_key,
        },
    })?;
    enforce_len_cap(
        &body,
        REQUEST_CAP_BYTES,
        McpAccountWireError::RequestLengthCap,
    )?;
    Ok(McpAccountRequest { body })
}

fn serialize_header(header: &ProtectedHeader) -> Result<Vec<u8>, McpAccountWireError> {
    checkpoint(AccountWirePrimitive::HeaderJsonSerialization)?;
    serde_json::to_vec(header).map_err(|_| McpAccountWireError::JsonSerialization)
}

fn serialize_claims(claims: &AssertionClaims<'_>) -> Result<Vec<u8>, McpAccountWireError> {
    checkpoint(AccountWirePrimitive::ClaimsJsonSerialization)?;
    serde_json::to_vec(claims).map_err(|_| McpAccountWireError::JsonSerialization)
}

fn serialize_request(request: &RequestBody<'_>) -> Result<Vec<u8>, McpAccountWireError> {
    checkpoint(AccountWirePrimitive::RequestJsonSerialization)?;
    serde_json::to_vec(request).map_err(|_| McpAccountWireError::JsonSerialization)
}

fn enforce_len_cap(
    bytes: &[u8],
    cap: usize,
    if_over: McpAccountWireError,
) -> Result<(), McpAccountWireError> {
    if bytes.len() > cap {
        Err(if_over)
    } else {
        Ok(())
    }
}

fn ca_public_key_pem(spki_der: &[u8]) -> String {
    let encoded = base64::engine::general_purpose::STANDARD.encode(spki_der);
    let mut pem = String::from("-----BEGIN PUBLIC KEY-----");
    for line in encoded.as_bytes().chunks(64) {
        pem.push('\n');
        pem.push_str(std::str::from_utf8(line).expect("base64 output is ASCII"));
    }
    pem.push_str("\n-----END PUBLIC KEY-----");
    pem
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AccountWirePrimitive {
    HeaderJsonSerialization,
    ClaimsJsonSerialization,
    RequestJsonSerialization,
    SigningKeyLoad,
    EcdsaSign,
}

#[cfg(any(test, feature = "test-hooks"))]
struct AccountWireFault {
    primitive: AccountWirePrimitive,
    consumed: bool,
}

#[cfg(any(test, feature = "test-hooks"))]
thread_local! {
    static ACCOUNT_WIRE_FAULT: std::cell::RefCell<Option<AccountWireFault>> = const {
        std::cell::RefCell::new(None)
    };
}

#[cfg(any(test, feature = "test-hooks"))]
struct AccountWireFaultGuard;

#[cfg(any(test, feature = "test-hooks"))]
impl AccountWireFaultGuard {
    fn install(primitive: AccountWirePrimitive) -> Self {
        ACCOUNT_WIRE_FAULT.with(|fault| {
            assert!(
                fault.borrow().is_none(),
                "account-registration fault is already active"
            );
            *fault.borrow_mut() = Some(AccountWireFault {
                primitive,
                consumed: false,
            });
        });
        Self
    }

    fn was_consumed(&self) -> bool {
        ACCOUNT_WIRE_FAULT.with(|fault| {
            fault
                .borrow()
                .as_ref()
                .expect("account-registration fault remains active")
                .consumed
        })
    }
}

#[cfg(any(test, feature = "test-hooks"))]
impl Drop for AccountWireFaultGuard {
    fn drop(&mut self) {
        ACCOUNT_WIRE_FAULT.with(|fault| {
            fault
                .borrow_mut()
                .take()
                .expect("account-registration fault remains active");
        });
    }
}

#[cfg(any(test, feature = "test-hooks"))]
fn run_with_account_wire_fault<T>(
    primitive: AccountWirePrimitive,
    op: impl FnOnce() -> T,
) -> (T, bool) {
    let guard = AccountWireFaultGuard::install(primitive);
    let result = op();
    (result, guard.was_consumed())
}

#[cfg(any(test, feature = "test-hooks"))]
fn checkpoint(primitive: AccountWirePrimitive) -> Result<(), McpAccountWireError> {
    ACCOUNT_WIRE_FAULT.with(|fault| {
        let mut fault = fault.borrow_mut();
        let Some(fault) = fault.as_mut() else {
            return Ok(());
        };
        if fault.primitive != primitive || fault.consumed {
            return Ok(());
        }
        fault.consumed = true;
        Err(match primitive {
            AccountWirePrimitive::HeaderJsonSerialization
            | AccountWirePrimitive::ClaimsJsonSerialization
            | AccountWirePrimitive::RequestJsonSerialization => {
                McpAccountWireError::JsonSerialization
            }
            AccountWirePrimitive::SigningKeyLoad => McpAccountWireError::SigningKeyLoad,
            AccountWirePrimitive::EcdsaSign => McpAccountWireError::EcdsaSign,
        })
    })
}

#[cfg(not(any(test, feature = "test-hooks")))]
fn checkpoint(_primitive: AccountWirePrimitive) -> Result<(), McpAccountWireError> {
    Ok(())
}

const RESPONSE_BODY_MAX_BYTES: usize = 65_536;
const TOKEN_MAX_BYTES: usize = 16_384;
const EXPIRES_AT_MAX_BYTES: usize = 64;
const INSTANCE_ID_MAX_BYTES: usize = 128;
const BRIDGE_ID_MAX_BYTES: usize = 128;
const V1_DENIED_IPV4_RANGES: &[(std::net::Ipv4Addr, std::net::Ipv4Addr)] = &[
    (
        std::net::Ipv4Addr::new(0, 0, 0, 0),
        std::net::Ipv4Addr::new(0, 255, 255, 255),
    ),
    (
        std::net::Ipv4Addr::new(10, 0, 0, 0),
        std::net::Ipv4Addr::new(10, 255, 255, 255),
    ),
    (
        std::net::Ipv4Addr::new(100, 64, 0, 0),
        std::net::Ipv4Addr::new(100, 127, 255, 255),
    ),
    (
        std::net::Ipv4Addr::new(127, 0, 0, 0),
        std::net::Ipv4Addr::new(127, 255, 255, 255),
    ),
    (
        std::net::Ipv4Addr::new(169, 254, 0, 0),
        std::net::Ipv4Addr::new(169, 254, 255, 255),
    ),
    (
        std::net::Ipv4Addr::new(172, 16, 0, 0),
        std::net::Ipv4Addr::new(172, 31, 255, 255),
    ),
    (
        std::net::Ipv4Addr::new(192, 0, 0, 0),
        std::net::Ipv4Addr::new(192, 0, 0, 255),
    ),
    (
        std::net::Ipv4Addr::new(192, 0, 2, 0),
        std::net::Ipv4Addr::new(192, 0, 2, 255),
    ),
    (
        std::net::Ipv4Addr::new(192, 88, 99, 0),
        std::net::Ipv4Addr::new(192, 88, 99, 255),
    ),
    (
        std::net::Ipv4Addr::new(192, 168, 0, 0),
        std::net::Ipv4Addr::new(192, 168, 255, 255),
    ),
    (
        std::net::Ipv4Addr::new(198, 18, 0, 0),
        std::net::Ipv4Addr::new(198, 19, 255, 255),
    ),
    (
        std::net::Ipv4Addr::new(198, 51, 100, 0),
        std::net::Ipv4Addr::new(198, 51, 100, 255),
    ),
    (
        std::net::Ipv4Addr::new(203, 0, 113, 0),
        std::net::Ipv4Addr::new(203, 0, 113, 255),
    ),
    (
        std::net::Ipv4Addr::new(224, 0, 0, 0),
        std::net::Ipv4Addr::new(239, 255, 255, 255),
    ),
    (
        std::net::Ipv4Addr::new(240, 0, 0, 0),
        std::net::Ipv4Addr::new(255, 255, 255, 255),
    ),
];

struct McpAccountResponseWire {
    token: String,
    expires_in: u64,
    expires_at: String,
    instance_id: String,
    hostname: String,
    bridge_id: String,
    bridge_address: String,
}

#[cfg(test)]
impl McpAccountResponseWire {
    fn token(&self) -> &str {
        &self.token
    }

    fn expires_in(&self) -> u64 {
        self.expires_in
    }

    fn expires_at(&self) -> &str {
        &self.expires_at
    }

    fn instance_id(&self) -> &str {
        &self.instance_id
    }

    fn hostname(&self) -> &str {
        &self.hostname
    }

    fn bridge_id(&self) -> &str {
        &self.bridge_id
    }

    fn bridge_address(&self) -> &str {
        &self.bridge_address
    }
}

#[derive(Debug)]
enum McpAccountResponseWireError {
    BodySize,
    UnexpectedStatus,
    CacheControlMissing,
    CacheControlDuplicate,
    CacheControlNonAscii,
    CacheControlMalformedOws,
    CacheControlWrongDirective,
    CacheControlMultipleDirectives,
    BodyUtf8,
    BodyJson,
    TokenLength,
    TokenType,
    ExpiresIn,
    ExpiresAtLength,
    InstanceIdLength,
    Hostname,
    BridgeId,
    BridgeAddressesCardinality,
    BridgeAddressIpv4,
    BridgeAddressDenied,
}

impl fmt::Display for McpAccountResponseWireError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::BodySize => "MCP account registration response body exceeds its size bounds",
            Self::UnexpectedStatus => "MCP account registration response has an unexpected status",
            Self::CacheControlMissing => {
                "MCP account registration response is missing Cache-Control"
            }
            Self::CacheControlDuplicate => {
                "MCP account registration response has duplicate Cache-Control"
            }
            Self::CacheControlNonAscii => {
                "MCP account registration response Cache-Control is not ASCII"
            }
            Self::CacheControlMalformedOws => {
                "MCP account registration response Cache-Control has malformed whitespace"
            }
            Self::CacheControlWrongDirective => {
                "MCP account registration response Cache-Control has the wrong directive"
            }
            Self::CacheControlMultipleDirectives => {
                "MCP account registration response Cache-Control has multiple directives"
            }
            Self::BodyUtf8 => "MCP account registration response body is not UTF-8",
            Self::BodyJson => "MCP account registration response body is not valid JSON",
            Self::TokenLength => "MCP account registration response token exceeds its size bounds",
            Self::TokenType => "MCP account registration response token type is invalid",
            Self::ExpiresIn => "MCP account registration response expiry is invalid",
            Self::ExpiresAtLength => {
                "MCP account registration response expiry timestamp exceeds its size bounds"
            }
            Self::InstanceIdLength => {
                "MCP account registration response instance ID exceeds its size bounds"
            }
            Self::Hostname => "MCP account registration response hostname is invalid",
            Self::BridgeId => "MCP account registration response bridge ID is invalid",
            Self::BridgeAddressesCardinality => {
                "MCP account registration response bridge address count is invalid"
            }
            Self::BridgeAddressIpv4 => {
                "MCP account registration response bridge address is not canonical IPv4"
            }
            Self::BridgeAddressDenied => {
                "MCP account registration response bridge address is denied"
            }
        })
    }
}

impl std::error::Error for McpAccountResponseWireError {}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct McpAccountResponseBody {
    token: String,
    token_type: String,
    expires_in: u64,
    expires_at: String,
    instance_id: String,
    hostname: String,
    bridge_id: String,
    bridge_addresses: Vec<String>,
}

fn parse_account_registration_response(
    status: u16,
    headers: &[(Vec<u8>, Vec<u8>)],
    body: &[u8],
) -> Result<McpAccountResponseWire, McpAccountResponseWireError> {
    if status != 200 {
        return Err(McpAccountResponseWireError::UnexpectedStatus);
    }
    if !(1..=RESPONSE_BODY_MAX_BYTES).contains(&body.len()) {
        return Err(McpAccountResponseWireError::BodySize);
    }
    validate_cache_control(headers)?;
    std::str::from_utf8(body).map_err(|_| McpAccountResponseWireError::BodyUtf8)?;
    let response: McpAccountResponseBody =
        serde_json::from_slice(body).map_err(|_| McpAccountResponseWireError::BodyJson)?;

    if response.token.is_empty() || response.token.len() > TOKEN_MAX_BYTES {
        return Err(McpAccountResponseWireError::TokenLength);
    }
    if response.token_type != "Bearer" {
        return Err(McpAccountResponseWireError::TokenType);
    }
    if response.expires_in == 0 {
        return Err(McpAccountResponseWireError::ExpiresIn);
    }
    if response.expires_at.is_empty() || response.expires_at.len() > EXPIRES_AT_MAX_BYTES {
        return Err(McpAccountResponseWireError::ExpiresAtLength);
    }
    if response.instance_id.is_empty() || response.instance_id.len() > INSTANCE_ID_MAX_BYTES {
        return Err(McpAccountResponseWireError::InstanceIdLength);
    }
    if !is_valid_hostname(&response.hostname) {
        return Err(McpAccountResponseWireError::Hostname);
    }
    if !is_valid_bridge_id(&response.bridge_id) {
        return Err(McpAccountResponseWireError::BridgeId);
    }
    if response.bridge_addresses.len() != 1 {
        return Err(McpAccountResponseWireError::BridgeAddressesCardinality);
    }
    let bridge_address = canonical_bridge_address(&response.bridge_addresses[0])?;

    Ok(McpAccountResponseWire {
        token: response.token,
        expires_in: response.expires_in,
        expires_at: response.expires_at,
        instance_id: response.instance_id,
        hostname: response.hostname,
        bridge_id: response.bridge_id,
        bridge_address,
    })
}

fn validate_cache_control(
    headers: &[(Vec<u8>, Vec<u8>)],
) -> Result<(), McpAccountResponseWireError> {
    let mut value = None;
    for (name, candidate) in headers {
        if !name.eq_ignore_ascii_case(b"Cache-Control") {
            continue;
        }
        if value.replace(candidate).is_some() {
            return Err(McpAccountResponseWireError::CacheControlDuplicate);
        }
    }
    let value = value.ok_or(McpAccountResponseWireError::CacheControlMissing)?;
    if !value.is_ascii() {
        return Err(McpAccountResponseWireError::CacheControlNonAscii);
    }

    let directives: Vec<_> = value
        .split(|byte| *byte == b',')
        .map(trim_cache_control_ows)
        .collect::<Result<_, _>>()?;
    if directives.len() != 1 {
        return Err(McpAccountResponseWireError::CacheControlMultipleDirectives);
    }
    if !directives[0].eq_ignore_ascii_case(b"no-store") {
        return Err(McpAccountResponseWireError::CacheControlWrongDirective);
    }
    Ok(())
}

fn trim_cache_control_ows(value: &[u8]) -> Result<&[u8], McpAccountResponseWireError> {
    let start = value
        .iter()
        .position(|byte| !matches!(*byte, b' ' | b'\t'))
        .unwrap_or(value.len());
    let value = &value[start..];
    let end = value
        .iter()
        .rposition(|byte| !matches!(*byte, b' ' | b'\t'))
        .map_or(0, |index| index + 1);
    let trimmed = &value[..end];
    if trimmed.iter().any(|byte| byte.is_ascii_whitespace()) {
        return Err(McpAccountResponseWireError::CacheControlMalformedOws);
    }
    Ok(trimmed)
}

fn is_valid_hostname(hostname: &str) -> bool {
    let bytes = hostname.as_bytes();
    bytes.len() == 20
        && bytes.ends_with(b".solstone.me")
        && bytes[..8]
            .iter()
            .all(|byte| matches!(*byte, b'a'..=b'z' | b'2'..=b'7'))
}

fn is_valid_bridge_id(bridge_id: &str) -> bool {
    !bridge_id.is_empty()
        && bridge_id.len() <= BRIDGE_ID_MAX_BYTES
        && bridge_id.as_bytes().iter().all(|byte| {
            matches!(
                *byte,
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'.' | b'_' | b':' | b'-'
            )
        })
}

fn canonical_bridge_address(address: &str) -> Result<String, McpAccountResponseWireError> {
    let parsed = <std::net::Ipv4Addr as std::str::FromStr>::from_str(address)
        .map_err(|_| McpAccountResponseWireError::BridgeAddressIpv4)?;
    let canonical = parsed.to_string();
    if canonical != address {
        return Err(McpAccountResponseWireError::BridgeAddressIpv4);
    }
    if is_v1_denied_ipv4(parsed) {
        return Err(McpAccountResponseWireError::BridgeAddressDenied);
    }
    Ok(canonical)
}

fn is_v1_denied_ipv4(address: std::net::Ipv4Addr) -> bool {
    let address = u32::from_be_bytes(address.octets());
    V1_DENIED_IPV4_RANGES.iter().any(|(start, end)| {
        let start = u32::from_be_bytes(start.octets());
        let end = u32::from_be_bytes(end.octets());
        start <= address && address <= end
    })
}

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;
    use std::fs;
    use std::os::unix::fs::PermissionsExt as _;
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::path::Path;

    use base64::Engine as _;
    use ring::digest;
    use ring::rand::SystemRandom;
    use ring::signature::{
        ECDSA_P256_SHA256_FIXED, Ed25519KeyPair, KeyPair as _, UnparsedPublicKey,
    };
    use tempfile::TempDir;

    use super::*;
    use crate::bootstrap_mcp_endpoint_owner_identity;

    const FIXED_POP_PKCS8_BASE64: &str = "MFECAQEwBQYDK2VwBCIEIK3BpJ7oyV0+ISBYqk1FhL7ddzXR7+nKfGOiBQ658apJgSEAWAnp/vbc7Fjw8uOw1n6YgKEZV+CDrOhYNcO2yPuva30=";
    const P256_SPKI_PREFIX: [u8; 26] = [
        0x30, 0x59, 0x30, 0x13, 0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01, 0x06, 0x08,
        0x2a, 0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07, 0x03, 0x42, 0x00,
    ];

    #[test]
    fn fixture_provenance_is_pinned() {
        const SOURCE_REPOSITORY: &str = "solstone.app";
        const SOURCE_PATH: &str = "account/test-fixtures/mcp_bridge_v1.json";
        const SOURCE_COMMIT: &str = "6c3dc18376b365792f5cb512eb5bf17d8eff17bc";
        const SIZE: usize = 2_203;
        const SHA256: &str = "6563b737522de561b62a00a93e5a083f5cfa56608bd45ea1bc388c0ee395c956";

        let fixture = fixture_bytes();
        assert_eq!(
            fixture.len(),
            SIZE,
            "{SOURCE_REPOSITORY}:{SOURCE_PATH}@{SOURCE_COMMIT}; cross-repo byte equality is VPE-direct evidence and is not asserted here"
        );
        assert_eq!(
            hex(digest::digest(&digest::SHA256, fixture).as_ref()),
            SHA256,
            "{SOURCE_REPOSITORY}:{SOURCE_PATH}@{SOURCE_COMMIT}; cross-repo byte equality is VPE-direct evidence and is not asserted here"
        );
    }

    #[test]
    fn fixture_request_assertion_matches_the_independent_protocol_oracle() {
        let fixture: serde_json::Value =
            serde_json::from_slice(fixture_bytes()).expect("fixture JSON");
        let request_body = fixture["request"]["body"]
            .as_str()
            .expect("fixture request body string");
        let request: serde_json::Value = serde_json::from_str(request_body).expect("request JSON");
        assert_object_keys(
            &request,
            &["instance_id", "assertion", "ca_pubkey", "cnf_jwk"],
        );
        assert_eq!(
            request["instance_id"],
            "8488ae64-b592-80a3-97c6-490e995daa85"
        );
        let ca_pubkey = request["ca_pubkey"].as_str().expect("fixture CA PEM");
        assert_eq!(
            ca_pubkey,
            "-----BEGIN PUBLIC KEY-----\nMFkwEwYHKoZIzj0CAQYIKoZIzj0DAQcDQgAEDqAw0i9YxRG5/1DAZ1eLejZJuTcq\nPjxbfiv6klgXm9nk08MUGpdn/Cgw5Fc0/lI39DF1GiyQ9AewtkawyxUDIQ==\n-----END PUBLIC KEY-----"
        );
        assert!(!ca_pubkey.ends_with('\n'));
        let spki_der = pem_spki_der(ca_pubkey);
        let assertion = request["assertion"].as_str().expect("fixture assertion");
        let (header_segment, claims_segment, signature_segment) = compact_parts(assertion);
        assert_canonical_base64url(header_segment);
        assert_canonical_base64url(claims_segment);
        assert_canonical_base64url(signature_segment);
        let header: serde_json::Value = serde_json::from_slice(
            &base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(header_segment)
                .expect("header bytes"),
        )
        .expect("header JSON");
        let claims: serde_json::Value = serde_json::from_slice(
            &base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(claims_segment)
                .expect("claims bytes"),
        )
        .expect("claims JSON");
        let signature = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(signature_segment)
            .expect("signature bytes");
        assert_eq!(signature.len(), 64);
        assert_object_keys(&header, &["alg", "typ"]);
        assert_eq!(header["alg"], "ES256");
        assert_eq!(header["typ"], "home-reach");
        assert_object_keys(
            &claims,
            &["iss", "aud", "scope", "instance_id", "iat", "exp"],
        );
        assert_eq!(claims["iss"], "home:8488ae64-b592-80a3-97c6-490e995daa85");
        assert_eq!(claims["aud"], "solstone-reach");
        assert_eq!(claims["scope"], "mcp.bridge.register");
        assert_eq!(
            claims["instance_id"],
            "8488ae64-b592-80a3-97c6-490e995daa85"
        );
        assert_eq!(claims["iat"], 1_700_000_000_i64);
        assert_eq!(claims["exp"], 1_700_000_240_i64);
        assert_eq!(
            claims["exp"].as_i64().expect("exp") - claims["iat"].as_i64().expect("iat"),
            240
        );
        let cnf_jwk = &request["cnf_jwk"];
        assert_object_keys(cnf_jwk, &["kty", "crv", "x"]);
        assert_eq!(cnf_jwk["kty"], "OKP");
        assert_eq!(cnf_jwk["crv"], "Ed25519");
        assert_eq!(cnf_jwk["x"], "AsjOOYUMUDDGYVvf2a02SDEXab1H9W3Zvc4WXzymL4c");
        assert_canonical_base64url(cnf_jwk["x"].as_str().expect("JWK key"));

        assert!(signature_verifies(
            &spki_der,
            &format!("{header_segment}.{claims_segment}"),
            &signature
        ));
        assert!(!signature_verifies(
            &spki_der,
            &format!("{}.{}", flip_one_bit(header_segment), claims_segment),
            &base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(signature_segment)
                .expect("signature bytes"),
        ));
        assert!(!signature_verifies(
            &spki_der,
            &format!("{header_segment}.{}", flip_one_bit(claims_segment)),
            &base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(signature_segment)
                .expect("signature bytes"),
        ));
        assert!(!signature_verifies(
            &spki_der,
            &format!("{header_segment}.{claims_segment}"),
            &base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(flip_one_bit(signature_segment))
                .expect("mutated signature bytes"),
        ));
    }

    #[test]
    fn builder_emits_a_verified_exact_account_request_from_a_fixed_pop_key() {
        let fixed_pop = fixed_pop_pkcs8();
        let (root, owner) = owner_with_pop(&fixed_pop);
        let wall_unix_seconds = 1_700_000_000;
        let request = build_account_registration_request(&owner, wall_unix_seconds)
            .expect("account registration request");
        assert_valid_account_request(
            request.body_bytes(),
            owner.committed.instance_id(),
            &fixed_pop,
            wall_unix_seconds,
            owner.committed.ca().spki_der(),
        );
        drop(root);
    }

    #[test]
    fn distinct_owners_pop_keys_and_times_produce_distinct_verified_values() {
        let first_pop = fixed_pop_pkcs8();
        let second_pop = Ed25519KeyPair::generate_pkcs8(&SystemRandom::new())
            .expect("second PoP key")
            .as_ref()
            .to_vec();
        let (first_root, first_owner) = owner_with_pop(&first_pop);
        let (second_root, second_owner) = owner_with_pop(&second_pop);
        let first_time = 1_700_000_000;
        let second_time = first_time + 1;
        let first_request = build_account_registration_request(&first_owner, first_time)
            .expect("first account request");
        let second_request = build_account_registration_request(&second_owner, second_time)
            .expect("second account request");
        assert_ne!(
            first_owner.committed.instance_id(),
            second_owner.committed.instance_id()
        );
        assert_ne!(
            Ed25519KeyPair::from_pkcs8(&first_pop)
                .expect("first PoP parses")
                .public_key()
                .as_ref(),
            Ed25519KeyPair::from_pkcs8(&second_pop)
                .expect("second PoP parses")
                .public_key()
                .as_ref()
        );
        assert_valid_account_request(
            first_request.body_bytes(),
            first_owner.committed.instance_id(),
            &first_pop,
            first_time,
            first_owner.committed.ca().spki_der(),
        );
        assert_valid_account_request(
            second_request.body_bytes(),
            second_owner.committed.instance_id(),
            &second_pop,
            second_time,
            second_owner.committed.ca().spki_der(),
        );
        drop((first_root, second_root));
    }

    #[test]
    fn caps_are_pure_and_independently_testable() {
        assert!(enforce_len_cap(b"ab", 2, McpAccountWireError::AssertionLengthCap).is_ok());
        assert!(matches!(
            enforce_len_cap(b"abc", 2, McpAccountWireError::AssertionLengthCap),
            Err(McpAccountWireError::AssertionLengthCap)
        ));
        assert!(matches!(
            enforce_len_cap(b"abc", 2, McpAccountWireError::RequestLengthCap),
            Err(McpAccountWireError::RequestLengthCap)
        ));
    }

    #[test]
    fn overflow_stops_before_any_signing_checkpoint() {
        let (_root, owner) = owner_with_pop(&fixed_pop_pkcs8());
        let (result, consumed) =
            run_with_account_wire_fault(AccountWirePrimitive::SigningKeyLoad, || {
                build_account_registration_request(&owner, i64::MAX)
            });
        assert!(matches!(
            result,
            Err(McpAccountWireError::ExpirationOverflow)
        ));
        assert!(!consumed, "overflow must stop before signing");
    }

    #[test]
    fn account_wire_faults_cover_serialization_and_signing() {
        let (_root, owner) = owner_with_pop(&fixed_pop_pkcs8());
        for primitive in [
            AccountWirePrimitive::HeaderJsonSerialization,
            AccountWirePrimitive::ClaimsJsonSerialization,
            AccountWirePrimitive::RequestJsonSerialization,
        ] {
            let (result, consumed) = run_with_account_wire_fault(primitive, || {
                build_account_registration_request(&owner, 1_700_000_000)
            });
            assert!(matches!(
                result,
                Err(McpAccountWireError::JsonSerialization)
            ));
            assert!(consumed, "fault checkpoint consumes {primitive:?}");
        }
        let (result, consumed) =
            run_with_account_wire_fault(AccountWirePrimitive::SigningKeyLoad, || {
                build_account_registration_request(&owner, 1_700_000_000)
            });
        assert!(matches!(result, Err(McpAccountWireError::SigningKeyLoad)));
        assert!(consumed);
        let (result, consumed) =
            run_with_account_wire_fault(AccountWirePrimitive::EcdsaSign, || {
                build_account_registration_request(&owner, 1_700_000_000)
            });
        assert!(matches!(result, Err(McpAccountWireError::EcdsaSign)));
        assert!(consumed);
    }

    #[test]
    fn fault_guard_cleans_up_after_a_panic() {
        let panic = catch_unwind(AssertUnwindSafe(|| {
            let _guard = AccountWireFaultGuard::install(AccountWirePrimitive::EcdsaSign);
            panic!("test panic");
        }));
        assert!(panic.is_err());
        assert!(checkpoint(AccountWirePrimitive::EcdsaSign).is_ok());
    }

    #[test]
    fn account_wire_errors_do_not_render_input_canaries() {
        let (_root, owner) = owner_with_pop(&fixed_pop_pkcs8());
        let canary = owner.committed.instance_id().to_owned();
        let (result, consumed) =
            run_with_account_wire_fault(AccountWirePrimitive::ClaimsJsonSerialization, || {
                build_account_registration_request(&owner, 1_700_000_000)
            });
        let error = match result {
            Ok(_) => panic!("fault injects serialization failure"),
            Err(error) => error,
        };
        let rendered = format!("{error} {error:?}");
        assert!(consumed);
        assert!(!rendered.contains(&canary));
        assert!(
            format!("{}", McpAccountWireError::RequestLengthCap).contains("request"),
            "a known static diagnostic must remain observable"
        );
    }

    #[test]
    fn request_type_has_no_forbidden_derive_surface() {
        let source = include_str!("account_wire.rs");
        let definition = source
            .find("pub(crate) struct McpAccountRequest")
            .expect("request type definition");
        let immediately_before = source[..definition]
            .trim_end()
            .rsplit_once("\n\n")
            .map_or("", |(_, item)| item);
        for forbidden in ["Clone", "Debug", "Display", "Serialize"] {
            assert!(
                !immediately_before.contains("#[derive(")
                    || !immediately_before.contains(forbidden),
                "McpAccountRequest must not derive {forbidden}"
            );
        }
    }

    fn fixture_bytes() -> &'static [u8] {
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/test-fixtures/mcp_bridge_v1.json"
        ))
        .as_bytes()
    }

    fn hex(bytes: &[u8]) -> String {
        let mut text = String::with_capacity(bytes.len() * 2);
        for byte in bytes {
            write!(text, "{byte:02x}").expect("string formats");
        }
        text
    }

    fn fixed_pop_pkcs8() -> Vec<u8> {
        base64::engine::general_purpose::STANDARD
            .decode(FIXED_POP_PKCS8_BASE64)
            .expect("fixed test key decodes")
    }

    fn owner_with_pop(pop_pkcs8: &[u8]) -> (TempDir, McpEndpointOwnerContext) {
        let root = TempDir::new().expect("test root creates");
        write_enabled_config(root.path());
        write_committed_identity(root.path());
        let endpoint = root.path().join("mcp-endpoint");
        fs::create_dir(&endpoint).expect("endpoint directory creates");
        set_mode(&endpoint, 0o700);
        let pop_path = endpoint.join("pop.ed25519.pk8");
        fs::write(&pop_path, pop_pkcs8).expect("fixed PoP key writes");
        set_mode(&pop_path, 0o600);
        let owner = bootstrap_mcp_endpoint_owner_identity(root.path())
            .expect("bootstrap succeeds")
            .expect("enabled endpoint returns owner");
        (root, owner)
    }

    fn write_enabled_config(root: &Path) {
        fs::create_dir_all(root.join("config")).expect("config directory creates");
        fs::write(
            root.join("config/journal.json"),
            br#"{"mcp_endpoint":{"enabled":true}}"#,
        )
        .expect("config writes");
    }

    fn write_committed_identity(root: &Path) {
        let ca = solstone_core_sol_link::ca::generate_ca().expect("CA generates");
        let instance_id =
            solstone_core_sol_link::ca::jid_from_spki(ca.spki_der()).expect("instance ID derives");
        let ca_directory = root.join("link/ca");
        fs::create_dir_all(&ca_directory).expect("CA directory creates");
        fs::write(ca_directory.join("cert.pem"), ca.certificate_pem()).expect("certificate writes");
        fs::write(ca_directory.join("private.pem"), ca.private_key_pem())
            .expect("private key writes");
        fs::write(
            root.join("link/state.json"),
            format!(r#"{{"instance_id":"{instance_id}","home_label":"Primary Home"}}"#),
        )
        .expect("state writes");
    }

    fn set_mode(path: &Path, mode: u32) {
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).expect("mode sets");
    }

    fn assert_valid_account_request(
        body: &[u8],
        expected_instance_id: &str,
        expected_pop_pkcs8: &[u8],
        expected_iat: i64,
        expected_spki_der: &[u8],
    ) {
        let request: serde_json::Value = serde_json::from_slice(body).expect("request JSON");
        assert_object_keys(
            &request,
            &["instance_id", "assertion", "ca_pubkey", "cnf_jwk"],
        );
        assert_eq!(request["instance_id"], expected_instance_id);
        let expected_ca_pubkey = independent_ca_public_key_pem(expected_spki_der);
        let actual_ca_pubkey = request["ca_pubkey"].as_str().expect("CA public key");
        assert_eq!(actual_ca_pubkey, expected_ca_pubkey);
        assert_ne!(actual_ca_pubkey, format!("{expected_ca_pubkey}\n"));
        let cnf_jwk = &request["cnf_jwk"];
        assert_object_keys(cnf_jwk, &["kty", "crv", "x"]);
        assert_eq!(cnf_jwk["kty"], "OKP");
        assert_eq!(cnf_jwk["crv"], "Ed25519");
        let expected_pop = Ed25519KeyPair::from_pkcs8(expected_pop_pkcs8)
            .expect("planted PoP key parses")
            .public_key()
            .as_ref()
            .to_vec();
        let actual_pop = cnf_jwk["x"].as_str().expect("JWK key");
        assert_canonical_base64url(actual_pop);
        assert_eq!(
            actual_pop,
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(expected_pop)
        );

        let assertion = request["assertion"].as_str().expect("assertion");
        let (header_segment, claims_segment, signature_segment) = compact_parts(assertion);
        assert_canonical_base64url(header_segment);
        assert_canonical_base64url(claims_segment);
        assert_canonical_base64url(signature_segment);
        let header: serde_json::Value = serde_json::from_slice(
            &base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(header_segment)
                .expect("header bytes"),
        )
        .expect("header JSON");
        let claims: serde_json::Value = serde_json::from_slice(
            &base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(claims_segment)
                .expect("claims bytes"),
        )
        .expect("claims JSON");
        let signature = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(signature_segment)
            .expect("signature bytes");
        assert_eq!(signature.len(), 64);
        assert_object_keys(&header, &["alg", "typ"]);
        assert_eq!(header["alg"], "ES256");
        assert_eq!(header["typ"], "home-reach");
        assert_object_keys(
            &claims,
            &["iss", "aud", "scope", "instance_id", "iat", "exp"],
        );
        assert_eq!(claims["iss"], format!("home:{expected_instance_id}"));
        assert_eq!(claims["aud"], "solstone-reach");
        assert_eq!(claims["scope"], "mcp.bridge.register");
        assert_eq!(claims["instance_id"], expected_instance_id);
        assert_eq!(claims["iat"], expected_iat);
        assert_eq!(claims["exp"], expected_iat + 240);
        assert_eq!(
            claims["exp"].as_i64().expect("exp") - claims["iat"].as_i64().expect("iat"),
            240
        );
        assert!(signature_verifies(
            expected_spki_der,
            &format!("{header_segment}.{claims_segment}"),
            &signature
        ));
    }

    fn independent_ca_public_key_pem(spki_der: &[u8]) -> String {
        let encoded = base64::engine::general_purpose::STANDARD.encode(spki_der);
        let mut pem = String::from("-----BEGIN PUBLIC KEY-----");
        for line in encoded.as_bytes().chunks(64) {
            pem.push('\n');
            pem.push_str(std::str::from_utf8(line).expect("base64 output is ASCII"));
        }
        pem.push_str("\n-----END PUBLIC KEY-----");
        pem
    }

    fn compact_parts(assertion: &str) -> (&str, &str, &str) {
        let mut parts = assertion.split('.');
        let header = parts.next().expect("header");
        let claims = parts.next().expect("claims");
        let signature = parts.next().expect("signature");
        assert!(parts.next().is_none(), "compact assertion has three parts");
        (header, claims, signature)
    }

    fn assert_canonical_base64url(segment: &str) {
        assert!(!segment.contains('='), "base64url is unpadded");
        let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(segment)
            .expect("base64url decodes");
        assert_eq!(
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(decoded),
            segment
        );
    }

    fn assert_object_keys(value: &serde_json::Value, expected: &[&str]) {
        let keys: Vec<_> = value
            .as_object()
            .expect("JSON object")
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(keys, expected);
    }

    fn pem_spki_der(pem: &str) -> Vec<u8> {
        assert!(pem.starts_with("-----BEGIN PUBLIC KEY-----\n"));
        assert!(pem.ends_with("\n-----END PUBLIC KEY-----"));
        let encoded: String = pem
            .lines()
            .filter(|line| !line.starts_with("-----"))
            .collect();
        base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .expect("PEM base64 decodes")
    }

    fn signature_verifies(spki_der: &[u8], signing_input: &str, signature: &[u8]) -> bool {
        UnparsedPublicKey::new(
            &ECDSA_P256_SHA256_FIXED,
            p256_uncompressed_public_key(spki_der),
        )
        .verify(signing_input.as_bytes(), signature)
        .is_ok()
    }

    fn p256_uncompressed_public_key(spki_der: &[u8]) -> &[u8] {
        assert_eq!(spki_der.len(), P256_SPKI_PREFIX.len() + 65);
        assert_eq!(&spki_der[..P256_SPKI_PREFIX.len()], P256_SPKI_PREFIX);
        &spki_der[P256_SPKI_PREFIX.len()..]
    }

    fn flip_one_bit(segment: &str) -> String {
        let mut decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(segment)
            .expect("base64url decodes");
        decoded[0] ^= 1;
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(decoded)
    }

    #[derive(Clone, Copy)]
    enum ExpectedResponseError {
        BodySize,
        UnexpectedStatus,
        CacheControlMissing,
        CacheControlDuplicate,
        CacheControlNonAscii,
        CacheControlMalformedOws,
        CacheControlWrongDirective,
        CacheControlMultipleDirectives,
        BodyUtf8,
        BodyJson,
        TokenLength,
        TokenType,
        ExpiresIn,
        ExpiresAtLength,
        InstanceIdLength,
        Hostname,
        BridgeId,
        BridgeAddressesCardinality,
        BridgeAddressIpv4,
        BridgeAddressDenied,
    }

    fn assert_response_error(
        result: Result<McpAccountResponseWire, McpAccountResponseWireError>,
        expected: ExpectedResponseError,
    ) {
        let error = match result {
            Ok(_) => panic!("response parser must refuse this input"),
            Err(error) => error,
        };
        assert!(matches!(
            (expected, error),
            (
                ExpectedResponseError::BodySize,
                McpAccountResponseWireError::BodySize
            ) | (
                ExpectedResponseError::UnexpectedStatus,
                McpAccountResponseWireError::UnexpectedStatus
            ) | (
                ExpectedResponseError::CacheControlMissing,
                McpAccountResponseWireError::CacheControlMissing
            ) | (
                ExpectedResponseError::CacheControlDuplicate,
                McpAccountResponseWireError::CacheControlDuplicate
            ) | (
                ExpectedResponseError::CacheControlNonAscii,
                McpAccountResponseWireError::CacheControlNonAscii
            ) | (
                ExpectedResponseError::CacheControlMalformedOws,
                McpAccountResponseWireError::CacheControlMalformedOws
            ) | (
                ExpectedResponseError::CacheControlWrongDirective,
                McpAccountResponseWireError::CacheControlWrongDirective
            ) | (
                ExpectedResponseError::CacheControlMultipleDirectives,
                McpAccountResponseWireError::CacheControlMultipleDirectives
            ) | (
                ExpectedResponseError::BodyUtf8,
                McpAccountResponseWireError::BodyUtf8
            ) | (
                ExpectedResponseError::BodyJson,
                McpAccountResponseWireError::BodyJson
            ) | (
                ExpectedResponseError::TokenLength,
                McpAccountResponseWireError::TokenLength
            ) | (
                ExpectedResponseError::TokenType,
                McpAccountResponseWireError::TokenType
            ) | (
                ExpectedResponseError::ExpiresIn,
                McpAccountResponseWireError::ExpiresIn
            ) | (
                ExpectedResponseError::ExpiresAtLength,
                McpAccountResponseWireError::ExpiresAtLength
            ) | (
                ExpectedResponseError::InstanceIdLength,
                McpAccountResponseWireError::InstanceIdLength
            ) | (
                ExpectedResponseError::Hostname,
                McpAccountResponseWireError::Hostname
            ) | (
                ExpectedResponseError::BridgeId,
                McpAccountResponseWireError::BridgeId
            ) | (
                ExpectedResponseError::BridgeAddressesCardinality,
                McpAccountResponseWireError::BridgeAddressesCardinality
            ) | (
                ExpectedResponseError::BridgeAddressIpv4,
                McpAccountResponseWireError::BridgeAddressIpv4
            ) | (
                ExpectedResponseError::BridgeAddressDenied,
                McpAccountResponseWireError::BridgeAddressDenied
            )
        ));
    }

    fn valid_response_value() -> serde_json::Value {
        serde_json::json!({
            "token": "opaque-bridge-token",
            "token_type": "Bearer",
            "expires_in": 600,
            "expires_at": "2023-11-14T22:23:20Z",
            "instance_id": "8488ae64-b592-80a3-97c6-490e995daa85",
            "hostname": "aaaqeaye.solstone.me",
            "bridge_id": "mcp-bridge-fixture",
            "bridge_addresses": ["20.186.92.169"]
        })
    }

    fn cache_control_headers(value: &[u8]) -> Vec<(Vec<u8>, Vec<u8>)> {
        vec![(b"Cache-Control".to_vec(), value.to_vec())]
    }

    fn response_body(value: &serde_json::Value) -> Vec<u8> {
        serde_json::to_vec(value).expect("response body serializes")
    }

    fn parse_response_value(
        status: u16,
        headers: &[(Vec<u8>, Vec<u8>)],
        value: &serde_json::Value,
    ) -> Result<McpAccountResponseWire, McpAccountResponseWireError> {
        parse_account_registration_response(status, headers, &response_body(value))
    }

    fn assert_response_matches(response: &McpAccountResponseWire, expected: &serde_json::Value) {
        assert_eq!(response.token(), expected["token"].as_str().expect("token"));
        assert_eq!(
            response.expires_in(),
            expected["expires_in"].as_u64().expect("expiry")
        );
        assert_eq!(
            response.expires_at(),
            expected["expires_at"].as_str().expect("expiry timestamp")
        );
        assert_eq!(
            response.instance_id(),
            expected["instance_id"].as_str().expect("instance ID")
        );
        assert_eq!(
            response.hostname(),
            expected["hostname"].as_str().expect("hostname")
        );
        assert_eq!(
            response.bridge_id(),
            expected["bridge_id"].as_str().expect("bridge ID")
        );
        assert_eq!(
            response.bridge_address(),
            expected["bridge_addresses"][0]
                .as_str()
                .expect("bridge address")
        );
    }

    #[test]
    fn response_fixture_round_trip_and_fields_are_independent() {
        let fixture: serde_json::Value =
            serde_json::from_slice(fixture_bytes()).expect("fixture JSON");
        let fixture_response = &fixture["response"];
        let headers = cache_control_headers(
            fixture_response["cache_control"]
                .as_str()
                .expect("fixture cache control")
                .as_bytes(),
        );
        let response = parse_response_value(
            fixture_response["status"].as_u64().expect("fixture status") as u16,
            &headers,
            &fixture_response["body"],
        )
        .expect("fixture response parses");
        assert_response_matches(&response, &fixture_response["body"]);

        let baseline = valid_response_value();
        for (field, replacement) in [
            ("token", serde_json::json!("different-opaque-token")),
            ("expires_in", serde_json::json!(601)),
            ("expires_at", serde_json::json!("another-raw-expiry")),
            ("instance_id", serde_json::json!("another-raw-instance")),
            ("hostname", serde_json::json!("bbbbbbbb.solstone.me")),
            ("bridge_id", serde_json::json!("another-bridge")),
            ("bridge_addresses", serde_json::json!(["8.8.8.8"])),
        ] {
            let mut changed = baseline.clone();
            changed[field] = replacement;
            let response = parse_response_value(200, &cache_control_headers(b"no-store"), &changed)
                .expect("one valid field changes independently");
            assert_response_matches(&response, &changed);
        }
    }

    #[test]
    fn response_status_cache_control_and_body_gates_are_ordered() {
        let body = valid_response_value();
        for status in [0, 199, 201, 404, 500] {
            assert_response_error(
                parse_response_value(status, &[], &body),
                ExpectedResponseError::UnexpectedStatus,
            );
        }

        for (headers, expected) in [
            (vec![], ExpectedResponseError::CacheControlMissing),
            (
                vec![
                    (b"Cache-Control".to_vec(), b"no-store".to_vec()),
                    (b"cache-control".to_vec(), b"no-store".to_vec()),
                ],
                ExpectedResponseError::CacheControlDuplicate,
            ),
            (
                cache_control_headers(&[0xff]),
                ExpectedResponseError::CacheControlNonAscii,
            ),
            (
                cache_control_headers(b"no\tstore"),
                ExpectedResponseError::CacheControlMalformedOws,
            ),
            (
                cache_control_headers(b"no-store\r\n"),
                ExpectedResponseError::CacheControlMalformedOws,
            ),
            (
                cache_control_headers(b"no-cache"),
                ExpectedResponseError::CacheControlWrongDirective,
            ),
            (
                cache_control_headers(b"no-store=1"),
                ExpectedResponseError::CacheControlWrongDirective,
            ),
            (
                cache_control_headers(b"no-store, no-cache"),
                ExpectedResponseError::CacheControlMultipleDirectives,
            ),
        ] {
            assert_response_error(parse_response_value(200, &headers, &body), expected);
        }
        let mixed_case = vec![(b"cAcHe-CoNtRoL".to_vec(), b" \tNo-StOrE\t ".to_vec())];
        assert!(parse_response_value(200, &mixed_case, &body).is_ok());

        let headers = cache_control_headers(b"no-store");
        assert_response_error(
            parse_account_registration_response(200, &headers, b""),
            ExpectedResponseError::BodySize,
        );
        assert_response_error(
            parse_account_registration_response(200, &headers, b"{"),
            ExpectedResponseError::BodyJson,
        );
        let mut boundary = vec![b' '; RESPONSE_BODY_MAX_BYTES];
        boundary[0] = b'{';
        assert_response_error(
            parse_account_registration_response(200, &headers, &boundary),
            ExpectedResponseError::BodyJson,
        );
        assert_response_error(
            parse_account_registration_response(
                200,
                &headers,
                &vec![0xff; RESPONSE_BODY_MAX_BYTES + 1],
            ),
            ExpectedResponseError::BodySize,
        );
    }

    enum ResponseMutation {
        String(&'static str, String),
        Number(&'static str, u64),
        Addresses(Vec<String>),
    }

    fn apply_response_mutation(value: &mut serde_json::Value, mutation: ResponseMutation) {
        match mutation {
            ResponseMutation::String(field, replacement) => {
                value[field] = serde_json::Value::String(replacement);
            }
            ResponseMutation::Number(field, replacement) => {
                value[field] = serde_json::json!(replacement);
            }
            ResponseMutation::Addresses(replacement) => {
                value["bridge_addresses"] = serde_json::json!(replacement);
            }
        }
    }

    #[test]
    fn response_json_and_field_bounds_are_validated() {
        let headers = cache_control_headers(b"no-store");
        let baseline = valid_response_value();
        let mut duplicate = response_body(&baseline);
        duplicate.pop().expect("object closing brace");
        duplicate.extend_from_slice(br#","token":"duplicate"}"#);
        assert_response_error(
            parse_account_registration_response(200, &headers, &duplicate),
            ExpectedResponseError::BodyJson,
        );
        let mut unknown = baseline.clone();
        unknown["unexpected"] = serde_json::json!(true);
        assert_response_error(
            parse_response_value(200, &headers, &unknown),
            ExpectedResponseError::BodyJson,
        );
        let mut wrong_type = baseline.clone();
        wrong_type["expires_in"] = serde_json::json!("600");
        assert_response_error(
            parse_response_value(200, &headers, &wrong_type),
            ExpectedResponseError::BodyJson,
        );
        let mut trailing = response_body(&baseline);
        trailing.extend_from_slice(b" trailing");
        assert_response_error(
            parse_account_registration_response(200, &headers, &trailing),
            ExpectedResponseError::BodyJson,
        );
        assert_response_error(
            parse_account_registration_response(200, &headers, &[0xff]),
            ExpectedResponseError::BodyUtf8,
        );

        let mut passing = Vec::new();
        passing.push(ResponseMutation::String(
            "token",
            "t".repeat(TOKEN_MAX_BYTES),
        ));
        passing.push(ResponseMutation::String("token_type", "Bearer".to_owned()));
        passing.push(ResponseMutation::Number("expires_in", 1));
        passing.push(ResponseMutation::String(
            "expires_at",
            "a".repeat(EXPIRES_AT_MAX_BYTES),
        ));
        passing.push(ResponseMutation::String(
            "instance_id",
            "i".repeat(INSTANCE_ID_MAX_BYTES),
        ));
        passing.push(ResponseMutation::String(
            "hostname",
            "a234567z.solstone.me".to_owned(),
        ));
        passing.push(ResponseMutation::String(
            "bridge_id",
            "b".repeat(BRIDGE_ID_MAX_BYTES),
        ));
        passing.push(ResponseMutation::Addresses(vec!["8.8.8.8".to_owned()]));
        for mutation in passing {
            let mut value = baseline.clone();
            apply_response_mutation(&mut value, mutation);
            assert!(parse_response_value(200, &headers, &value).is_ok());
        }

        let failing = vec![
            (
                ResponseMutation::String("token", String::new()),
                ExpectedResponseError::TokenLength,
            ),
            (
                ResponseMutation::String("token", "t".repeat(TOKEN_MAX_BYTES + 1)),
                ExpectedResponseError::TokenLength,
            ),
            (
                ResponseMutation::String("token_type", "bearer".to_owned()),
                ExpectedResponseError::TokenType,
            ),
            (
                ResponseMutation::Number("expires_in", 0),
                ExpectedResponseError::ExpiresIn,
            ),
            (
                ResponseMutation::String("expires_at", String::new()),
                ExpectedResponseError::ExpiresAtLength,
            ),
            (
                ResponseMutation::String(
                    "expires_at",
                    format!("{}é", "a".repeat(EXPIRES_AT_MAX_BYTES - 1)),
                ),
                ExpectedResponseError::ExpiresAtLength,
            ),
            (
                ResponseMutation::String("instance_id", String::new()),
                ExpectedResponseError::InstanceIdLength,
            ),
            (
                ResponseMutation::String(
                    "instance_id",
                    format!("{}é", "i".repeat(INSTANCE_ID_MAX_BYTES - 1)),
                ),
                ExpectedResponseError::InstanceIdLength,
            ),
            (
                ResponseMutation::Addresses(Vec::new()),
                ExpectedResponseError::BridgeAddressesCardinality,
            ),
            (
                ResponseMutation::Addresses(vec!["8.8.8.8".to_owned(), "1.1.1.1".to_owned()]),
                ExpectedResponseError::BridgeAddressesCardinality,
            ),
        ];
        for (mutation, expected) in failing {
            let mut value = baseline.clone();
            apply_response_mutation(&mut value, mutation);
            assert_response_error(parse_response_value(200, &headers, &value), expected);
        }
    }

    #[test]
    fn response_hostname_and_bridge_id_grammar_is_validated() {
        let headers = cache_control_headers(b"no-store");
        let baseline = valid_response_value();
        for hostname in [
            "short.solstone.me",
            "AAAAAAAA.solstone.me",
            "aaaaaaa0.solstone.me",
            "aaaaaaa1.solstone.me",
            "aaaaaaa8.solstone.me",
            "aaaaaaa9.solstone.me",
            "aaaqeaye.example.com",
        ] {
            let mut value = baseline.clone();
            value["hostname"] = serde_json::json!(hostname);
            assert_response_error(
                parse_response_value(200, &headers, &value),
                ExpectedResponseError::Hostname,
            );
        }
        for bridge_id in [
            String::new(),
            "b".repeat(BRIDGE_ID_MAX_BYTES + 1),
            "has space".to_owned(),
            "bad/name".to_owned(),
            "bad@name".to_owned(),
        ] {
            let mut value = baseline.clone();
            value["bridge_id"] = serde_json::json!(bridge_id);
            assert_response_error(
                parse_response_value(200, &headers, &value),
                ExpectedResponseError::BridgeId,
            );
        }
        for bridge_id in [
            "A".to_owned(),
            "ABC.def_ghi:jkl-123".to_owned(),
            "b".repeat(BRIDGE_ID_MAX_BYTES),
        ] {
            let mut value = baseline.clone();
            value["bridge_id"] = serde_json::json!(bridge_id);
            assert!(parse_response_value(200, &headers, &value).is_ok());
        }
    }

    fn ipv4_u32(address: std::net::Ipv4Addr) -> u32 {
        u32::from_be_bytes(address.octets())
    }

    fn ipv4_text(address: u32) -> String {
        let bytes = address.to_be_bytes();
        std::net::Ipv4Addr::new(bytes[0], bytes[1], bytes[2], bytes[3]).to_string()
    }

    fn expected_v1_denied_ipv4(address: u32, ranges: &[(u32, u32)]) -> bool {
        ranges
            .iter()
            .any(|(start, end)| *start <= address && address <= *end)
    }

    #[test]
    fn response_ipv4_denylist_boundaries_and_neighbors_are_enforced() {
        let expected = [
            (0x0000_0000, 0x00ff_ffff),
            (0x0a00_0000, 0x0aff_ffff),
            (0x6440_0000, 0x647f_ffff),
            (0x7f00_0000, 0x7fff_ffff),
            (0xa9fe_0000, 0xa9fe_ffff),
            (0xac10_0000, 0xac1f_ffff),
            (0xc000_0000, 0xc000_00ff),
            (0xc000_0200, 0xc000_02ff),
            (0xc058_6300, 0xc058_63ff),
            (0xc0a8_0000, 0xc0a8_ffff),
            (0xc612_0000, 0xc613_ffff),
            (0xc633_6400, 0xc633_64ff),
            (0xcb00_7100, 0xcb00_71ff),
            (0xe000_0000, 0xefff_ffff),
            (0xf000_0000, 0xffff_ffff),
        ];
        assert_eq!(V1_DENIED_IPV4_RANGES.len(), expected.len());
        for ((actual_start, actual_end), (expected_start, expected_end)) in
            V1_DENIED_IPV4_RANGES.iter().zip(expected)
        {
            assert_eq!(ipv4_u32(*actual_start), expected_start);
            assert_eq!(ipv4_u32(*actual_end), expected_end);
        }

        let headers = cache_control_headers(b"no-store");
        let baseline = valid_response_value();
        for (start, end) in expected {
            for address in [start, start + (end - start) / 2, end] {
                let mut value = baseline.clone();
                value["bridge_addresses"] = serde_json::json!([ipv4_text(address)]);
                assert_response_error(
                    parse_response_value(200, &headers, &value),
                    ExpectedResponseError::BridgeAddressDenied,
                );
            }
            for neighbor in [start.checked_sub(1), end.checked_add(1)]
                .into_iter()
                .flatten()
            {
                let mut value = baseline.clone();
                value["bridge_addresses"] = serde_json::json!([ipv4_text(neighbor)]);
                if expected_v1_denied_ipv4(neighbor, &expected) {
                    assert_response_error(
                        parse_response_value(200, &headers, &value),
                        ExpectedResponseError::BridgeAddressDenied,
                    );
                } else {
                    assert!(parse_response_value(200, &headers, &value).is_ok());
                }
            }
        }

        for (address, expected_error) in [
            (
                "192.0.0.9",
                Some(ExpectedResponseError::BridgeAddressDenied),
            ),
            (
                "192.0.0.10",
                Some(ExpectedResponseError::BridgeAddressDenied),
            ),
            ("191.255.255.255", None),
            ("192.0.1.0", None),
            ("20.186.92.169", None),
        ] {
            let mut value = baseline.clone();
            value["bridge_addresses"] = serde_json::json!([address]);
            match expected_error {
                Some(expected) => {
                    assert_response_error(parse_response_value(200, &headers, &value), expected)
                }
                None => assert!(parse_response_value(200, &headers, &value).is_ok()),
            }
        }
    }

    #[test]
    fn response_bridge_address_shape_and_cardinality_are_validated() {
        let headers = cache_control_headers(b"no-store");
        let baseline = valid_response_value();
        for address in ["192.0.2.001", "192.0.2", "192.0.2.1.1", ""] {
            let mut value = baseline.clone();
            value["bridge_addresses"] = serde_json::json!([address]);
            assert_response_error(
                parse_response_value(200, &headers, &value),
                ExpectedResponseError::BridgeAddressIpv4,
            );
        }
    }
}
