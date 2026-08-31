// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Compact account-registration wire construction from an admitted owner.

use std::fmt;
use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::Engine as _;
use chrono::{DateTime, SecondsFormat, Utc};
use ring::rand::SystemRandom;
use ring::signature::{ECDSA_P256_SHA256_FIXED_SIGNING, EcdsaKeyPair, KeyPair as _};
use rustls::pki_types::ServerName;
use rustls::{ClientConfig, RootCertStore};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::watch;
use tokio_rustls::TlsConnector;

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
const JWT_SIGNATURE_BYTES: usize = 64;
const JWT_SKEW_SECONDS: i64 = 60;
const JWT_MIN_TTL_SECONDS: i64 = 600;
const JWT_MAX_TTL_SECONDS: i64 = 900;
const ACCOUNT_ORIGIN_HOST: &str = "services.solstone.app";
const ACCOUNT_ORIGIN_PORT: u16 = 443;
const ACCOUNT_REGISTRATION_PATH: &str = "/reach/mcp/bridge-token";
const ACCOUNT_ATTEMPT_TIMEOUT: Duration = Duration::from_secs(10);
const ACCOUNT_CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
const HTTP_HEADER_MAX_BYTES: usize = 32_768;
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

/// A validated, memory-only authority to register one journal with one bridge.
///
/// Its fields deliberately remain private: later tunnel code may carry this
/// opaque value but cannot recover the bearer credential or alter its bindings.
pub(crate) struct McpAccountRegistration {
    token: String,
    hostname: String,
    bridge_id: String,
    bridge_address: String,
    issued_at: i64,
    expires_at: i64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum McpAccountRegistrationError {
    WallBounds,
    CompactJwt,
    JwtEncoding,
    JwtCanonicalEncoding,
    JwtSignatureLength,
    JwtHeader,
    JwtKid,
    JwtPayload,
    JwtIssuer,
    JwtAudience,
    JwtSubject,
    JwtHostname,
    JwtConfirmation,
    JwtPopKey,
    JwtResponseBinding,
    JwtTime,
    JwtTtl,
    ResponseExpiry,
}

impl fmt::Display for McpAccountRegistrationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::WallBounds => "MCP account registration wall bounds are invalid",
            Self::CompactJwt => "MCP account registration token is not a compact JWT",
            Self::JwtEncoding => "MCP account registration token encoding is invalid",
            Self::JwtCanonicalEncoding => {
                "MCP account registration token encoding is not canonical"
            }
            Self::JwtSignatureLength => {
                "MCP account registration token signature has invalid length"
            }
            Self::JwtHeader => "MCP account registration token header is invalid",
            Self::JwtKid => "MCP account registration token key ID is invalid",
            Self::JwtPayload => "MCP account registration token payload is invalid",
            Self::JwtIssuer => "MCP account registration token issuer is invalid",
            Self::JwtAudience => "MCP account registration token audience is invalid",
            Self::JwtSubject => "MCP account registration token subject is invalid",
            Self::JwtHostname => "MCP account registration token hostname is invalid",
            Self::JwtConfirmation => "MCP account registration token confirmation is invalid",
            Self::JwtPopKey => "MCP account registration token proof key is invalid",
            Self::JwtResponseBinding => "MCP account registration response binding is invalid",
            Self::JwtTime => "MCP account registration token time is invalid",
            Self::JwtTtl => "MCP account registration token lifetime is invalid",
            Self::ResponseExpiry => "MCP account registration response expiry is invalid",
        })
    }
}

impl std::error::Error for McpAccountRegistrationError {}

/// A failed fixed-origin account-registration attempt.
///
/// Every variant is deliberately payload-free. In particular, it must never
/// expose account assertions, response tokens, hostnames, addresses, or TLS
/// verification details through diagnostics.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum McpAccountError {
    Cancelled,
    Deadline,
    Clock,
    Request,
    Resolve,
    Connect,
    Tls,
    HttpWrite,
    HttpRead,
    HttpFraming,
    Response,
}

impl fmt::Display for McpAccountError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Cancelled => "MCP account registration was cancelled",
            Self::Deadline => "MCP account registration exceeded its deadline",
            Self::Clock => "MCP account registration clock is unavailable",
            Self::Request => "MCP account registration request could not be built",
            Self::Resolve => "MCP account registration origin could not be resolved",
            Self::Connect => "MCP account registration origin could not be connected",
            Self::Tls => "MCP account registration TLS connection failed",
            Self::HttpWrite => "MCP account registration request write failed",
            Self::HttpRead => "MCP account registration response read failed",
            Self::HttpFraming => "MCP account registration response framing is invalid",
            Self::Response => "MCP account registration response is invalid",
        })
    }
}

impl std::error::Error for McpAccountError {}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RegistrationJwtHeader {
    alg: String,
    typ: String,
    kid: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RegistrationJwtPayload {
    iss: String,
    aud: String,
    sub: String,
    iat: i64,
    exp: i64,
    hostname: String,
    cnf: RegistrationConfirmation,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RegistrationConfirmation {
    jwk: RegistrationJwk,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RegistrationJwk {
    kty: String,
    crv: String,
    x: String,
}

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

/// Perform exactly one cancellable registration request to the account origin.
///
/// This stays private to the account-wire unit: only later same-crate
/// reachability orchestration may carry the resulting opaque registration.
#[allow(dead_code)]
async fn request_account_registration(
    owner: &McpEndpointOwnerContext,
    shutdown: &mut watch::Receiver<bool>,
) -> Result<McpAccountRegistration, McpAccountError> {
    let mut io = TokioAccountAttemptIo;
    run_fixed_account_attempt(owner, shutdown, &mut io, &SystemAccountClock).await
}

trait AccountAttemptIo {
    type Socket;
    type Connection: AsyncRead + AsyncWrite + Unpin;

    async fn resolve(&mut self) -> io::Result<Vec<SocketAddr>>;
    async fn connect(&mut self, address: SocketAddr) -> io::Result<Self::Socket>;
    async fn tls(&mut self, socket: Self::Socket) -> io::Result<Self::Connection>;
}

trait AccountClock {
    fn monotonic_now(&self) -> Instant;
    fn wall_now(&self) -> i64;
}

struct SystemAccountClock;

impl AccountClock for SystemAccountClock {
    fn monotonic_now(&self) -> Instant {
        Instant::now()
    }

    fn wall_now(&self) -> i64 {
        Utc::now().timestamp()
    }
}

struct TokioAccountAttemptIo;

impl AccountAttemptIo for TokioAccountAttemptIo {
    type Socket = tokio::net::TcpStream;
    type Connection = tokio_rustls::client::TlsStream<tokio::net::TcpStream>;

    async fn resolve(&mut self) -> io::Result<Vec<SocketAddr>> {
        tokio::net::lookup_host((ACCOUNT_ORIGIN_HOST, ACCOUNT_ORIGIN_PORT))
            .await
            .map(Iterator::collect)
    }

    async fn connect(&mut self, address: SocketAddr) -> io::Result<Self::Socket> {
        tokio::net::TcpStream::connect(address).await
    }

    async fn tls(&mut self, socket: Self::Socket) -> io::Result<Self::Connection> {
        let server_name = ServerName::try_from(ACCOUNT_ORIGIN_HOST.to_owned())
            .map_err(|_| io::Error::other("fixed account server name is invalid"))?;
        let config = account_tls_config()
            .map_err(|_| io::Error::other("fixed account TLS configuration is unavailable"))?;
        TlsConnector::from(config)
            .connect(server_name, socket)
            .await
    }
}

async fn run_fixed_account_attempt<I: AccountAttemptIo, C: AccountClock>(
    owner: &McpEndpointOwnerContext,
    shutdown: &mut watch::Receiver<bool>,
    io: &mut I,
    clock: &C,
) -> Result<McpAccountRegistration, McpAccountError> {
    if account_shutdown_requested(shutdown) {
        return Err(McpAccountError::Cancelled);
    }
    let deadline = clock.monotonic_now() + ACCOUNT_ATTEMPT_TIMEOUT;
    let wall_start = clock.wall_now();
    let request = build_account_registration_request(owner, wall_start)
        .map_err(|_| McpAccountError::Request)?;
    let addresses = await_account_phase(shutdown, deadline, clock, io.resolve())
        .await
        .map_err(|error| match error {
            McpAccountError::Cancelled | McpAccountError::Deadline => error,
            _ => McpAccountError::Resolve,
        })?;
    let connect_deadline = deadline.min(clock.monotonic_now() + ACCOUNT_CONNECT_TIMEOUT);
    let mut socket = None;
    let mut attempted_address = false;
    for address in addresses {
        attempted_address = true;
        match await_account_phase(shutdown, connect_deadline, clock, io.connect(address)).await {
            Ok(connected) => {
                socket = Some(connected);
                break;
            }
            Err(error @ (McpAccountError::Cancelled | McpAccountError::Deadline)) => {
                return Err(error);
            }
            Err(_) => continue,
        }
    }
    if !attempted_address {
        return Err(McpAccountError::Resolve);
    }
    let socket = socket.ok_or(McpAccountError::Connect)?;
    let tls = await_account_phase(shutdown, deadline, clock, io.tls(socket))
        .await
        .map_err(|error| match error {
            McpAccountError::Cancelled | McpAccountError::Deadline => error,
            _ => McpAccountError::Tls,
        })?;
    let (mut reader, mut writer) = tokio::io::split(tls);
    write_account_request(&mut writer, request.body_bytes(), shutdown, deadline, clock).await?;
    let response = read_account_response(&mut reader, shutdown, deadline, clock).await?;
    account_deadline_remaining(clock, deadline)?;
    let wall_end = clock.wall_now();
    let wire =
        parse_account_registration_response(response.status, &response.headers, &response.body)
            .map_err(|_| McpAccountError::Response)?;
    validate_account_registration(wire, owner, wall_start, wall_end)
        .map_err(|_| McpAccountError::Response)
}

fn account_tls_config() -> Result<Arc<ClientConfig>, McpAccountError> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let roots = RootCertStore::from_iter(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let mut config = ClientConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13, &rustls::version::TLS12])
        .map_err(|_| McpAccountError::Tls)?
        .with_root_certificates(roots)
        .with_no_client_auth();
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(Arc::new(config))
}

fn account_shutdown_requested(shutdown: &watch::Receiver<bool>) -> bool {
    *shutdown.borrow() || shutdown.has_changed().is_err()
}

fn account_deadline_remaining<C: AccountClock>(
    clock: &C,
    deadline: Instant,
) -> Result<Duration, McpAccountError> {
    deadline
        .checked_duration_since(clock.monotonic_now())
        .ok_or(McpAccountError::Deadline)
}

async fn await_account_phase<T, F, C>(
    shutdown: &mut watch::Receiver<bool>,
    deadline: Instant,
    clock: &C,
    future: F,
) -> Result<T, McpAccountError>
where
    F: std::future::Future<Output = std::io::Result<T>>,
    C: AccountClock,
{
    tokio::pin!(future);
    loop {
        if account_shutdown_requested(shutdown) {
            return Err(McpAccountError::Cancelled);
        }
        let remaining = account_deadline_remaining(clock, deadline)?;
        tokio::select! {
            biased;
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow_and_update() {
                    return Err(McpAccountError::Cancelled);
                }
            }
            result = tokio::time::timeout(remaining, &mut future) => {
                let value = result
                    .map_err(|_| McpAccountError::Deadline)?
                    .map_err(|_| McpAccountError::HttpRead)?;
                account_deadline_remaining(clock, deadline)?;
                return Ok(value);
            }
        }
    }
}

async fn write_account_request<W: AsyncWrite + Unpin, C: AccountClock>(
    writer: &mut W,
    body: &[u8],
    shutdown: &mut watch::Receiver<bool>,
    deadline: Instant,
    clock: &C,
) -> Result<(), McpAccountError> {
    let head = format!(
        "POST {ACCOUNT_REGISTRATION_PATH} HTTP/1.1\r\nHost: {ACCOUNT_ORIGIN_HOST}\r\nContent-Type: application/json\r\nAccept: application/json\r\nCache-Control: no-store\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    await_account_phase(shutdown, deadline, clock, writer.write_all(head.as_bytes()))
        .await
        .map_err(|error| match error {
            McpAccountError::Cancelled | McpAccountError::Deadline => error,
            _ => McpAccountError::HttpWrite,
        })?;
    await_account_phase(shutdown, deadline, clock, writer.write_all(body))
        .await
        .map_err(|error| match error {
            McpAccountError::Cancelled | McpAccountError::Deadline => error,
            _ => McpAccountError::HttpWrite,
        })?;
    await_account_phase(shutdown, deadline, clock, writer.flush())
        .await
        .map_err(|error| match error {
            McpAccountError::Cancelled | McpAccountError::Deadline => error,
            _ => McpAccountError::HttpWrite,
        })
}

struct AccountHttpResponse {
    status: u16,
    headers: AccountHttpHeaders,
    body: Vec<u8>,
}

type AccountHttpHeaders = Vec<(Vec<u8>, Vec<u8>)>;

enum AccountHttpBodyFraming {
    ContentLength(usize),
    Chunked,
}

async fn read_account_response<R: AsyncRead + Unpin, C: AccountClock>(
    reader: &mut R,
    shutdown: &mut watch::Receiver<bool>,
    deadline: Instant,
    clock: &C,
) -> Result<AccountHttpResponse, McpAccountError> {
    let head = read_http_head(reader, shutdown, deadline, clock).await?;
    let (status, headers, framing) = parse_http_head(&head)?;
    let body = match framing {
        AccountHttpBodyFraming::ContentLength(length) => {
            read_http_bytes(reader, length, shutdown, deadline, clock).await?
        }
        AccountHttpBodyFraming::Chunked => {
            read_chunked_http_body(reader, shutdown, deadline, clock).await?
        }
    };
    ensure_http_eof(reader, shutdown, deadline, clock).await?;
    Ok(AccountHttpResponse {
        status,
        headers,
        body,
    })
}

async fn read_http_head<R: AsyncRead + Unpin, C: AccountClock>(
    reader: &mut R,
    shutdown: &mut watch::Receiver<bool>,
    deadline: Instant,
    clock: &C,
) -> Result<Vec<u8>, McpAccountError> {
    let mut head = Vec::with_capacity(HTTP_HEADER_MAX_BYTES);
    loop {
        if head.len() == HTTP_HEADER_MAX_BYTES {
            return Err(McpAccountError::HttpFraming);
        }
        head.push(read_http_byte(reader, shutdown, deadline, clock).await?);
        if head.ends_with(b"\r\n\r\n") {
            return Ok(head);
        }
    }
}

async fn read_http_byte<R: AsyncRead + Unpin, C: AccountClock>(
    reader: &mut R,
    shutdown: &mut watch::Receiver<bool>,
    deadline: Instant,
    clock: &C,
) -> Result<u8, McpAccountError> {
    let mut byte = [0_u8; 1];
    match await_account_phase(shutdown, deadline, clock, reader.read(&mut byte)).await? {
        0 => Err(McpAccountError::HttpRead),
        1 => Ok(byte[0]),
        _ => unreachable!("one-byte read cannot exceed its buffer"),
    }
}

async fn read_http_bytes<R: AsyncRead + Unpin, C: AccountClock>(
    reader: &mut R,
    length: usize,
    shutdown: &mut watch::Receiver<bool>,
    deadline: Instant,
    clock: &C,
) -> Result<Vec<u8>, McpAccountError> {
    if length > RESPONSE_BODY_MAX_BYTES {
        return Err(McpAccountError::HttpFraming);
    }
    let mut body = Vec::with_capacity(length);
    for _ in 0..length {
        body.push(read_http_byte(reader, shutdown, deadline, clock).await?);
    }
    Ok(body)
}

async fn read_chunked_http_body<R: AsyncRead + Unpin, C: AccountClock>(
    reader: &mut R,
    shutdown: &mut watch::Receiver<bool>,
    deadline: Instant,
    clock: &C,
) -> Result<Vec<u8>, McpAccountError> {
    let mut body = Vec::with_capacity(RESPONSE_BODY_MAX_BYTES);
    loop {
        let line = read_http_line(reader, shutdown, deadline, HTTP_HEADER_MAX_BYTES, clock).await?;
        let size_text = parse_chunk_size_and_extensions(&line)?;
        if size_text.is_empty() || !size_text.iter().all(u8::is_ascii_hexdigit) {
            return Err(McpAccountError::HttpFraming);
        }
        let chunk_len = usize::from_str_radix(
            std::str::from_utf8(size_text).map_err(|_| McpAccountError::HttpFraming)?,
            16,
        )
        .map_err(|_| McpAccountError::HttpFraming)?;
        if chunk_len > RESPONSE_BODY_MAX_BYTES.saturating_sub(body.len()) {
            return Err(McpAccountError::HttpFraming);
        }
        if chunk_len == 0 {
            loop {
                let trailer =
                    read_http_line(reader, shutdown, deadline, HTTP_HEADER_MAX_BYTES, clock)
                        .await?;
                if trailer.is_empty() {
                    return Ok(body);
                }
                parse_http_field_line(&trailer)?;
            }
        }
        body.extend(read_http_bytes(reader, chunk_len, shutdown, deadline, clock).await?);
        if read_http_byte(reader, shutdown, deadline, clock).await? != b'\r'
            || read_http_byte(reader, shutdown, deadline, clock).await? != b'\n'
        {
            return Err(McpAccountError::HttpFraming);
        }
    }
}

async fn read_http_line<R: AsyncRead + Unpin, C: AccountClock>(
    reader: &mut R,
    shutdown: &mut watch::Receiver<bool>,
    deadline: Instant,
    limit: usize,
    clock: &C,
) -> Result<Vec<u8>, McpAccountError> {
    let mut line = Vec::new();
    loop {
        if line.len() == limit {
            return Err(McpAccountError::HttpFraming);
        }
        let byte = read_http_byte(reader, shutdown, deadline, clock).await?;
        line.push(byte);
        if line.ends_with(b"\r\n") {
            line.truncate(line.len() - 2);
            return Ok(line);
        }
        if byte == b'\n' {
            return Err(McpAccountError::HttpFraming);
        }
    }
}

async fn ensure_http_eof<R: AsyncRead + Unpin, C: AccountClock>(
    reader: &mut R,
    shutdown: &mut watch::Receiver<bool>,
    deadline: Instant,
    clock: &C,
) -> Result<(), McpAccountError> {
    let mut byte = [0_u8; 1];
    match await_account_phase(shutdown, deadline, clock, reader.read(&mut byte)).await? {
        0 => Ok(()),
        _ => Err(McpAccountError::HttpFraming),
    }
}

fn parse_http_head(
    head: &[u8],
) -> Result<(u16, AccountHttpHeaders, AccountHttpBodyFraming), McpAccountError> {
    let content = head
        .strip_suffix(b"\r\n")
        .ok_or(McpAccountError::HttpFraming)?;
    let mut lines = content.split(|byte| *byte == b'\n');
    let status_line = lines.next().ok_or(McpAccountError::HttpFraming)?;
    let status_line = status_line
        .strip_suffix(b"\r")
        .ok_or(McpAccountError::HttpFraming)?;
    let status = parse_http_status(status_line)?;
    if (100..200).contains(&status) {
        return Err(McpAccountError::HttpFraming);
    }
    let mut headers = Vec::new();
    let mut content_length = None;
    let mut chunked = false;
    for line in lines {
        if line.is_empty() {
            continue;
        }
        let line = line
            .strip_suffix(b"\r")
            .ok_or(McpAccountError::HttpFraming)?;
        let (name, value) = parse_http_field_line(line)?;
        if name.eq_ignore_ascii_case(b"content-length") {
            if content_length
                .replace(parse_content_length(trim_http_ows(value))?)
                .is_some()
            {
                return Err(McpAccountError::HttpFraming);
            }
        } else if name.eq_ignore_ascii_case(b"transfer-encoding") {
            if chunked || !trim_http_ows(value).eq_ignore_ascii_case(b"chunked") {
                return Err(McpAccountError::HttpFraming);
            }
            chunked = true;
        } else if name.eq_ignore_ascii_case(b"content-encoding") {
            return Err(McpAccountError::HttpFraming);
        }
        headers.push((name.to_vec(), value.to_vec()));
    }
    let framing = match (content_length, chunked) {
        (Some(_), true) | (None, false) => return Err(McpAccountError::HttpFraming),
        (Some(length), false) => AccountHttpBodyFraming::ContentLength(length),
        (None, true) => AccountHttpBodyFraming::Chunked,
    };
    Ok((status, headers, framing))
}

fn parse_chunk_size_and_extensions(line: &[u8]) -> Result<&[u8], McpAccountError> {
    let mut segments = line.split(|byte| *byte == b';');
    let size = segments.next().unwrap_or_default();
    if size.is_empty() || !size.iter().all(u8::is_ascii_hexdigit) {
        return Err(McpAccountError::HttpFraming);
    }
    for extension in segments {
        parse_chunk_extension(extension)?;
    }
    Ok(size)
}

fn parse_chunk_extension(extension: &[u8]) -> Result<(), McpAccountError> {
    let (name, value) = match extension.iter().position(|byte| *byte == b'=') {
        Some(separator) => (&extension[..separator], Some(&extension[separator + 1..])),
        None => (extension, None),
    };
    if name.is_empty() || !name.iter().all(is_http_token_byte) {
        return Err(McpAccountError::HttpFraming);
    }
    if let Some(value) = value
        && (value.is_empty()
            || (!value.iter().all(is_http_token_byte) && !is_http_quoted_string(value)))
    {
        return Err(McpAccountError::HttpFraming);
    }
    Ok(())
}

fn is_http_quoted_string(value: &[u8]) -> bool {
    if value.len() < 2 || value.first() != Some(&b'"') || value.last() != Some(&b'"') {
        return false;
    }
    let mut escaped = false;
    for byte in &value[1..value.len() - 1] {
        if escaped {
            if *byte < 0x20 || *byte == 0x7f {
                return false;
            }
            escaped = false;
        } else if *byte == b'\\' {
            escaped = true;
        } else if *byte == b'"' || !matches!(*byte, 0x20 | 0x21 | 0x23..=0x5b | 0x5d..=0x7e) {
            return false;
        }
    }
    !escaped
}

fn parse_http_field_line(line: &[u8]) -> Result<(&[u8], &[u8]), McpAccountError> {
    if line.is_empty() || line.starts_with(b" ") || line.starts_with(b"\t") {
        return Err(McpAccountError::HttpFraming);
    }
    let separator = line
        .iter()
        .position(|byte| *byte == b':')
        .ok_or(McpAccountError::HttpFraming)?;
    let (name, value) = (&line[..separator], &line[separator + 1..]);
    if name.is_empty()
        || !name.iter().all(is_http_token_byte)
        || value
            .iter()
            .any(|byte| (*byte < 0x20 && *byte != b'\t') || *byte == 0x7f)
    {
        return Err(McpAccountError::HttpFraming);
    }
    Ok((name, value))
}

fn parse_http_status(line: &[u8]) -> Result<u16, McpAccountError> {
    let code = line
        .strip_prefix(b"HTTP/1.1 ")
        .and_then(|rest| rest.get(..3))
        .filter(|code| code.iter().all(u8::is_ascii_digit))
        .ok_or(McpAccountError::HttpFraming)?;
    if line.len() > 12 && line[12] != b' ' {
        return Err(McpAccountError::HttpFraming);
    }
    std::str::from_utf8(code)
        .ok()
        .and_then(|code| code.parse().ok())
        .ok_or(McpAccountError::HttpFraming)
}

fn parse_content_length(value: &[u8]) -> Result<usize, McpAccountError> {
    if value.is_empty() || !value.iter().all(u8::is_ascii_digit) {
        return Err(McpAccountError::HttpFraming);
    }
    let value = std::str::from_utf8(value).map_err(|_| McpAccountError::HttpFraming)?;
    value.parse().map_err(|_| McpAccountError::HttpFraming)
}

fn trim_http_ows(value: &[u8]) -> &[u8] {
    let start = value
        .iter()
        .position(|byte| !matches!(*byte, b' ' | b'\t'))
        .unwrap_or(value.len());
    let value = &value[start..];
    let end = value
        .iter()
        .rposition(|byte| !matches!(*byte, b' ' | b'\t'))
        .map_or(0, |index| index + 1);
    &value[..end]
}

fn is_http_token_byte(byte: &u8) -> bool {
    matches!(
        *byte,
        b'!' | b'#'..=b'\'' | b'*' | b'+' | b'-' | b'.' | b'^' | b'_' | b'`' | b'|' | b'~'
            | b'0'..=b'9'
            | b'A'..=b'Z'
            | b'a'..=b'z'
    )
}

/// Turn one sealed response wire into the sole opaque registration authority.
///
/// Signature verification intentionally does not happen here: the account
/// transport owns trusted-origin WebPKI and the bridge validates the JWT
/// signature against JWKS before it admits a tunnel. This transition still
/// makes every locally knowable structural, time, owner, and PoP binding exact.
fn validate_account_registration(
    wire: McpAccountResponseWire,
    owner: &McpEndpointOwnerContext,
    wall_start_unix_seconds: i64,
    wall_end_unix_seconds: i64,
) -> Result<McpAccountRegistration, McpAccountRegistrationError> {
    if wall_end_unix_seconds < wall_start_unix_seconds {
        return Err(McpAccountRegistrationError::WallBounds);
    }
    let (header_segment, payload_segment, signature_segment) = compact_jwt_segments(&wire.token)?;
    let header: RegistrationJwtHeader = decode_jwt_json(header_segment)?;
    let payload: RegistrationJwtPayload = decode_jwt_json(payload_segment)?;
    let signature = decode_canonical_base64url(signature_segment)?;
    if signature.len() != JWT_SIGNATURE_BYTES {
        return Err(McpAccountRegistrationError::JwtSignatureLength);
    }
    if header.alg != "EdDSA" || header.typ != "JWT" {
        return Err(McpAccountRegistrationError::JwtHeader);
    }
    if header.kid.is_empty() || has_ecmascript_trim_edge(&header.kid) {
        return Err(McpAccountRegistrationError::JwtKid);
    }

    if payload.iss != "services.solstone.app" {
        return Err(McpAccountRegistrationError::JwtIssuer);
    }
    if payload.aud != wire.bridge_id {
        return Err(McpAccountRegistrationError::JwtAudience);
    }
    let owner_instance_id = owner.committed.instance_id();
    if payload.sub != format!("home:{owner_instance_id}") {
        return Err(McpAccountRegistrationError::JwtSubject);
    }
    if payload.hostname != wire.hostname {
        return Err(McpAccountRegistrationError::JwtHostname);
    }
    if wire.instance_id != owner_instance_id {
        return Err(McpAccountRegistrationError::JwtResponseBinding);
    }
    validate_confirmation_key(&payload.cnf.jwk, owner.keypair.public_key().as_ref())?;

    let earliest_iat = wall_start_unix_seconds
        .checked_sub(JWT_SKEW_SECONDS)
        .ok_or(McpAccountRegistrationError::JwtTime)?;
    let latest_iat = wall_end_unix_seconds
        .checked_add(JWT_SKEW_SECONDS)
        .ok_or(McpAccountRegistrationError::JwtTime)?;
    if !(earliest_iat..=latest_iat).contains(&payload.iat) || payload.exp <= wall_end_unix_seconds {
        return Err(McpAccountRegistrationError::JwtTime);
    }
    let ttl = payload
        .exp
        .checked_sub(payload.iat)
        .ok_or(McpAccountRegistrationError::JwtTtl)?;
    if !(JWT_MIN_TTL_SECONDS..=JWT_MAX_TTL_SECONDS).contains(&ttl) {
        return Err(McpAccountRegistrationError::JwtTtl);
    }
    if wire.expires_in != u64::try_from(ttl).map_err(|_| McpAccountRegistrationError::JwtTtl)? {
        return Err(McpAccountRegistrationError::JwtResponseBinding);
    }
    let expected_expiry = DateTime::<Utc>::from_timestamp(payload.exp, 0)
        .ok_or(McpAccountRegistrationError::ResponseExpiry)?
        .to_rfc3339_opts(SecondsFormat::Secs, true);
    if wire.expires_at != expected_expiry {
        return Err(McpAccountRegistrationError::ResponseExpiry);
    }

    Ok(McpAccountRegistration {
        token: wire.token,
        hostname: wire.hostname,
        bridge_id: wire.bridge_id,
        bridge_address: wire.bridge_address,
        issued_at: payload.iat,
        expires_at: payload.exp,
    })
}

#[cfg(test)]
fn validate_fixture_account_registration(
    wire: McpAccountResponseWire,
    fixture_instance_id: &str,
    fixture_public_key: &[u8],
    wall_start_unix_seconds: i64,
    wall_end_unix_seconds: i64,
) -> Result<McpAccountRegistration, McpAccountRegistrationError> {
    if wall_end_unix_seconds < wall_start_unix_seconds {
        return Err(McpAccountRegistrationError::WallBounds);
    }
    let (header_segment, payload_segment, signature_segment) = compact_jwt_segments(&wire.token)?;
    let header: RegistrationJwtHeader = decode_jwt_json(header_segment)?;
    let payload: RegistrationJwtPayload = decode_jwt_json(payload_segment)?;
    let signature = decode_canonical_base64url(signature_segment)?;
    if signature.len() != JWT_SIGNATURE_BYTES {
        return Err(McpAccountRegistrationError::JwtSignatureLength);
    }
    if header.alg != "EdDSA" || header.typ != "JWT" {
        return Err(McpAccountRegistrationError::JwtHeader);
    }
    if header.kid.is_empty() || has_ecmascript_trim_edge(&header.kid) {
        return Err(McpAccountRegistrationError::JwtKid);
    }
    if payload.iss != "services.solstone.app" {
        return Err(McpAccountRegistrationError::JwtIssuer);
    }
    if payload.aud != wire.bridge_id {
        return Err(McpAccountRegistrationError::JwtAudience);
    }
    if payload.sub != format!("home:{fixture_instance_id}") {
        return Err(McpAccountRegistrationError::JwtSubject);
    }
    if payload.hostname != wire.hostname {
        return Err(McpAccountRegistrationError::JwtHostname);
    }
    if wire.instance_id != fixture_instance_id {
        return Err(McpAccountRegistrationError::JwtResponseBinding);
    }
    validate_confirmation_key(&payload.cnf.jwk, fixture_public_key)?;

    let earliest_iat = wall_start_unix_seconds
        .checked_sub(JWT_SKEW_SECONDS)
        .ok_or(McpAccountRegistrationError::JwtTime)?;
    let latest_iat = wall_end_unix_seconds
        .checked_add(JWT_SKEW_SECONDS)
        .ok_or(McpAccountRegistrationError::JwtTime)?;
    if !(earliest_iat..=latest_iat).contains(&payload.iat) || payload.exp <= wall_end_unix_seconds {
        return Err(McpAccountRegistrationError::JwtTime);
    }
    let ttl = payload
        .exp
        .checked_sub(payload.iat)
        .ok_or(McpAccountRegistrationError::JwtTtl)?;
    if !(JWT_MIN_TTL_SECONDS..=JWT_MAX_TTL_SECONDS).contains(&ttl) {
        return Err(McpAccountRegistrationError::JwtTtl);
    }
    if wire.expires_in != u64::try_from(ttl).map_err(|_| McpAccountRegistrationError::JwtTtl)? {
        return Err(McpAccountRegistrationError::JwtResponseBinding);
    }
    let expected_expiry = DateTime::<Utc>::from_timestamp(payload.exp, 0)
        .ok_or(McpAccountRegistrationError::ResponseExpiry)?
        .to_rfc3339_opts(SecondsFormat::Secs, true);
    if wire.expires_at != expected_expiry {
        return Err(McpAccountRegistrationError::ResponseExpiry);
    }

    Ok(McpAccountRegistration {
        token: wire.token,
        hostname: wire.hostname,
        bridge_id: wire.bridge_id,
        bridge_address: wire.bridge_address,
        issued_at: payload.iat,
        expires_at: payload.exp,
    })
}

fn compact_jwt_segments(token: &str) -> Result<(&str, &str, &str), McpAccountRegistrationError> {
    let mut segments = token.split('.');
    let header = segments
        .next()
        .ok_or(McpAccountRegistrationError::CompactJwt)?;
    let payload = segments
        .next()
        .ok_or(McpAccountRegistrationError::CompactJwt)?;
    let signature = segments
        .next()
        .ok_or(McpAccountRegistrationError::CompactJwt)?;
    if header.is_empty() || payload.is_empty() || signature.is_empty() || segments.next().is_some()
    {
        return Err(McpAccountRegistrationError::CompactJwt);
    }
    Ok((header, payload, signature))
}

fn decode_jwt_json<T: serde::de::DeserializeOwned>(
    segment: &str,
) -> Result<T, McpAccountRegistrationError> {
    let bytes = decode_canonical_base64url(segment)?;
    serde_json::from_slice(&bytes).map_err(|_| McpAccountRegistrationError::JwtPayload)
}

fn decode_canonical_base64url(segment: &str) -> Result<Vec<u8>, McpAccountRegistrationError> {
    let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(segment)
        .map_err(|_| McpAccountRegistrationError::JwtEncoding)?;
    if base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&decoded) != segment {
        return Err(McpAccountRegistrationError::JwtCanonicalEncoding);
    }
    Ok(decoded)
}

fn validate_confirmation_key(
    jwk: &RegistrationJwk,
    owner_public_key: &[u8],
) -> Result<(), McpAccountRegistrationError> {
    if jwk.kty != "OKP" || jwk.crv != "Ed25519" {
        return Err(McpAccountRegistrationError::JwtConfirmation);
    }
    let supplied = decode_canonical_base64url(&jwk.x)?;
    if supplied.len() != 32 || supplied.as_slice() != owner_public_key {
        return Err(McpAccountRegistrationError::JwtPopKey);
    }
    Ok(())
}

fn has_ecmascript_trim_edge(value: &str) -> bool {
    value
        .chars()
        .next()
        .is_some_and(is_ecmascript_trim_character)
        || value
            .chars()
            .next_back()
            .is_some_and(is_ecmascript_trim_character)
}

fn is_ecmascript_trim_character(character: char) -> bool {
    matches!(
        character,
        '\u{0009}'..='\u{000D}'
            | '\u{0020}'
            | '\u{00A0}'
            | '\u{1680}'
            | '\u{2000}'..='\u{200A}'
            | '\u{2028}'
            | '\u{2029}'
            | '\u{202F}'
            | '\u{205F}'
            | '\u{3000}'
            | '\u{FEFF}'
    )
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
    use std::cell::Cell;
    use std::collections::VecDeque;
    use std::fs;
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};
    use std::os::unix::fs::PermissionsExt as _;
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::path::Path;
    use std::rc::Rc;
    use std::task::Poll;

    use base64::Engine as _;
    use ring::digest;
    use ring::rand::SystemRandom;
    use ring::signature::{
        ECDSA_P256_SHA256_FIXED, Ed25519KeyPair, KeyPair as _, UnparsedPublicKey,
    };
    use tempfile::TempDir;
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    use super::*;
    use crate::bootstrap_mcp_endpoint_owner_identity;

    const FIXED_POP_PKCS8_BASE64: &str = "MFECAQEwBQYDK2VwBCIEIK3BpJ7oyV0+ISBYqk1FhL7ddzXR7+nKfGOiBQ658apJgSEAWAnp/vbc7Fjw8uOw1n6YgKEZV+CDrOhYNcO2yPuva30=";
    const P256_SPKI_PREFIX: [u8; 26] = [
        0x30, 0x59, 0x30, 0x13, 0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01, 0x06, 0x08,
        0x2a, 0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07, 0x03, 0x42, 0x00,
    ];

    #[derive(Clone)]
    struct TestAccountClock {
        start: Instant,
        monotonic: Rc<Cell<Instant>>,
        wall: Rc<Cell<i64>>,
    }

    impl TestAccountClock {
        fn new(wall: i64) -> Self {
            let start = Instant::now();
            Self {
                start,
                monotonic: Rc::new(Cell::new(start)),
                wall: Rc::new(Cell::new(wall)),
            }
        }

        fn set_elapsed(&self, elapsed: Duration) {
            self.monotonic.set(self.start + elapsed);
        }
    }

    impl AccountClock for TestAccountClock {
        fn monotonic_now(&self) -> Instant {
            self.monotonic.get()
        }

        fn wall_now(&self) -> i64 {
            self.wall.get()
        }
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum AccountAttemptPhase {
        Resolve,
        Connect,
        Tls,
        RequestHead,
        RequestBody,
        RequestFlush,
        ResponseHead,
        ResponseBody,
        ResponseEof,
    }

    enum AccountAttemptPlan {
        Complete,
        Fail,
        Advance(Duration),
        CancelAndStall(watch::Sender<bool>),
        CloseAndStall(Option<watch::Sender<bool>>),
    }

    struct TestAccountConnectionState {
        clock: TestAccountClock,
        response: Vec<u8>,
        response_head_end: usize,
        response_offset: usize,
        request: Vec<u8>,
        writes: usize,
        active_phase: Option<AccountAttemptPhase>,
        plans: VecDeque<(AccountAttemptPhase, AccountAttemptPlan)>,
        trace: Rc<std::cell::RefCell<Vec<AccountAttemptPhase>>>,
        dropped: bool,
    }

    struct TestAccountConnection {
        state: Rc<std::cell::RefCell<TestAccountConnectionState>>,
    }

    impl TestAccountConnectionState {
        fn phase_poll(&mut self, phase: AccountAttemptPhase) -> Poll<io::Result<()>> {
            if self.active_phase == Some(phase) {
                return Poll::Ready(Ok(()));
            }
            self.active_phase = Some(phase);
            self.trace.borrow_mut().push(phase);
            let plan = if self
                .plans
                .front()
                .is_some_and(|(planned, _)| *planned == phase)
            {
                self.plans
                    .pop_front()
                    .expect("matching connection plan exists")
                    .1
            } else {
                AccountAttemptPlan::Complete
            };
            match plan {
                AccountAttemptPlan::Complete => Poll::Ready(Ok(())),
                AccountAttemptPlan::Fail => {
                    Poll::Ready(Err(io::Error::other("injected account I/O failure")))
                }
                AccountAttemptPlan::Advance(elapsed) => {
                    self.clock.set_elapsed(elapsed);
                    Poll::Ready(Ok(()))
                }
                AccountAttemptPlan::CancelAndStall(sender) => {
                    sender
                        .send(true)
                        .expect("account cancellation receiver remains live");
                    Poll::Pending
                }
                AccountAttemptPlan::CloseAndStall(mut sender) => {
                    drop(sender.take());
                    Poll::Pending
                }
            }
        }
    }

    impl Drop for TestAccountConnection {
        fn drop(&mut self) {
            self.state.borrow_mut().dropped = true;
        }
    }

    impl AsyncRead for TestAccountConnection {
        fn poll_read(
            self: std::pin::Pin<&mut Self>,
            _context: &mut std::task::Context<'_>,
            buffer: &mut tokio::io::ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            let mut state = self.state.borrow_mut();
            let phase = if state.response_offset < state.response_head_end {
                AccountAttemptPhase::ResponseHead
            } else if state.response_offset < state.response.len() {
                AccountAttemptPhase::ResponseBody
            } else {
                AccountAttemptPhase::ResponseEof
            };
            match state.phase_poll(phase) {
                Poll::Ready(Ok(())) => {}
                Poll::Ready(Err(error)) => return Poll::Ready(Err(error)),
                Poll::Pending => return Poll::Pending,
            }
            if state.response_offset == state.response.len() {
                return Poll::Ready(Ok(()));
            }
            let available = &state.response[state.response_offset..];
            let count = available.len().min(buffer.remaining());
            buffer.put_slice(&available[..count]);
            state.response_offset += count;
            Poll::Ready(Ok(()))
        }
    }

    impl AsyncWrite for TestAccountConnection {
        fn poll_write(
            self: std::pin::Pin<&mut Self>,
            _context: &mut std::task::Context<'_>,
            bytes: &[u8],
        ) -> Poll<io::Result<usize>> {
            let mut state = self.state.borrow_mut();
            let phase = if state.writes == 0 {
                AccountAttemptPhase::RequestHead
            } else {
                AccountAttemptPhase::RequestBody
            };
            match state.phase_poll(phase) {
                Poll::Ready(Ok(())) => {}
                Poll::Ready(Err(error)) => return Poll::Ready(Err(error)),
                Poll::Pending => return Poll::Pending,
            }
            state.request.extend_from_slice(bytes);
            state.writes += 1;
            Poll::Ready(Ok(bytes.len()))
        }

        fn poll_flush(
            self: std::pin::Pin<&mut Self>,
            _context: &mut std::task::Context<'_>,
        ) -> Poll<io::Result<()>> {
            self.state
                .borrow_mut()
                .phase_poll(AccountAttemptPhase::RequestFlush)
        }

        fn poll_shutdown(
            self: std::pin::Pin<&mut Self>,
            _context: &mut std::task::Context<'_>,
        ) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }

    struct TestAccountAttemptIo {
        clock: TestAccountClock,
        response: Vec<u8>,
        addresses: Vec<SocketAddr>,
        resolve: AccountAttemptPlan,
        connects: VecDeque<AccountAttemptPlan>,
        tls: AccountAttemptPlan,
        connection_plans: VecDeque<(AccountAttemptPhase, AccountAttemptPlan)>,
        trace: Vec<AccountAttemptPhase>,
        connection: Option<Rc<std::cell::RefCell<TestAccountConnectionState>>>,
    }

    impl TestAccountAttemptIo {
        fn success(clock: TestAccountClock, response: Vec<u8>) -> Self {
            Self {
                clock,
                response,
                addresses: vec![SocketAddr::new(
                    IpAddr::V4(Ipv4Addr::new(192, 0, 2, 9)),
                    ACCOUNT_ORIGIN_PORT,
                )],
                resolve: AccountAttemptPlan::Complete,
                connects: VecDeque::from([AccountAttemptPlan::Complete]),
                tls: AccountAttemptPlan::Complete,
                connection_plans: VecDeque::new(),
                trace: Vec::new(),
                connection: None,
            }
        }

        async fn run_plan(
            &mut self,
            phase: AccountAttemptPhase,
            plan: AccountAttemptPlan,
        ) -> io::Result<()> {
            self.trace.push(phase);
            match plan {
                AccountAttemptPlan::Complete => Ok(()),
                AccountAttemptPlan::Fail => Err(io::Error::other("injected account phase failure")),
                AccountAttemptPlan::Advance(elapsed) => {
                    self.clock.set_elapsed(elapsed);
                    Ok(())
                }
                AccountAttemptPlan::CancelAndStall(sender) => {
                    sender
                        .send(true)
                        .expect("account cancellation receiver remains live");
                    std::future::pending().await
                }
                AccountAttemptPlan::CloseAndStall(mut sender) => {
                    drop(sender.take());
                    std::future::pending().await
                }
            }
        }

        fn request_bytes(&self) -> Vec<u8> {
            self.connection
                .as_ref()
                .expect("test connection remains owned")
                .borrow()
                .request
                .clone()
        }

        fn connection_trace(&self) -> Vec<AccountAttemptPhase> {
            self.connection
                .as_ref()
                .expect("test connection remains owned")
                .borrow()
                .trace
                .borrow()
                .clone()
        }

        fn connection_dropped(&self) -> bool {
            self.connection
                .as_ref()
                .expect("test connection remains owned")
                .borrow()
                .dropped
        }
    }

    impl AccountAttemptIo for TestAccountAttemptIo {
        type Socket = ();
        type Connection = TestAccountConnection;

        async fn resolve(&mut self) -> io::Result<Vec<SocketAddr>> {
            let plan = std::mem::replace(&mut self.resolve, AccountAttemptPlan::Complete);
            self.run_plan(AccountAttemptPhase::Resolve, plan).await?;
            Ok(self.addresses.clone())
        }

        async fn connect(&mut self, _address: SocketAddr) -> io::Result<Self::Socket> {
            let plan = self
                .connects
                .pop_front()
                .unwrap_or(AccountAttemptPlan::Fail);
            self.run_plan(AccountAttemptPhase::Connect, plan).await?;
            Ok(())
        }

        async fn tls(&mut self, _socket: Self::Socket) -> io::Result<Self::Connection> {
            let plan = std::mem::replace(&mut self.tls, AccountAttemptPlan::Complete);
            self.run_plan(AccountAttemptPhase::Tls, plan).await?;
            let response_head_end = self
                .response
                .windows(4)
                .position(|window| window == b"\r\n\r\n")
                .expect("test response has a complete header")
                + 4;
            let state = Rc::new(std::cell::RefCell::new(TestAccountConnectionState {
                clock: self.clock.clone(),
                response: self.response.clone(),
                response_head_end,
                response_offset: 0,
                request: Vec::new(),
                writes: 0,
                active_phase: None,
                plans: std::mem::take(&mut self.connection_plans),
                trace: Rc::new(std::cell::RefCell::new(Vec::new())),
                dropped: false,
            }));
            self.connection = Some(state.clone());
            Ok(TestAccountConnection { state })
        }
    }

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
            text.push_str(&format!("{byte:02x}"));
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

    fn assert_response_error_redacts(
        result: Result<McpAccountResponseWire, McpAccountResponseWireError>,
        expected: ExpectedResponseError,
        canary: &str,
    ) {
        let error = match result {
            Ok(_) => panic!("response parser must refuse canary input"),
            Err(error) => error,
        };
        let display = error.to_string();
        let debug = format!("{error:?}");
        assert!(!display.contains(canary), "display redacts the canary");
        assert!(!debug.contains(canary), "debug redacts the canary");
        assert!(!display.is_empty(), "display retains a static category");
        assert!(!debug.is_empty(), "debug retains a static category");
        assert_response_error(Err(error), expected);
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

    #[test]
    fn response_cache_control_and_json_whitespace_boundaries_are_complete() {
        let value = valid_response_value();
        let body = response_body(&value);

        for cache_control in [
            b"no-store".as_slice(),
            b" NO-STORE ",
            b"\tNo-StOrE\t",
            b" \t no-store \t ",
        ] {
            assert!(
                parse_account_registration_response(
                    200,
                    &cache_control_headers(cache_control),
                    &body,
                )
                .is_ok()
            );
        }

        for cache_control in [
            b"".as_slice(),
            b" ",
            b"\t",
            b"\"no-store\"",
            b"no-store=1",
            b"no-store; private",
            b"no-cache",
            b"private",
            b"public",
            b"max-age=0",
            b"s-maxage=0",
            b"must-revalidate",
            b"extension",
            b"no-store,",
            b",no-store",
            b"no-store,,no-cache",
            b"no-store,no-store",
            b"no-store\r",
            b"no-store\n",
            b"no-store\x0b",
            b"no-store\x0c",
            b"no-store\0",
            b"no-store\x7f",
            b"no\r-store",
            b"no\n-store",
            b"no\x0b-store",
            b"no\x0c-store",
            b"no\0-store",
            b"no\x7f-store",
        ] {
            assert!(
                parse_account_registration_response(
                    200,
                    &cache_control_headers(cache_control),
                    &body,
                )
                .is_err(),
                "Cache-Control value must reject: {cache_control:?}"
            );
        }
        for cache_control in [b"no-store\x80".as_slice(), b"\xffno-store"] {
            assert_response_error(
                parse_account_registration_response(
                    200,
                    &cache_control_headers(cache_control),
                    &body,
                ),
                ExpectedResponseError::CacheControlNonAscii,
            );
        }

        let headers = vec![
            (b"Content-Type".to_vec(), b"application/json".to_vec()),
            (
                b"Strict-Transport-Security".to_vec(),
                b"max-age=31536000".to_vec(),
            ),
            (b"X-Frame-Options".to_vec(), b"DENY".to_vec()),
            (b"X-Content-Type-Options".to_vec(), b"nosniff".to_vec()),
            (b"Referrer-Policy".to_vec(), b"no-referrer".to_vec()),
            (
                b"Permissions-Policy".to_vec(),
                b"interest-cohort=()".to_vec(),
            ),
            (
                b"Content-Security-Policy".to_vec(),
                b"default-src 'none'".to_vec(),
            ),
            (b"X-Repeated".to_vec(), b"one".to_vec()),
            (b"X-Repeated".to_vec(), b"two".to_vec()),
            (b"Cache-Control".to_vec(), b"no-store".to_vec()),
        ];
        assert!(parse_account_registration_response(200, &headers, &body).is_ok());
        let no_cache = &headers[..headers.len() - 1];
        assert_response_error(
            parse_account_registration_response(200, no_cache, &body),
            ExpectedResponseError::CacheControlMissing,
        );

        let internal_whitespace = br#"{
            "token" 	: "opaque-bridge-token" ,
            "token_type":	"Bearer",
            "expires_in" : 600,
            "expires_at": "2023-11-14T22:23:20Z",
            "instance_id":"8488ae64-b592-80a3-97c6-490e995daa85",
            "hostname" : "aaaqeaye.solstone.me",
            "bridge_id":"mcp-bridge-fixture",
            "bridge_addresses" : [ "20.186.92.169" ]
        }"#;
        assert!(
            parse_account_registration_response(
                200,
                &cache_control_headers(b"no-store"),
                internal_whitespace,
            )
            .is_ok()
        );
        let cr_internal = concat!(
            "{\"token\"\r:\r\"opaque-bridge-token\"\r,\r",
            "\"token_type\"\r:\r\"Bearer\"\r,\r",
            "\"expires_in\"\r:\r600\r,\r",
            "\"expires_at\"\r:\r\"2023-11-14T22:23:20Z\"\r,\r",
            "\"instance_id\"\r:\r\"8488ae64-b592-80a3-97c6-490e995daa85\"\r,\r",
            "\"hostname\"\r:\r\"aaaqeaye.solstone.me\"\r,\r",
            "\"bridge_id\"\r:\r\"mcp-bridge-fixture\"\r,\r",
            "\"bridge_addresses\"\r:\r[\"20.186.92.169\"]}"
        );
        assert!(
            parse_account_registration_response(
                200,
                &cache_control_headers(b"no-store"),
                cr_internal.as_bytes(),
            )
            .is_ok()
        );
        for &whitespace in b" \t\n\r" {
            let mut prefixed = vec![whitespace];
            prefixed.extend_from_slice(&body);
            assert!(
                parse_account_registration_response(
                    200,
                    &cache_control_headers(b"no-store"),
                    &prefixed,
                )
                .is_ok()
            );
            let mut suffixed = body.clone();
            suffixed.push(whitespace);
            assert!(
                parse_account_registration_response(
                    200,
                    &cache_control_headers(b"no-store"),
                    &suffixed,
                )
                .is_ok()
            );
        }
        for invalid_whitespace in [b"\x0b".as_slice(), b"\x0c", b"\0", b"\xef\xbb\xbf"] {
            for leading in [true, false] {
                let mut invalid = Vec::new();
                if leading {
                    invalid.extend_from_slice(invalid_whitespace);
                }
                invalid.extend_from_slice(&body);
                if !leading {
                    invalid.extend_from_slice(invalid_whitespace);
                }
                assert_response_error(
                    parse_account_registration_response(
                        200,
                        &cache_control_headers(b"no-store"),
                        &invalid,
                    ),
                    ExpectedResponseError::BodyJson,
                );
            }
        }
        for nonobject in [b"null".as_slice(), b"true", b"1", b"\"text\"", b"[]"] {
            assert_response_error(
                parse_account_registration_response(
                    200,
                    &cache_control_headers(b"no-store"),
                    nonobject,
                ),
                ExpectedResponseError::BodyJson,
            );
        }
        let mut second_value = body.clone();
        second_value.extend_from_slice(b" {} ");
        assert_response_error(
            parse_account_registration_response(
                200,
                &cache_control_headers(b"no-store"),
                &second_value,
            ),
            ExpectedResponseError::BodyJson,
        );

        let mut exact_cap = body.clone();
        exact_cap.resize(RESPONSE_BODY_MAX_BYTES, b' ');
        assert!(
            parse_account_registration_response(
                200,
                &cache_control_headers(b"no-store"),
                &exact_cap,
            )
            .is_ok()
        );
        exact_cap.push(b' ');
        assert_response_error(
            parse_account_registration_response(
                200,
                &cache_control_headers(b"no-store"),
                &exact_cap,
            ),
            ExpectedResponseError::BodySize,
        );
        let mut invalid_utf8_over_cap = vec![b' '; RESPONSE_BODY_MAX_BYTES + 1];
        invalid_utf8_over_cap[0] = 0xff;
        assert_response_error(
            parse_account_registration_response(
                200,
                &cache_control_headers(b"no-store"),
                &invalid_utf8_over_cap,
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

        let passing = vec![
            ResponseMutation::String("token", "t".repeat(TOKEN_MAX_BYTES)),
            ResponseMutation::String("token_type", "Bearer".to_owned()),
            ResponseMutation::Number("expires_in", 1),
            ResponseMutation::String("expires_at", "a".repeat(EXPIRES_AT_MAX_BYTES)),
            ResponseMutation::String("instance_id", "i".repeat(INSTANCE_ID_MAX_BYTES)),
            ResponseMutation::String("hostname", "a234567z.solstone.me".to_owned()),
            ResponseMutation::String("bridge_id", "b".repeat(BRIDGE_ID_MAX_BYTES)),
            ResponseMutation::Addresses(vec!["8.8.8.8".to_owned()]),
        ];
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

    fn response_with_expires_in_literal(literal: &str) -> Vec<u8> {
        let body = String::from_utf8(response_body(&valid_response_value()))
            .expect("valid response JSON is UTF-8");
        let marker = "\"expires_in\":600";
        assert!(body.contains(marker), "expiry marker is present");
        body.replacen(marker, &format!("\"expires_in\":{literal}"), 1)
            .into_bytes()
    }

    #[test]
    fn response_top_level_types_and_byte_bounds_are_complete() {
        let headers = cache_control_headers(b"no-store");
        let baseline = valid_response_value();
        let fields = [
            "token",
            "token_type",
            "expires_in",
            "expires_at",
            "instance_id",
            "hostname",
            "bridge_id",
            "bridge_addresses",
        ];
        for field in fields {
            let mut value = baseline.clone();
            value
                .as_object_mut()
                .expect("response is an object")
                .remove(field);
            assert_response_error(
                parse_response_value(200, &headers, &value),
                ExpectedResponseError::BodyJson,
            );
        }

        let reordered = br#"{
            "bridge_addresses":["20.186.92.169"],
            "bridge_id":"mcp-bridge-fixture",
            "hostname":"aaaqeaye.solstone.me",
            "instance_id":"8488ae64-b592-80a3-97c6-490e995daa85",
            "expires_at":"2023-11-14T22:23:20Z",
            "expires_in":600,
            "token_type":"Bearer",
            "token":"opaque-bridge-token"
        }"#;
        assert!(parse_account_registration_response(200, &headers, reordered).is_ok());

        let non_strings = [
            serde_json::Value::Null,
            serde_json::json!(true),
            serde_json::json!(1),
            serde_json::json!([]),
            serde_json::json!({}),
        ];
        for field in [
            "token",
            "token_type",
            "expires_at",
            "instance_id",
            "hostname",
            "bridge_id",
        ] {
            for wrong_type in &non_strings {
                let mut value = baseline.clone();
                value[field] = wrong_type.clone();
                assert_response_error(
                    parse_response_value(200, &headers, &value),
                    ExpectedResponseError::BodyJson,
                );
            }
        }
        for wrong_type in [
            serde_json::Value::Null,
            serde_json::json!(true),
            serde_json::json!("600"),
            serde_json::json!([]),
            serde_json::json!({}),
        ] {
            let mut value = baseline.clone();
            value["expires_in"] = wrong_type;
            assert_response_error(
                parse_response_value(200, &headers, &value),
                ExpectedResponseError::BodyJson,
            );
        }
        for wrong_type in [
            serde_json::Value::Null,
            serde_json::json!(true),
            serde_json::json!(1),
            serde_json::json!("20.186.92.169"),
            serde_json::json!({}),
        ] {
            let mut value = baseline.clone();
            value["bridge_addresses"] = wrong_type;
            assert_response_error(
                parse_response_value(200, &headers, &value),
                ExpectedResponseError::BodyJson,
            );
        }

        for (literal, expected) in [
            ("-1", ExpectedResponseError::BodyJson),
            ("0", ExpectedResponseError::ExpiresIn),
            ("1", ExpectedResponseError::ExpiresIn),
            ("18446744073709551615", ExpectedResponseError::ExpiresIn),
            ("18446744073709551616", ExpectedResponseError::BodyJson),
            ("1.0", ExpectedResponseError::BodyJson),
        ] {
            let result = parse_account_registration_response(
                200,
                &headers,
                &response_with_expires_in_literal(literal),
            );
            if matches!(literal, "1" | "18446744073709551615") {
                assert!(result.is_ok(), "positive u64 expiry {literal} passes");
            } else {
                assert_response_error(result, expected);
            }
        }

        for (field, passing, failing, expected) in [
            (
                "token",
                vec![
                    "t".to_owned(),
                    "t".repeat(TOKEN_MAX_BYTES),
                    "é".repeat(TOKEN_MAX_BYTES / 2),
                ],
                vec![
                    String::new(),
                    "t".repeat(TOKEN_MAX_BYTES + 1),
                    format!("{}a", "é".repeat(TOKEN_MAX_BYTES / 2)),
                ],
                ExpectedResponseError::TokenLength,
            ),
            (
                "expires_at",
                vec![
                    "x".to_owned(),
                    "x".repeat(EXPIRES_AT_MAX_BYTES),
                    "é".repeat(EXPIRES_AT_MAX_BYTES / 2),
                ],
                vec![
                    String::new(),
                    "x".repeat(EXPIRES_AT_MAX_BYTES + 1),
                    format!("{}a", "é".repeat(EXPIRES_AT_MAX_BYTES / 2)),
                ],
                ExpectedResponseError::ExpiresAtLength,
            ),
            (
                "instance_id",
                vec![
                    "i".to_owned(),
                    "i".repeat(INSTANCE_ID_MAX_BYTES),
                    "é".repeat(INSTANCE_ID_MAX_BYTES / 2),
                ],
                vec![
                    String::new(),
                    "i".repeat(INSTANCE_ID_MAX_BYTES + 1),
                    format!("{}a", "é".repeat(INSTANCE_ID_MAX_BYTES / 2)),
                ],
                ExpectedResponseError::InstanceIdLength,
            ),
        ] {
            for text in passing {
                let mut value = baseline.clone();
                value[field] = serde_json::json!(text);
                assert!(parse_response_value(200, &headers, &value).is_ok());
            }
            for text in failing {
                let mut value = baseline.clone();
                value[field] = serde_json::json!(text);
                assert_response_error(parse_response_value(200, &headers, &value), expected);
            }
        }
        for token_type in ["", "bearer", "BEARER", "Bearer ", " Bearer"] {
            let mut value = baseline.clone();
            value["token_type"] = serde_json::json!(token_type);
            assert_response_error(
                parse_response_value(200, &headers, &value),
                ExpectedResponseError::TokenType,
            );
        }
    }

    #[test]
    fn response_hostname_and_bridge_id_grammar_is_validated() {
        let headers = cache_control_headers(b"no-store");
        let baseline = valid_response_value();
        for hostname in [
            "aaaaaaa.solstone.me",
            "aaaaaaaaa.solstone.me",
            "AAAAAAAA.solstone.me",
            "aaaaaaa0.solstone.me",
            "aaaaaaa1.solstone.me",
            "aaaaaaa8.solstone.me",
            "aaaaaaa9.solstone.me",
            "aaaa-aaa.solstone.me",
            "aaaa_aaa.solstone.me",
            "aaaa.aaa.solstone.me",
            "éaaaaaaa.solstone.me",
            "aaaqeaye.example.com",
            "aaaqeaye.SOLSTONE.ME",
            " aaaqeaye.solstone.me",
            "aaaqeaye.solstone.me ",
            "aa.aeaye.solstone.me",
            "aaaqeaye..solstone.me",
            "aaaqeaye.solstone.me.",
            "aaaqeaye.solstone.me:443",
            "aaaqeaye.solstone.me%zone",
            "prefix-aaaqeaye.solstone.me",
            "aaaqeaye.solstone.me-suffix",
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
            "has\ttab".to_owned(),
            "has\nnewline".to_owned(),
            "bad/name".to_owned(),
            "bad@name".to_owned(),
            "bad\0name".to_owned(),
            "nonascii-é".to_owned(),
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
            "._:-".to_owned(),
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
        for address in [
            "192.0.2.001",
            "192.0.2",
            "192.0.2.1.1",
            "",
            " 8.8.8.8",
            "8.8.8.8 ",
            "x8.8.8.8",
            "8.8.8.8x",
            "2001:4860:4860::8888",
            "8.8.8.8:443",
            "8.8.8.8%eth0",
            "[8.8.8.8]",
        ] {
            let mut value = baseline.clone();
            value["bridge_addresses"] = serde_json::json!([address]);
            assert_response_error(
                parse_response_value(200, &headers, &value),
                ExpectedResponseError::BridgeAddressIpv4,
            );
        }
        for element in [
            serde_json::Value::Null,
            serde_json::json!(true),
            serde_json::json!(8),
            serde_json::json!({}),
            serde_json::json!([]),
        ] {
            let mut value = baseline.clone();
            value["bridge_addresses"] = serde_json::Value::Array(vec![element]);
            assert_response_error(
                parse_response_value(200, &headers, &value),
                ExpectedResponseError::BodyJson,
            );
        }
        let mut value = baseline;
        value["bridge_addresses"] = serde_json::json!(["8.8.8.8"]);
        let response =
            parse_response_value(200, &headers, &value).expect("one allowed string address passes");
        assert_eq!(response.bridge_address(), "8.8.8.8");
    }

    #[test]
    fn response_errors_are_closed_static_and_payload_free() {
        let canary = "secret-response-canary";
        let headers = cache_control_headers(b"no-store");
        let baseline = valid_response_value();

        let mut status_value = baseline.clone();
        status_value["token"] = serde_json::json!(canary);
        assert_response_error_redacts(
            parse_response_value(500, &headers, &status_value),
            ExpectedResponseError::UnexpectedStatus,
            canary,
        );

        assert_response_error_redacts(
            parse_response_value(200, &cache_control_headers(canary.as_bytes()), &baseline),
            ExpectedResponseError::CacheControlWrongDirective,
            canary,
        );
        assert_response_error_redacts(
            parse_account_registration_response(
                200,
                &headers,
                format!("{{\"{canary}\":").as_bytes(),
            ),
            ExpectedResponseError::BodyJson,
            canary,
        );

        for (field, value, expected) in [
            (
                "token",
                format!("{canary}{}", "x".repeat(TOKEN_MAX_BYTES + 1 - canary.len())),
                ExpectedResponseError::TokenLength,
            ),
            (
                "token_type",
                canary.to_owned(),
                ExpectedResponseError::TokenType,
            ),
            (
                "expires_at",
                format!(
                    "{canary}{}",
                    "x".repeat(EXPIRES_AT_MAX_BYTES + 1 - canary.len())
                ),
                ExpectedResponseError::ExpiresAtLength,
            ),
            (
                "instance_id",
                format!(
                    "{canary}{}",
                    "x".repeat(INSTANCE_ID_MAX_BYTES + 1 - canary.len())
                ),
                ExpectedResponseError::InstanceIdLength,
            ),
            (
                "hostname",
                format!("{canary}.solstone.me"),
                ExpectedResponseError::Hostname,
            ),
            (
                "bridge_id",
                format!("{canary}/invalid"),
                ExpectedResponseError::BridgeId,
            ),
        ] {
            let mut changed = baseline.clone();
            changed[field] = serde_json::json!(value);
            assert_response_error_redacts(
                parse_response_value(200, &headers, &changed),
                expected,
                canary,
            );
        }
        let mut expiry = baseline.clone();
        expiry["token"] = serde_json::json!(canary);
        expiry["expires_in"] = serde_json::json!(0);
        assert_response_error_redacts(
            parse_response_value(200, &headers, &expiry),
            ExpectedResponseError::ExpiresIn,
            canary,
        );
        let mut address = baseline.clone();
        address["bridge_addresses"] = serde_json::json!([canary]);
        assert_response_error_redacts(
            parse_response_value(200, &headers, &address),
            ExpectedResponseError::BridgeAddressIpv4,
            canary,
        );

        for (cache_headers, expected) in [
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
                cache_control_headers(b"no\r-store"),
                ExpectedResponseError::CacheControlMalformedOws,
            ),
            (
                cache_control_headers(b"no-cache"),
                ExpectedResponseError::CacheControlWrongDirective,
            ),
            (
                cache_control_headers(b"no-store,private"),
                ExpectedResponseError::CacheControlMultipleDirectives,
            ),
        ] {
            let mut value = baseline.clone();
            value["token"] = serde_json::json!(canary);
            assert_response_error_redacts(
                parse_response_value(200, &cache_headers, &value),
                expected,
                canary,
            );
        }

        let mut cardinality = baseline.clone();
        cardinality["token"] = serde_json::json!(canary);
        cardinality["bridge_addresses"] = serde_json::json!([]);
        assert_response_error_redacts(
            parse_response_value(200, &headers, &cardinality),
            ExpectedResponseError::BridgeAddressesCardinality,
            canary,
        );
        let mut denied = baseline;
        denied["token"] = serde_json::json!(canary);
        denied["bridge_addresses"] = serde_json::json!(["10.0.0.1"]);
        assert_response_error_redacts(
            parse_response_value(200, &headers, &denied),
            ExpectedResponseError::BridgeAddressDenied,
            canary,
        );

        for error in [
            McpAccountResponseWireError::BodySize,
            McpAccountResponseWireError::UnexpectedStatus,
            McpAccountResponseWireError::CacheControlMissing,
            McpAccountResponseWireError::CacheControlDuplicate,
            McpAccountResponseWireError::CacheControlNonAscii,
            McpAccountResponseWireError::CacheControlMalformedOws,
            McpAccountResponseWireError::CacheControlWrongDirective,
            McpAccountResponseWireError::CacheControlMultipleDirectives,
            McpAccountResponseWireError::BodyUtf8,
            McpAccountResponseWireError::BodyJson,
            McpAccountResponseWireError::TokenLength,
            McpAccountResponseWireError::TokenType,
            McpAccountResponseWireError::ExpiresIn,
            McpAccountResponseWireError::ExpiresAtLength,
            McpAccountResponseWireError::InstanceIdLength,
            McpAccountResponseWireError::Hostname,
            McpAccountResponseWireError::BridgeId,
            McpAccountResponseWireError::BridgeAddressesCardinality,
            McpAccountResponseWireError::BridgeAddressIpv4,
            McpAccountResponseWireError::BridgeAddressDenied,
        ] {
            assert!(!error.to_string().contains(canary));
            assert!(!format!("{error:?}").contains(canary));
        }
    }

    const REGISTRATION_WALL_START: i64 = 1_700_000_000;
    const REGISTRATION_WALL_END: i64 = 1_700_000_005;
    const FIXTURE_INSTANCE_ID: &str = "8488ae64-b592-80a3-97c6-490e995daa85";
    const FIXTURE_POP_PUBLIC_KEY: &str = "AsjOOYUMUDDGYVvf2a02SDEXab1H9W3Zvc4WXzymL4c";

    fn registration_expiry(epoch: i64) -> String {
        DateTime::<Utc>::from_timestamp(epoch, 0)
            .expect("fixture epoch is representable")
            .to_rfc3339_opts(SecondsFormat::Secs, true)
    }

    fn valid_registration_payload(
        owner: &McpEndpointOwnerContext,
        iat: i64,
        exp: i64,
    ) -> serde_json::Value {
        serde_json::json!({
            "iss": "services.solstone.app",
            "aud": "mcp-bridge-fixture",
            "sub": format!("home:{}", owner.committed.instance_id()),
            "iat": iat,
            "exp": exp,
            "hostname": "aaaqeaye.solstone.me",
            "cnf": {
                "jwk": {
                    "kty": "OKP",
                    "crv": "Ed25519",
                    "x": base64::engine::general_purpose::URL_SAFE_NO_PAD
                        .encode(owner.keypair.public_key().as_ref()),
                }
            }
        })
    }

    fn signed_registration_token(
        owner: &McpEndpointOwnerContext,
        header: &serde_json::Value,
        payload: &serde_json::Value,
    ) -> String {
        let header = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(serde_json::to_vec(header).expect("header serializes"));
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(serde_json::to_vec(payload).expect("payload serializes"));
        let signing_input = format!("{header}.{payload}");
        let signature = owner.keypair.sign(signing_input.as_bytes());
        format!(
            "{signing_input}.{}",
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(signature.as_ref())
        )
    }

    fn valid_registration_wire(
        owner: &McpEndpointOwnerContext,
        iat: i64,
        exp: i64,
    ) -> McpAccountResponseWire {
        let payload = valid_registration_payload(owner, iat, exp);
        let token = signed_registration_token(
            owner,
            &serde_json::json!({"alg":"EdDSA", "typ":"JWT", "kid":"account-key-1"}),
            &payload,
        );
        McpAccountResponseWire {
            token,
            expires_in: u64::try_from(exp - iat).expect("fixture TTL is positive"),
            expires_at: registration_expiry(exp),
            instance_id: owner.committed.instance_id().to_owned(),
            hostname: "aaaqeaye.solstone.me".to_owned(),
            bridge_id: "mcp-bridge-fixture".to_owned(),
            bridge_address: "20.186.92.169".to_owned(),
        }
    }

    fn account_http_registration_response(owner: &McpEndpointOwnerContext) -> Vec<u8> {
        let wire = valid_registration_wire(
            owner,
            REGISTRATION_WALL_START,
            REGISTRATION_WALL_START + 600,
        );
        let body = serde_json::to_vec(&serde_json::json!({
            "token": wire.token,
            "token_type": "Bearer",
            "expires_in": wire.expires_in,
            "expires_at": wire.expires_at,
            "instance_id": wire.instance_id,
            "hostname": wire.hostname,
            "bridge_id": wire.bridge_id,
            "bridge_addresses": [wire.bridge_address],
        }))
        .expect("test response serializes");
        [
            format!(
                "HTTP/1.1 200 OK\r\nCache-Control: no-store\r\nContent-Length: {}\r\n\r\n",
                body.len()
            )
            .into_bytes(),
            body,
        ]
        .concat()
    }

    fn assert_registration_error(
        result: Result<McpAccountRegistration, McpAccountRegistrationError>,
        expected: McpAccountRegistrationError,
    ) {
        match result {
            Ok(_) => panic!("fixture must refuse"),
            Err(error) => assert_eq!(error, expected),
        }
    }

    fn pinned_fixture_response_wire() -> McpAccountResponseWire {
        let fixture: serde_json::Value =
            serde_json::from_slice(fixture_bytes()).expect("fixture JSON");
        let response = &fixture["response"];
        parse_response_value(
            response["status"]
                .as_u64()
                .expect("fixture response status") as u16,
            &cache_control_headers(
                response["cache_control"]
                    .as_str()
                    .expect("fixture cache control")
                    .as_bytes(),
            ),
            &response["body"],
        )
        .expect("fixture response wire")
    }

    fn fixture_public_key() -> Vec<u8> {
        fixture_decode_canonical_base64url(FIXTURE_POP_PUBLIC_KEY)
            .expect("fixture PoP key is canonical base64url")
    }

    fn fixture_compact_jwt_parts(token: &str) -> Option<(&str, &str, &str)> {
        let mut parts = token.split('.');
        let header = parts.next()?;
        let payload = parts.next()?;
        let signature = parts.next()?;
        if header.is_empty() || payload.is_empty() || signature.is_empty() || parts.next().is_some()
        {
            return None;
        }
        Some((header, payload, signature))
    }

    fn fixture_decode_canonical_base64url(segment: &str) -> Option<Vec<u8>> {
        if segment.is_empty() {
            return None;
        }
        let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(segment)
            .ok()?;
        (base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&decoded) == segment)
            .then_some(decoded)
    }

    fn fixture_jwks_oracle(token: &str, jwks: &serde_json::Value) -> bool {
        let Some((header_segment, payload_segment, signature_segment)) =
            fixture_compact_jwt_parts(token)
        else {
            return false;
        };
        let Some(header_bytes) = fixture_decode_canonical_base64url(header_segment) else {
            return false;
        };
        let Ok(header) = serde_json::from_slice::<serde_json::Value>(&header_bytes) else {
            return false;
        };
        let Some(kid) = header["kid"].as_str() else {
            return false;
        };
        if header["alg"] != "EdDSA" {
            return false;
        }
        let Some(key) = jwks["body"]["keys"].as_array().and_then(|keys| {
            keys.iter().find(|key| {
                key["kid"].as_str() == Some(kid)
                    && key["alg"] == "EdDSA"
                    && key["kty"] == "OKP"
                    && key["crv"] == "Ed25519"
            })
        }) else {
            return false;
        };
        let Some(public_key) = key["x"]
            .as_str()
            .and_then(fixture_decode_canonical_base64url)
        else {
            return false;
        };
        let Some(signature) = fixture_decode_canonical_base64url(signature_segment) else {
            return false;
        };
        ring::signature::UnparsedPublicKey::new(&ring::signature::ED25519, public_key)
            .verify(
                format!("{header_segment}.{payload_segment}").as_bytes(),
                &signature,
            )
            .is_ok()
    }

    fn replace_token_signature(token: &str, mutation: impl FnOnce(&mut [u8])) -> String {
        let (header, payload, signature) = compact_jwt_segments(token).expect("fixture token");
        let mut signature = decode_canonical_base64url(signature).expect("fixture signature");
        mutation(&mut signature);
        format!(
            "{header}.{payload}.{}",
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(signature)
        )
    }

    fn replace_fixture_token_signature(token: &str, mutation: impl FnOnce(&mut [u8])) -> String {
        let (header, payload, signature) = fixture_compact_jwt_parts(token).expect("fixture token");
        let mut signature =
            fixture_decode_canonical_base64url(signature).expect("fixture signature is canonical");
        mutation(&mut signature);
        format!(
            "{header}.{payload}.{}",
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(signature)
        )
    }

    #[test]
    fn validation_turns_only_a_bound_wire_into_an_opaque_registration() {
        let (_root, owner) = owner_with_pop(&fixed_pop_pkcs8());
        let wire = valid_registration_wire(
            &owner,
            REGISTRATION_WALL_START,
            REGISTRATION_WALL_START + 600,
        );
        let expected_token = wire.token.clone();
        let registration = validate_account_registration(
            wire,
            &owner,
            REGISTRATION_WALL_START,
            REGISTRATION_WALL_END,
        )
        .expect("bound registration validates");
        assert_eq!(registration.token, expected_token);
        assert_eq!(registration.hostname, "aaaqeaye.solstone.me");
        assert_eq!(registration.bridge_id, "mcp-bridge-fixture");
        assert_eq!(registration.bridge_address, "20.186.92.169");
        assert_eq!(registration.issued_at, REGISTRATION_WALL_START);
        assert_eq!(registration.expires_at, REGISTRATION_WALL_START + 600);
    }

    #[test]
    fn validation_accepts_a_mutated_signature_until_the_bridge_checks_jwks() {
        let (_root, owner) = owner_with_pop(&fixed_pop_pkcs8());
        let mut wire = valid_registration_wire(
            &owner,
            REGISTRATION_WALL_START,
            REGISTRATION_WALL_START + 600,
        );
        wire.token = replace_token_signature(&wire.token, |signature| signature[0] ^= 1);
        assert!(
            validate_account_registration(
                wire,
                &owner,
                REGISTRATION_WALL_START,
                REGISTRATION_WALL_END,
            )
            .is_ok()
        );
    }

    #[test]
    fn pinned_account_fixture_jwks_oracle_verifies_before_bridge_admission() {
        let fixture: serde_json::Value =
            serde_json::from_slice(fixture_bytes()).expect("fixture JSON");
        let wire = pinned_fixture_response_wire();
        let fixture_pop = fixture_public_key();
        assert!(fixture_jwks_oracle(&wire.token, &fixture["jwks"]));
        assert!(
            validate_fixture_account_registration(
                wire_for(&wire),
                FIXTURE_INSTANCE_ID,
                &fixture_pop,
                REGISTRATION_WALL_START,
                REGISTRATION_WALL_END,
            )
            .is_ok(),
            "the pinned response must bind to its fixed owner"
        );

        let mut mutated = wire;
        mutated.token =
            replace_fixture_token_signature(&mutated.token, |signature| signature[0] ^= 1);
        assert!(
            !fixture_jwks_oracle(&mutated.token, &fixture["jwks"]),
            "the independent fixture JWKS oracle must reject a changed signature"
        );
        assert!(
            validate_fixture_account_registration(
                mutated,
                FIXTURE_INSTANCE_ID,
                &fixture_pop,
                REGISTRATION_WALL_START,
                REGISTRATION_WALL_END,
            )
            .is_ok(),
            "the local transition deliberately leaves issuer signature verification to the bridge"
        );
    }

    #[test]
    fn validation_rejects_jwt_structure_header_and_kid_edges() {
        let (_root, owner) = owner_with_pop(&fixed_pop_pkcs8());
        let payload = valid_registration_payload(
            &owner,
            REGISTRATION_WALL_START,
            REGISTRATION_WALL_START + 600,
        );
        let mut wire = valid_registration_wire(
            &owner,
            REGISTRATION_WALL_START,
            REGISTRATION_WALL_START + 600,
        );
        for token in ["", "one.two", "one.two.three.four", ".one.two"] {
            wire.token = token.to_owned();
            assert_registration_error(
                validate_account_registration(
                    wire_for(&wire),
                    &owner,
                    REGISTRATION_WALL_START,
                    REGISTRATION_WALL_END,
                ),
                McpAccountRegistrationError::CompactJwt,
            );
        }
        for (header, expected) in [
            (
                serde_json::json!({"alg":"HS256", "typ":"JWT", "kid":"key"}),
                McpAccountRegistrationError::JwtHeader,
            ),
            (
                serde_json::json!({"alg":"EdDSA", "typ":"token", "kid":"key"}),
                McpAccountRegistrationError::JwtHeader,
            ),
            (
                serde_json::json!({"alg":"EdDSA", "typ":"JWT", "kid":""}),
                McpAccountRegistrationError::JwtKid,
            ),
            (
                serde_json::json!({"alg":"EdDSA", "typ":"JWT", "kid":"\u{FEFF}key"}),
                McpAccountRegistrationError::JwtKid,
            ),
            (
                serde_json::json!({"alg":"EdDSA", "typ":"JWT", "kid":"key\u{3000}"}),
                McpAccountRegistrationError::JwtKid,
            ),
        ] {
            wire.token = signed_registration_token(&owner, &header, &payload);
            assert_registration_error(
                validate_account_registration(
                    wire_for(&wire),
                    &owner,
                    REGISTRATION_WALL_START,
                    REGISTRATION_WALL_END,
                ),
                expected,
            );
        }
        wire.token = signed_registration_token(
            &owner,
            &serde_json::json!({"alg":"EdDSA", "typ":"JWT", "kid":"\u{0085}key"}),
            &payload,
        );
        assert!(
            validate_account_registration(
                wire,
                &owner,
                REGISTRATION_WALL_START,
                REGISTRATION_WALL_END,
            )
            .is_ok()
        );
    }

    #[test]
    fn validation_enforces_owner_response_and_confirmation_bindings() {
        let (_root, owner) = owner_with_pop(&fixed_pop_pkcs8());
        let baseline = valid_registration_wire(
            &owner,
            REGISTRATION_WALL_START,
            REGISTRATION_WALL_START + 600,
        );
        for (field, value, expected) in [
            (
                "iss",
                serde_json::json!("other"),
                McpAccountRegistrationError::JwtIssuer,
            ),
            (
                "aud",
                serde_json::json!("other"),
                McpAccountRegistrationError::JwtAudience,
            ),
            (
                "sub",
                serde_json::json!("home:other"),
                McpAccountRegistrationError::JwtSubject,
            ),
            (
                "hostname",
                serde_json::json!("bbbbbbbb.solstone.me"),
                McpAccountRegistrationError::JwtHostname,
            ),
        ] {
            let mut payload = valid_registration_payload(
                &owner,
                REGISTRATION_WALL_START,
                REGISTRATION_WALL_START + 600,
            );
            payload[field] = value;
            let mut wire = wire_for(&baseline);
            wire.token = signed_registration_token(
                &owner,
                &serde_json::json!({"alg":"EdDSA", "typ":"JWT", "kid":"key"}),
                &payload,
            );
            assert_registration_error(
                validate_account_registration(
                    wire,
                    &owner,
                    REGISTRATION_WALL_START,
                    REGISTRATION_WALL_END,
                ),
                expected,
            );
        }
        let mut mismatched_response = wire_for(&baseline);
        mismatched_response.instance_id = "other-owner".to_owned();
        assert_registration_error(
            validate_account_registration(
                mismatched_response,
                &owner,
                REGISTRATION_WALL_START,
                REGISTRATION_WALL_END,
            ),
            McpAccountRegistrationError::JwtResponseBinding,
        );
        for (jwk, expected) in [
            (
                serde_json::json!({"kty":"EC","crv":"Ed25519","x":"AA"}),
                McpAccountRegistrationError::JwtConfirmation,
            ),
            (
                serde_json::json!({"kty":"OKP","crv":"Ed25519","x":"AA"}),
                McpAccountRegistrationError::JwtPopKey,
            ),
        ] {
            let mut payload = valid_registration_payload(
                &owner,
                REGISTRATION_WALL_START,
                REGISTRATION_WALL_START + 600,
            );
            payload["cnf"]["jwk"] = jwk;
            let mut wire = wire_for(&baseline);
            wire.token = signed_registration_token(
                &owner,
                &serde_json::json!({"alg":"EdDSA", "typ":"JWT", "kid":"key"}),
                &payload,
            );
            assert_registration_error(
                validate_account_registration(
                    wire,
                    &owner,
                    REGISTRATION_WALL_START,
                    REGISTRATION_WALL_END,
                ),
                expected,
            );
        }
    }

    #[test]
    fn validation_enforces_checked_skew_ttl_and_exact_response_expiry() {
        let (_root, owner) = owner_with_pop(&fixed_pop_pkcs8());
        for (iat, exp, expected) in [
            (
                REGISTRATION_WALL_START - 60,
                REGISTRATION_WALL_START + 600,
                None,
            ),
            (
                REGISTRATION_WALL_END + 60,
                REGISTRATION_WALL_END + 660,
                None,
            ),
            (
                REGISTRATION_WALL_START - 61,
                REGISTRATION_WALL_START + 600,
                Some(McpAccountRegistrationError::JwtTime),
            ),
            (
                REGISTRATION_WALL_END + 61,
                REGISTRATION_WALL_END + 661,
                Some(McpAccountRegistrationError::JwtTime),
            ),
            (
                REGISTRATION_WALL_START,
                REGISTRATION_WALL_START + 599,
                Some(McpAccountRegistrationError::JwtTtl),
            ),
            (
                REGISTRATION_WALL_START,
                REGISTRATION_WALL_START + 901,
                Some(McpAccountRegistrationError::JwtTtl),
            ),
        ] {
            let wire = valid_registration_wire(&owner, iat, exp);
            let result = validate_account_registration(
                wire,
                &owner,
                REGISTRATION_WALL_START,
                REGISTRATION_WALL_END,
            );
            if let Some(expected) = expected {
                assert_registration_error(result, expected);
            } else {
                assert!(result.is_ok(), "boundary token passes");
            }
        }
        let mut wire = valid_registration_wire(
            &owner,
            REGISTRATION_WALL_START,
            REGISTRATION_WALL_START + 600,
        );
        wire.expires_in -= 1;
        assert_registration_error(
            validate_account_registration(
                wire_for(&wire),
                &owner,
                REGISTRATION_WALL_START,
                REGISTRATION_WALL_END,
            ),
            McpAccountRegistrationError::JwtResponseBinding,
        );
        wire.expires_in += 1;
        wire.expires_at = "2023-11-14T22:23:20+00:00".to_owned();
        assert_registration_error(
            validate_account_registration(
                wire,
                &owner,
                REGISTRATION_WALL_START,
                REGISTRATION_WALL_END,
            ),
            McpAccountRegistrationError::ResponseExpiry,
        );
        let valid = valid_registration_wire(
            &owner,
            REGISTRATION_WALL_START,
            REGISTRATION_WALL_START + 600,
        );
        assert_registration_error(
            validate_account_registration(valid, &owner, 1, 0),
            McpAccountRegistrationError::WallBounds,
        );
    }

    fn wire_for(wire: &McpAccountResponseWire) -> McpAccountResponseWire {
        McpAccountResponseWire {
            token: wire.token.clone(),
            expires_in: wire.expires_in,
            expires_at: wire.expires_at.clone(),
            instance_id: wire.instance_id.clone(),
            hostname: wire.hostname.clone(),
            bridge_id: wire.bridge_id.clone(),
            bridge_address: wire.bridge_address.clone(),
        }
    }

    fn account_runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("account test runtime creates")
    }

    #[test]
    fn injected_account_attempt_completes_one_fixed_post_and_returns_registration() {
        let (_root, owner) = owner_with_pop(&fixed_pop_pkcs8());
        let clock = TestAccountClock::new(REGISTRATION_WALL_START);
        let mut io = TestAccountAttemptIo::success(
            clock.clone(),
            account_http_registration_response(&owner),
        );
        let (_sender, mut shutdown) = watch::channel(false);
        let runtime = account_runtime();
        let registration = runtime
            .block_on(run_fixed_account_attempt(
                &owner,
                &mut shutdown,
                &mut io,
                &clock,
            ))
            .expect("injected fixed account attempt succeeds");
        assert_eq!(registration.hostname, "aaaqeaye.solstone.me");
        assert_eq!(
            io.trace,
            vec![
                AccountAttemptPhase::Resolve,
                AccountAttemptPhase::Connect,
                AccountAttemptPhase::Tls,
            ]
        );
        let request = io.request_bytes();
        let (head, body) = request.split_at(
            request
                .windows(4)
                .position(|window| window == b"\r\n\r\n")
                .expect("request has one complete header")
                + 4,
        );
        assert_eq!(
            head,
            format!(
                "POST {ACCOUNT_REGISTRATION_PATH} HTTP/1.1\r\nHost: {ACCOUNT_ORIGIN_HOST}\r\nContent-Type: application/json\r\nAccept: application/json\r\nCache-Control: no-store\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len(),
            )
            .as_bytes(),
        );
        let body: serde_json::Value = serde_json::from_slice(body).expect("request JSON parses");
        assert_eq!(body["instance_id"], owner.committed.instance_id());
        assert_eq!(body["cnf_jwk"]["kty"], "OKP");
        assert_eq!(body["cnf_jwk"]["crv"], "Ed25519");
    }

    #[test]
    fn injected_account_attempt_cancellation_stops_at_the_active_phase() {
        let (_root, owner) = owner_with_pop(&fixed_pop_pkcs8());
        for close_sender in [false, true] {
            let clock = TestAccountClock::new(REGISTRATION_WALL_START);
            let mut io = TestAccountAttemptIo::success(
                clock.clone(),
                account_http_registration_response(&owner),
            );
            let (sender, mut shutdown) = watch::channel(false);
            io.resolve = if close_sender {
                AccountAttemptPlan::CloseAndStall(Some(sender))
            } else {
                AccountAttemptPlan::CancelAndStall(sender)
            };
            let result = account_runtime().block_on(run_fixed_account_attempt(
                &owner,
                &mut shutdown,
                &mut io,
                &clock,
            ));
            assert!(matches!(result, Err(McpAccountError::Cancelled)));
            assert_eq!(io.trace, vec![AccountAttemptPhase::Resolve]);
            assert!(
                io.connection.is_none(),
                "no connection survives cancellation"
            );
        }
    }

    #[test]
    fn injected_connect_and_tls_barriers_cancel_before_any_request() {
        for later_candidate in [false, true] {
            for close_sender in [false, true] {
                let (_root, owner) = owner_with_pop(&fixed_pop_pkcs8());
                let clock = TestAccountClock::new(REGISTRATION_WALL_START);
                let mut io = TestAccountAttemptIo::success(
                    clock.clone(),
                    account_http_registration_response(&owner),
                );
                io.addresses.push(SocketAddr::new(
                    IpAddr::V4(Ipv4Addr::new(192, 0, 2, 10)),
                    ACCOUNT_ORIGIN_PORT,
                ));
                let (sender, mut shutdown) = watch::channel(false);
                let cancellation = if close_sender {
                    AccountAttemptPlan::CloseAndStall(Some(sender))
                } else {
                    AccountAttemptPlan::CancelAndStall(sender)
                };
                io.connects = if later_candidate {
                    VecDeque::from([AccountAttemptPlan::Fail, cancellation])
                } else {
                    VecDeque::from([cancellation, AccountAttemptPlan::Complete])
                };
                let runtime = account_runtime();
                let result = runtime.block_on(async {
                    tokio::time::timeout(
                        Duration::from_secs(1),
                        run_fixed_account_attempt(&owner, &mut shutdown, &mut io, &clock),
                    )
                    .await
                });
                assert!(matches!(result, Ok(Err(McpAccountError::Cancelled))));
                assert_eq!(io.trace.last(), Some(&AccountAttemptPhase::Connect));
                assert!(
                    io.connection.is_none(),
                    "no TLS connection survives connect cancellation"
                );
            }
        }

        for close_sender in [false, true] {
            let (_root, owner) = owner_with_pop(&fixed_pop_pkcs8());
            let clock = TestAccountClock::new(REGISTRATION_WALL_START);
            let mut io = TestAccountAttemptIo::success(
                clock.clone(),
                account_http_registration_response(&owner),
            );
            let (sender, mut shutdown) = watch::channel(false);
            io.tls = if close_sender {
                AccountAttemptPlan::CloseAndStall(Some(sender))
            } else {
                AccountAttemptPlan::CancelAndStall(sender)
            };
            let runtime = account_runtime();
            let result = runtime.block_on(async {
                tokio::time::timeout(
                    Duration::from_secs(1),
                    run_fixed_account_attempt(&owner, &mut shutdown, &mut io, &clock),
                )
                .await
            });
            assert!(matches!(result, Ok(Err(McpAccountError::Cancelled))));
            assert_eq!(io.trace.last(), Some(&AccountAttemptPhase::Tls));
            assert!(
                io.connection.is_none(),
                "no request follows TLS cancellation"
            );
        }
    }

    #[test]
    fn injected_account_io_barriers_cancel_and_drop_without_later_phase_work() {
        for phase in [
            AccountAttemptPhase::RequestHead,
            AccountAttemptPhase::RequestBody,
            AccountAttemptPhase::RequestFlush,
            AccountAttemptPhase::ResponseHead,
            AccountAttemptPhase::ResponseBody,
            AccountAttemptPhase::ResponseEof,
        ] {
            for close_sender in [false, true] {
                let (_root, owner) = owner_with_pop(&fixed_pop_pkcs8());
                let clock = TestAccountClock::new(REGISTRATION_WALL_START);
                let mut io = TestAccountAttemptIo::success(
                    clock.clone(),
                    account_http_registration_response(&owner),
                );
                let (sender, mut shutdown) = watch::channel(false);
                io.connection_plans.push_back((
                    phase,
                    if close_sender {
                        AccountAttemptPlan::CloseAndStall(Some(sender))
                    } else {
                        AccountAttemptPlan::CancelAndStall(sender)
                    },
                ));
                let runtime = account_runtime();
                let result = runtime.block_on(async {
                    tokio::time::timeout(
                        Duration::from_secs(1),
                        run_fixed_account_attempt(&owner, &mut shutdown, &mut io, &clock),
                    )
                    .await
                });
                assert!(matches!(result, Ok(Err(McpAccountError::Cancelled))));
                let trace = io.connection_trace();
                assert_eq!(trace.last(), Some(&phase));
                assert!(io.connection_dropped(), "connection drops on {phase:?}");
            }
        }
    }

    #[test]
    fn injected_account_attempt_retries_only_pre_request_addresses() {
        let (_root, owner) = owner_with_pop(&fixed_pop_pkcs8());
        let clock = TestAccountClock::new(REGISTRATION_WALL_START);
        let mut io = TestAccountAttemptIo::success(
            clock.clone(),
            account_http_registration_response(&owner),
        );
        io.addresses.push(SocketAddr::new(
            IpAddr::V4(Ipv4Addr::new(192, 0, 2, 10)),
            ACCOUNT_ORIGIN_PORT,
        ));
        io.connects = VecDeque::from([AccountAttemptPlan::Fail, AccountAttemptPlan::Complete]);
        let (_sender, mut shutdown) = watch::channel(false);
        let runtime = account_runtime();
        assert!(
            runtime
                .block_on(run_fixed_account_attempt(
                    &owner,
                    &mut shutdown,
                    &mut io,
                    &clock,
                ))
                .is_ok()
        );
        assert_eq!(
            io.trace,
            vec![
                AccountAttemptPhase::Resolve,
                AccountAttemptPhase::Connect,
                AccountAttemptPhase::Connect,
                AccountAttemptPhase::Tls,
            ]
        );
        let request = io.request_bytes();
        assert_eq!(
            request
                .windows(b"POST ".len())
                .filter(|window| *window == b"POST ")
                .count(),
            1,
            "only the connected address receives a request"
        );
    }

    #[test]
    fn injected_account_attempt_never_retries_after_tls_or_shared_deadline_failure() {
        let (_root, owner) = owner_with_pop(&fixed_pop_pkcs8());
        let clock = TestAccountClock::new(REGISTRATION_WALL_START);
        let mut tls_failure = TestAccountAttemptIo::success(
            clock.clone(),
            account_http_registration_response(&owner),
        );
        tls_failure.addresses.push(SocketAddr::new(
            IpAddr::V4(Ipv4Addr::new(192, 0, 2, 10)),
            ACCOUNT_ORIGIN_PORT,
        ));
        tls_failure.tls = AccountAttemptPlan::Fail;
        let (_sender, mut shutdown) = watch::channel(false);
        assert!(matches!(
            account_runtime().block_on(run_fixed_account_attempt(
                &owner,
                &mut shutdown,
                &mut tls_failure,
                &clock,
            )),
            Err(McpAccountError::Tls)
        ));
        assert_eq!(
            tls_failure.trace,
            vec![
                AccountAttemptPhase::Resolve,
                AccountAttemptPhase::Connect,
                AccountAttemptPhase::Tls,
            ]
        );

        let clock = TestAccountClock::new(REGISTRATION_WALL_START);
        let mut deadline = TestAccountAttemptIo::success(
            clock.clone(),
            account_http_registration_response(&owner),
        );
        deadline.resolve =
            AccountAttemptPlan::Advance(ACCOUNT_ATTEMPT_TIMEOUT + Duration::from_millis(1));
        let (_sender, mut shutdown) = watch::channel(false);
        assert!(matches!(
            account_runtime().block_on(run_fixed_account_attempt(
                &owner,
                &mut shutdown,
                &mut deadline,
                &clock,
            )),
            Err(McpAccountError::Deadline)
        ));
        assert_eq!(deadline.trace, vec![AccountAttemptPhase::Resolve]);

        let clock = TestAccountClock::new(REGISTRATION_WALL_START);
        let attempt_deadline = clock.monotonic_now() + ACCOUNT_ATTEMPT_TIMEOUT;
        clock.set_elapsed(ACCOUNT_ATTEMPT_TIMEOUT);
        assert_eq!(
            account_deadline_remaining(&clock, attempt_deadline),
            Ok(Duration::ZERO),
            "completion at the exact deadline remains admissible"
        );
        clock.set_elapsed(ACCOUNT_ATTEMPT_TIMEOUT + Duration::from_nanos(1));
        assert!(matches!(
            account_deadline_remaining(&clock, attempt_deadline),
            Err(McpAccountError::Deadline)
        ));
    }

    #[test]
    fn injected_clock_caps_tcp_candidates_across_one_three_second_window() {
        let (_root, owner) = owner_with_pop(&fixed_pop_pkcs8());
        let clock = TestAccountClock::new(REGISTRATION_WALL_START);
        let mut io = TestAccountAttemptIo::success(
            clock.clone(),
            account_http_registration_response(&owner),
        );
        io.addresses.push(SocketAddr::new(
            IpAddr::V4(Ipv4Addr::new(192, 0, 2, 10)),
            ACCOUNT_ORIGIN_PORT,
        ));
        io.connects = VecDeque::from([
            AccountAttemptPlan::Advance(ACCOUNT_CONNECT_TIMEOUT + Duration::from_nanos(1)),
            AccountAttemptPlan::Complete,
        ]);
        let (_sender, mut shutdown) = watch::channel(false);
        assert!(matches!(
            account_runtime().block_on(run_fixed_account_attempt(
                &owner,
                &mut shutdown,
                &mut io,
                &clock,
            )),
            Err(McpAccountError::Deadline)
        ));
        assert_eq!(
            io.trace,
            vec![AccountAttemptPhase::Resolve, AccountAttemptPhase::Connect],
            "the second candidate never receives a separate three-second budget"
        );
    }

    #[test]
    fn account_http_request_has_one_fixed_origin_and_minimal_http11_headers() {
        let emitted = account_runtime().block_on(async {
            let (mut writer, mut reader) = tokio::io::duplex(4_096);
            let (_sender, mut shutdown) = watch::channel(false);
            write_account_request(
                &mut writer,
                br#"{"fixture":true}"#,
                &mut shutdown,
                Instant::now() + Duration::from_secs(1),
                &SystemAccountClock,
            )
            .await
            .expect("fixed request writes");
            drop(writer);
            let mut bytes = Vec::new();
            reader.read_to_end(&mut bytes).await.expect("request reads");
            bytes
        });
        assert_eq!(
            emitted,
            b"POST /reach/mcp/bridge-token HTTP/1.1\r\nHost: services.solstone.app\r\nContent-Type: application/json\r\nAccept: application/json\r\nCache-Control: no-store\r\nContent-Length: 16\r\nConnection: close\r\n\r\n{\"fixture\":true}"
        );
    }

    #[test]
    fn account_http_framing_rejects_ambiguity_upgrade_and_content_encoding() {
        for head in [
            b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nTransfer-Encoding: chunked\r\n\r\n"
                .as_slice(),
            b"HTTP/1.1 101 Switching Protocols\r\nContent-Length: 0\r\n\r\n".as_slice(),
            b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nContent-Encoding: gzip\r\n\r\n".as_slice(),
            b"HTTP/1.1 200X\r\nContent-Length: 2\r\n\r\n".as_slice(),
        ] {
            assert!(matches!(
                parse_http_head(head),
                Err(McpAccountError::HttpFraming)
            ));
        }
    }

    #[test]
    fn account_http_framing_rejects_malformed_chunk_extensions_and_trailers() {
        for extension in [
            b"1;".as_slice(),
            b"1;=value",
            b"1;name=",
            b"1;name=bad value",
        ] {
            assert!(matches!(
                parse_chunk_size_and_extensions(extension),
                Err(McpAccountError::HttpFraming)
            ));
        }
        assert_eq!(
            parse_chunk_size_and_extensions(b"A;name=value;quoted=\"two words\"")
                .expect("well-formed chunk extensions parse"),
            b"A"
        );
        for trailer in [
            b" leading: value".as_slice(),
            b"no-colon".as_slice(),
            b"bad space: value".as_slice(),
            b"name: value\x01".as_slice(),
        ] {
            assert!(matches!(
                parse_http_field_line(trailer),
                Err(McpAccountError::HttpFraming)
            ));
        }
        assert!(parse_http_field_line(b"X-Trace: bounded\tvalue").is_ok());
    }

    #[test]
    fn account_http_reader_accepts_one_bounded_fixture_response_then_requires_eof() {
        let fixture: serde_json::Value =
            serde_json::from_slice(fixture_bytes()).expect("fixture JSON");
        let body = serde_json::to_vec(&fixture["response"]["body"]).expect("body serializes");
        let response = account_runtime().block_on(async {
            let (mut server, mut client) = tokio::io::duplex(128 * 1024);
            let response = format!(
                "HTTP/1.1 200 OK\r\nCache-Control: no-store\r\nContent-Length: {}\r\n\r\n",
                body.len()
            );
            server
                .write_all(response.as_bytes())
                .await
                .expect("head writes");
            server.write_all(&body).await.expect("body writes");
            drop(server);
            let (_sender, mut shutdown) = watch::channel(false);
            read_account_response(
                &mut client,
                &mut shutdown,
                Instant::now() + Duration::from_secs(1),
                &SystemAccountClock,
            )
            .await
            .expect("bounded response reads")
        });
        let wire =
            parse_account_registration_response(response.status, &response.headers, &response.body)
                .expect("fixture parser receives exact bounded fields");
        assert_eq!(wire.hostname, "aaaqeaye.solstone.me");
        assert_eq!(wire.bridge_address, "20.186.92.169");
    }

    #[test]
    fn account_attempt_cancels_before_request_signing_or_network_work() {
        let (_root, owner) = owner_with_pop(&fixed_pop_pkcs8());
        let (sender, mut shutdown) = watch::channel(true);
        assert!(matches!(
            account_runtime().block_on(request_account_registration(&owner, &mut shutdown)),
            Err(McpAccountError::Cancelled)
        ));
        drop(sender);

        let (_sender, mut shutdown) = watch::channel(false);
        drop(_sender);
        assert!(matches!(
            account_runtime().block_on(request_account_registration(&owner, &mut shutdown)),
            Err(McpAccountError::Cancelled)
        ));
    }

    #[test]
    fn registration_type_and_errors_have_no_payload_egress_surface() {
        let source = include_str!("account_wire.rs");
        let definition = source
            .find("pub(crate) struct McpAccountRegistration")
            .expect("registration definition");
        let immediately_before = source[..definition]
            .trim_end()
            .rsplit_once("\n\n")
            .map_or("", |(_, item)| item);
        for forbidden in ["Clone", "Debug", "Display", "Serialize", "Deserialize"] {
            assert!(
                !immediately_before.contains("#[derive(")
                    || !immediately_before.contains(forbidden),
                "McpAccountRegistration must not derive {forbidden}"
            );
        }
        let canary = "CANARY-REGISTRATION-SECRET";
        for error in [
            McpAccountRegistrationError::WallBounds,
            McpAccountRegistrationError::CompactJwt,
            McpAccountRegistrationError::JwtEncoding,
            McpAccountRegistrationError::JwtCanonicalEncoding,
            McpAccountRegistrationError::JwtSignatureLength,
            McpAccountRegistrationError::JwtHeader,
            McpAccountRegistrationError::JwtKid,
            McpAccountRegistrationError::JwtPayload,
            McpAccountRegistrationError::JwtIssuer,
            McpAccountRegistrationError::JwtAudience,
            McpAccountRegistrationError::JwtSubject,
            McpAccountRegistrationError::JwtHostname,
            McpAccountRegistrationError::JwtConfirmation,
            McpAccountRegistrationError::JwtPopKey,
            McpAccountRegistrationError::JwtResponseBinding,
            McpAccountRegistrationError::JwtTime,
            McpAccountRegistrationError::JwtTtl,
            McpAccountRegistrationError::ResponseExpiry,
        ] {
            assert!(!error.to_string().contains(canary));
            assert!(!format!("{error:?}").contains(canary));
        }
    }
}
