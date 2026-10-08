// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Host helper for signing compact home-reach assertions.

use std::fmt;

use base64::Engine as _;
use ring::rand::SystemRandom;
use ring::signature::{ECDSA_P256_SHA256_FIXED_SIGNING, EcdsaKeyPair};
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

use crate::committed::CommittedIdentity;

const ASSERTION_CAP_BYTES: usize = 8_192;
const ASSERTION_LIFETIME_SECONDS: i64 = 240;

/// The pinned service-enable artifact parsed at compile/runtime.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ServiceEnableProof {
    pub version: u64,
    pub alg: String,
    pub typ: String,
    pub aud: String,
    pub scope: String,
    pub services: Vec<String>,
    pub lifetime_seconds: i64,
    pub clock_skew_seconds: i64,
}

static SERVICE_ENABLE_PROOF: OnceLock<ServiceEnableProof> = OnceLock::new();

pub fn service_enable_proof() -> &'static ServiceEnableProof {
    SERVICE_ENABLE_PROOF.get_or_init(|| {
        serde_json::from_str(include_str!("../../../contracts/service-enable-proof.json"))
            .expect("service-enable-proof.json")
    })
}

/// A signed home-reach assertion and its public verification material.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HomeReachAssertion {
    pub compact: String,
    pub signature: [u8; 64],
    pub ca_pubkey_pem: String,
}

/// Failure while signing a home-reach assertion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HomeReachAssertionError {
    ExpirationOverflow,
    HeaderJsonSerialization,
    ClaimsJsonSerialization,
    SigningKeyLoad,
    EcdsaSign,
    AssertionLengthCap,
    UnlistedService,
}

impl fmt::Display for HomeReachAssertionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::ExpirationOverflow => "home-reach assertion expiration overflowed",
            Self::HeaderJsonSerialization => {
                "home-reach assertion header JSON could not be serialized"
            }
            Self::ClaimsJsonSerialization => {
                "home-reach assertion claims JSON could not be serialized"
            }
            Self::SigningKeyLoad => "home-reach assertion signing key could not be loaded",
            Self::EcdsaSign => "home-reach assertion could not be signed",
            Self::AssertionLengthCap => "home-reach assertion exceeds its size limit",
            Self::UnlistedService => "home-reach assertion service is not listed",
        })
    }
}

impl std::error::Error for HomeReachAssertionError {}

#[derive(Serialize)]
struct ProtectedHeader {
    alg: &'static str,
    typ: &'static str,
}

#[derive(Serialize)]
struct AssertionClaims<'a> {
    iss: String,
    aud: &'static str,
    scope: &'a str,
    instance_id: &'a str,
    iat: i64,
    exp: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    acme_account_uri: Option<&'a str>,
}

#[derive(Serialize)]
struct ServiceEnableClaims<'a> {
    iss: String,
    aud: &'a str,
    scope: &'a str,
    instance_id: &'a str,
    nonce: &'a str,
    service: &'a str,
    iat: i64,
    exp: i64,
}

/// Sign a compact home-reach assertion with the committed CA private key.
pub fn sign_home_reach_assertion(
    scope: &str,
    committed: &CommittedIdentity,
    wall_unix_seconds: i64,
) -> Result<HomeReachAssertion, HomeReachAssertionError> {
    sign_home_reach_assertion_inner(scope, committed, wall_unix_seconds, None)
}

/// Sign a compact home-reach assertion that also carries the journal's ACME
/// account URL as a signed claim, so the service can pin the address's CAA
/// record to that account from the journal's own authenticated request.
pub fn sign_home_reach_assertion_with_acme_account(
    scope: &str,
    committed: &CommittedIdentity,
    wall_unix_seconds: i64,
    acme_account_uri: &str,
) -> Result<HomeReachAssertion, HomeReachAssertionError> {
    sign_home_reach_assertion_inner(scope, committed, wall_unix_seconds, Some(acme_account_uri))
}

fn sign_home_reach_assertion_inner(
    scope: &str,
    committed: &CommittedIdentity,
    wall_unix_seconds: i64,
    acme_account_uri: Option<&str>,
) -> Result<HomeReachAssertion, HomeReachAssertionError> {
    let exp = wall_unix_seconds
        .checked_add(ASSERTION_LIFETIME_SECONDS)
        .ok_or(HomeReachAssertionError::ExpirationOverflow)?;
    let instance_id = committed.instance_id();

    checkpoint(HomeReachFaultPrimitive::HeaderJsonSerialization)?;
    let header_bytes = serde_json::to_vec(&ProtectedHeader {
        alg: "ES256",
        typ: "home-reach",
    })
    .map_err(|_| HomeReachAssertionError::HeaderJsonSerialization)?;
    let header = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&header_bytes);

    checkpoint(HomeReachFaultPrimitive::ClaimsJsonSerialization)?;
    let claims_bytes = serde_json::to_vec(&AssertionClaims {
        iss: format!("home:{instance_id}"),
        aud: "solstone-reach",
        scope,
        instance_id,
        iat: wall_unix_seconds,
        exp,
        acme_account_uri,
    })
    .map_err(|_| HomeReachAssertionError::ClaimsJsonSerialization)?;
    let claims = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&claims_bytes);

    finish_compact_assertion(committed, &header, &claims)
}

/// Sign a service-enable proof assertion for external services portal handoff.
pub fn sign_service_enable_assertion(
    committed: &CommittedIdentity,
    service: &str,
    nonce: &str,
    wall_unix_seconds: i64,
) -> Result<HomeReachAssertion, HomeReachAssertionError> {
    let proof = service_enable_proof();
    if !proof.services.iter().any(|s| s == service) {
        return Err(HomeReachAssertionError::UnlistedService);
    }
    let exp = wall_unix_seconds
        .checked_add(proof.lifetime_seconds)
        .ok_or(HomeReachAssertionError::ExpirationOverflow)?;
    let instance_id = committed.instance_id();

    checkpoint(HomeReachFaultPrimitive::HeaderJsonSerialization)?;
    let header_bytes = serde_json::to_vec(&ProtectedHeader {
        alg: &proof.alg,
        typ: &proof.typ,
    })
    .map_err(|_| HomeReachAssertionError::HeaderJsonSerialization)?;
    let header = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&header_bytes);

    checkpoint(HomeReachFaultPrimitive::ClaimsJsonSerialization)?;
    let claims_bytes = serde_json::to_vec(&ServiceEnableClaims {
        iss: format!("home:{instance_id}"),
        aud: &proof.aud,
        scope: &proof.scope,
        instance_id,
        nonce,
        service,
        iat: wall_unix_seconds,
        exp,
    })
    .map_err(|_| HomeReachAssertionError::ClaimsJsonSerialization)?;
    let claims = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&claims_bytes);

    finish_compact_assertion(committed, &header, &claims)
}

fn finish_compact_assertion(
    committed: &CommittedIdentity,
    header: &str,
    claims: &str,
) -> Result<HomeReachAssertion, HomeReachAssertionError> {
    let signing_input = format!("{header}.{claims}");

    checkpoint(HomeReachFaultPrimitive::SigningKeyLoad)?;
    let ca_key = rcgen::KeyPair::from_pem_and_sign_algo(
        &committed.ca().private_key_pem(),
        &rcgen::PKCS_ECDSA_P256_SHA256,
    )
    .map_err(|_| HomeReachAssertionError::SigningKeyLoad)?;
    let signing_key = EcdsaKeyPair::from_pkcs8(
        &ECDSA_P256_SHA256_FIXED_SIGNING,
        &ca_key.serialize_der(),
        &SystemRandom::new(),
    )
    .map_err(|_| HomeReachAssertionError::SigningKeyLoad)?;

    checkpoint(HomeReachFaultPrimitive::EcdsaSign)?;
    let signature_obj = signing_key
        .sign(&SystemRandom::new(), signing_input.as_bytes())
        .map_err(|_| HomeReachAssertionError::EcdsaSign)?;
    let sig_bytes = signature_obj.as_ref();
    let mut signature = [0u8; 64];
    if sig_bytes.len() != 64 {
        return Err(HomeReachAssertionError::EcdsaSign);
    }
    signature.copy_from_slice(sig_bytes);

    let compact = format!(
        "{signing_input}.{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(signature)
    );
    if compact.len() > ASSERTION_CAP_BYTES {
        return Err(HomeReachAssertionError::AssertionLengthCap);
    }

    let ca_pubkey_pem = ca_public_key_pem(committed.ca().spki_der());

    Ok(HomeReachAssertion {
        compact,
        signature,
        ca_pubkey_pem,
    })
}

/// Format the signed portal handoff URL for service enablement.
pub fn service_enable_portal_url(
    base: &str,
    path_segment: &str,
    nonce: &str,
    instance_id: &str,
    assertion_compact: &str,
    ca_pubkey_pem: &str,
) -> String {
    let trimmed_base = base.trim_end_matches('/');
    let enc_nonce = solstone_core_auth_flow::percent_encode(nonce);
    let enc_instance = solstone_core_auth_flow::percent_encode(instance_id);
    let enc_assertion = solstone_core_auth_flow::percent_encode(assertion_compact);
    let enc_ca_pubkey = solstone_core_auth_flow::percent_encode(ca_pubkey_pem);
    format!(
        "{trimmed_base}/enable/{path_segment}?nonce={enc_nonce}&instance={enc_instance}&assertion={enc_assertion}&ca_pubkey={enc_ca_pubkey}"
    )
}

/// Percent-decode a string value using auth-flow decoder.
pub fn percent_decode(value: &str) -> Result<String, String> {
    solstone_core_auth_flow::percent_decode(value).map_err(|e| e.to_string())
}

#[cfg(any(test, feature = "test-hooks"))]
pub fn verify_service_enable_compact(ca_pubkey_pem: &str, compact: &str) -> bool {
    let Some((signing_input, sig_b64)) = compact.rsplit_once('.') else {
        return false;
    };
    let Ok(sig_bytes) = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(sig_b64) else {
        return false;
    };
    if sig_bytes.len() != 64 {
        return false;
    }
    let Some(spki_der) = spki_der_from_pem(ca_pubkey_pem) else {
        return false;
    };
    if spki_der.len() < P256_SPKI_PREFIX.len() + 64 {
        return false;
    }
    if !spki_der.starts_with(P256_SPKI_PREFIX) {
        return false;
    }
    let raw_pubkey = &spki_der[P256_SPKI_PREFIX.len()..];
    ring::signature::UnparsedPublicKey::new(&ring::signature::ECDSA_P256_SHA256_FIXED, raw_pubkey)
        .verify(signing_input.as_bytes(), &sig_bytes)
        .is_ok()
}

/// Decode the JSON claims of a compact assertion without verifying its signature.
pub fn decode_service_enable_claims(compact: &str) -> Result<serde_json::Value, String> {
    let parts: Vec<&str> = compact.split('.').collect();
    if parts.len() != 3 {
        return Err("expected 3 JWS parts".to_owned());
    }
    let claims_bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(parts[1])
        .map_err(|e| e.to_string())?;
    serde_json::from_slice(&claims_bytes).map_err(|e| e.to_string())
}

#[cfg(any(test, feature = "test-hooks"))]
fn spki_der_from_pem(pem: &str) -> Option<Vec<u8>> {
    let trimmed = pem.trim();
    let body = trimmed
        .strip_prefix("-----BEGIN PUBLIC KEY-----")?
        .strip_suffix("-----END PUBLIC KEY-----")?;
    let b64: String = body.chars().filter(|c| !c.is_whitespace()).collect();
    base64::engine::general_purpose::STANDARD.decode(b64).ok()
}

#[cfg(any(test, feature = "test-hooks"))]
const P256_SPKI_PREFIX: &[u8] = &[
    0x30, 0x59, 0x30, 0x13, 0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01, 0x06, 0x08, 0x2a,
    0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07, 0x03, 0x42, 0x00,
];

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
pub enum HomeReachFaultPrimitive {
    HeaderJsonSerialization,
    ClaimsJsonSerialization,
    SigningKeyLoad,
    EcdsaSign,
}

#[cfg(any(test, feature = "test-hooks"))]
struct HomeReachFault {
    primitive: HomeReachFaultPrimitive,
    consumed: bool,
}

#[cfg(any(test, feature = "test-hooks"))]
thread_local! {
    static HOME_REACH_FAULT: std::cell::RefCell<Option<HomeReachFault>> = const {
        std::cell::RefCell::new(None)
    };
}

#[cfg(any(test, feature = "test-hooks"))]
pub struct HomeReachFaultGuard;

#[cfg(any(test, feature = "test-hooks"))]
impl HomeReachFaultGuard {
    pub fn install(primitive: HomeReachFaultPrimitive) -> Self {
        HOME_REACH_FAULT.with(|fault| {
            assert!(
                fault.borrow().is_none(),
                "home-reach fault is already active"
            );
            *fault.borrow_mut() = Some(HomeReachFault {
                primitive,
                consumed: false,
            });
        });
        Self
    }

    pub fn was_consumed(&self) -> bool {
        HOME_REACH_FAULT.with(|fault| {
            fault
                .borrow()
                .as_ref()
                .expect("home-reach fault remains active")
                .consumed
        })
    }
}

#[cfg(any(test, feature = "test-hooks"))]
impl Drop for HomeReachFaultGuard {
    fn drop(&mut self) {
        HOME_REACH_FAULT.with(|fault| {
            fault
                .borrow_mut()
                .take()
                .expect("home-reach fault remains active");
        });
    }
}

#[cfg(any(test, feature = "test-hooks"))]
pub fn run_with_home_reach_fault<T>(
    primitive: HomeReachFaultPrimitive,
    op: impl FnOnce() -> T,
) -> (T, bool) {
    let guard = HomeReachFaultGuard::install(primitive);
    let result = op();
    (result, guard.was_consumed())
}

#[cfg(any(test, feature = "test-hooks"))]
fn checkpoint(primitive: HomeReachFaultPrimitive) -> Result<(), HomeReachAssertionError> {
    HOME_REACH_FAULT.with(|fault| {
        let mut fault = fault.borrow_mut();
        let Some(fault) = fault.as_mut() else {
            return Ok(());
        };
        if fault.primitive != primitive || fault.consumed {
            return Ok(());
        }
        fault.consumed = true;
        Err(match primitive {
            HomeReachFaultPrimitive::HeaderJsonSerialization => {
                HomeReachAssertionError::HeaderJsonSerialization
            }
            HomeReachFaultPrimitive::ClaimsJsonSerialization => {
                HomeReachAssertionError::ClaimsJsonSerialization
            }
            HomeReachFaultPrimitive::SigningKeyLoad => HomeReachAssertionError::SigningKeyLoad,
            HomeReachFaultPrimitive::EcdsaSign => HomeReachAssertionError::EcdsaSign,
        })
    })
}

#[cfg(not(any(test, feature = "test-hooks")))]
fn checkpoint(_primitive: HomeReachFaultPrimitive) -> Result<(), HomeReachAssertionError> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    use base64::Engine as _;
    use ring::digest;
    use ring::signature::{ECDSA_P256_SHA256_FIXED, UnparsedPublicKey};

    use super::*;
    use crate::ca::{generate_ca, jid_from_spki};
    use crate::committed::load_committed_identity;

    static SEQUENCE: AtomicU64 = AtomicU64::new(0);

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock is after epoch")
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "solstone-home-reach-{}-{nanos}-{sequence}",
                std::process::id()
            ));
            fs::create_dir_all(&path).expect("temporary root creates");
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn write_committed_fixture(root: &Path) -> (CommittedIdentity, Vec<u8>) {
        let ca = generate_ca().expect("CA generates");
        let spki_der = ca.spki_der().to_vec();
        let instance_id = jid_from_spki(&spki_der).expect("JID derives");
        let ca_dir = root.join("link/ca");
        fs::create_dir_all(&ca_dir).expect("CA dir creates");
        fs::write(ca_dir.join("cert.pem"), ca.certificate_pem()).expect("cert writes");
        fs::write(ca_dir.join("private.pem"), ca.private_key_pem()).expect("key writes");
        fs::write(
            root.join("link/state.json"),
            format!(r#"{{"instance_id":"{instance_id}","home_label":"Test Home"}}"#),
        )
        .expect("state writes");
        let committed = load_committed_identity(root).expect("committed identity loads");
        (committed, spki_der)
    }

    fn p256_uncompressed_public_key(spki_der: &[u8]) -> &[u8] {
        assert_eq!(spki_der.len(), P256_SPKI_PREFIX.len() + 65);
        assert_eq!(&spki_der[..P256_SPKI_PREFIX.len()], P256_SPKI_PREFIX);
        &spki_der[P256_SPKI_PREFIX.len()..]
    }

    fn verify_sig(spki_der: &[u8], signing_input: &str, signature: &[u8]) -> bool {
        UnparsedPublicKey::new(
            &ECDSA_P256_SHA256_FIXED,
            p256_uncompressed_public_key(spki_der),
        )
        .verify(signing_input.as_bytes(), signature)
        .is_ok()
    }

    fn flip_one_bit(segment: &str) -> String {
        let mut decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(segment)
            .expect("base64url decodes");
        decoded[0] ^= 1;
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(decoded)
    }

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    fn claims_of(compact: &str) -> (String, serde_json::Value) {
        let parts: Vec<&str> = compact.split('.').collect();
        let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(parts[1])
            .expect("claims decodes");
        (
            format!("{}.{}", parts[0], parts[1]),
            serde_json::from_slice(&bytes).expect("claims JSON"),
        )
    }

    #[test]
    fn signs_valid_assertion_and_verifies_with_oracle() {
        let temp = TempDir::new();
        let (committed, spki_der) = write_committed_fixture(temp.path());
        let wall = 1_700_000_000_i64;
        let scope = "push.relay.enroll";

        let assertion =
            sign_home_reach_assertion(scope, &committed, wall).expect("assertion signs");

        let expected_pem = {
            let encoded = base64::engine::general_purpose::STANDARD.encode(&spki_der);
            let mut pem = String::from("-----BEGIN PUBLIC KEY-----");
            for chunk in encoded.as_bytes().chunks(64) {
                pem.push('\n');
                pem.push_str(std::str::from_utf8(chunk).unwrap());
            }
            pem.push_str("\n-----END PUBLIC KEY-----");
            pem
        };
        assert_eq!(assertion.ca_pubkey_pem, expected_pem);

        let parts: Vec<&str> = assertion.compact.split('.').collect();
        assert_eq!(parts.len(), 3);
        let header_segment = parts[0];
        let claims_segment = parts[1];
        let sig_segment = parts[2];

        let header_bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(header_segment)
            .expect("header decodes");
        let header: serde_json::Value = serde_json::from_slice(&header_bytes).expect("header JSON");
        assert_eq!(header["alg"], "ES256");
        assert_eq!(header["typ"], "home-reach");

        let claims_bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(claims_segment)
            .expect("claims decodes");
        let claims: serde_json::Value = serde_json::from_slice(&claims_bytes).expect("claims JSON");
        assert_eq!(claims["iss"], format!("home:{}", committed.instance_id()));
        assert_eq!(claims["aud"], "solstone-reach");
        assert_eq!(claims["scope"], scope);
        assert_eq!(claims["instance_id"], committed.instance_id());
        assert_eq!(claims["iat"], wall);
        assert_eq!(claims["exp"], wall + 240);

        let signing_input = format!("{header_segment}.{claims_segment}");
        assert!(verify_sig(&spki_der, &signing_input, &assertion.signature));

        // Test signature mismatches on mutations
        assert!(!verify_sig(
            &spki_der,
            &format!("{}.{}", flip_one_bit(header_segment), claims_segment),
            &assertion.signature
        ));
        assert!(!verify_sig(
            &spki_der,
            &format!("{header_segment}.{}", flip_one_bit(claims_segment)),
            &assertion.signature
        ));
        let mut flipped_sig = assertion.signature;
        flipped_sig[0] ^= 1;
        assert!(!verify_sig(&spki_der, &signing_input, &flipped_sig));

        assert_eq!(
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(assertion.signature),
            sig_segment
        );
    }

    #[test]
    fn the_acme_account_claim_is_signed_only_when_asked_for() {
        let temp = TempDir::new();
        let (committed, spki_der) = write_committed_fixture(temp.path());
        let wall = 1_700_000_000_i64;
        let plain = sign_home_reach_assertion("mcp.bridge.register", &committed, wall)
            .expect("plain signs");
        let (_, plain_claims) = claims_of(&plain.compact);
        let mut keys: Vec<&str> = plain_claims
            .as_object()
            .expect("object")
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(keys, ["aud", "exp", "iat", "instance_id", "iss", "scope"]);

        let uri = "https://acme-v02.api.letsencrypt.org/acme/acct/123456";
        let with = sign_home_reach_assertion_with_acme_account(
            "mcp.bridge.register",
            &committed,
            wall,
            uri,
        )
        .expect("claim signs");
        let (signing_input, claims) = claims_of(&with.compact);
        assert_eq!(claims["acme_account_uri"], uri);
        for key in ["aud", "exp", "iat", "instance_id", "iss", "scope"] {
            assert_eq!(claims[key], plain_claims[key], "{key}");
        }
        assert!(verify_sig(&spki_der, &signing_input, &with.signature));
    }

    #[test]
    fn expiration_overflow_fails_cleanly() {
        let temp = TempDir::new();
        let (committed, _) = write_committed_fixture(temp.path());
        let (result, consumed) =
            run_with_home_reach_fault(HomeReachFaultPrimitive::SigningKeyLoad, || {
                sign_home_reach_assertion("mcp.bridge.register", &committed, i64::MAX)
            });
        assert_eq!(
            result.unwrap_err(),
            HomeReachAssertionError::ExpirationOverflow
        );
        assert!(!consumed, "overflow must fail before any checkpoint");
    }

    #[test]
    fn fault_checkpoints_cover_all_primitives() {
        let temp = TempDir::new();
        let (committed, _) = write_committed_fixture(temp.path());
        let wall = 1_700_000_000_i64;

        for (primitive, expected_err) in [
            (
                HomeReachFaultPrimitive::HeaderJsonSerialization,
                HomeReachAssertionError::HeaderJsonSerialization,
            ),
            (
                HomeReachFaultPrimitive::ClaimsJsonSerialization,
                HomeReachAssertionError::ClaimsJsonSerialization,
            ),
            (
                HomeReachFaultPrimitive::SigningKeyLoad,
                HomeReachAssertionError::SigningKeyLoad,
            ),
            (
                HomeReachFaultPrimitive::EcdsaSign,
                HomeReachAssertionError::EcdsaSign,
            ),
        ] {
            let (result, consumed) = run_with_home_reach_fault(primitive, || {
                sign_home_reach_assertion("mcp.bridge.register", &committed, wall)
            });
            assert_eq!(result.unwrap_err(), expected_err);
            assert!(consumed);
        }
    }

    #[test]
    fn fault_guard_cleans_up_after_panic() {
        let panic = catch_unwind(AssertUnwindSafe(|| {
            let _guard = HomeReachFaultGuard::install(HomeReachFaultPrimitive::EcdsaSign);
            panic!("test panic");
        }));
        assert!(panic.is_err());
        assert!(checkpoint(HomeReachFaultPrimitive::EcdsaSign).is_ok());
    }

    #[test]
    fn errors_do_not_render_canaries() {
        let temp = TempDir::new();
        let (committed, _) = write_committed_fixture(temp.path());
        let canary = committed.instance_id();
        for error in [
            HomeReachAssertionError::ExpirationOverflow,
            HomeReachAssertionError::HeaderJsonSerialization,
            HomeReachAssertionError::ClaimsJsonSerialization,
            HomeReachAssertionError::SigningKeyLoad,
            HomeReachAssertionError::EcdsaSign,
            HomeReachAssertionError::AssertionLengthCap,
            HomeReachAssertionError::UnlistedService,
        ] {
            let display = format!("{error}");
            let debug = format!("{error:?}");
            assert!(!display.contains(canary));
            assert!(!debug.contains(canary));
            assert!(!display.is_empty());
        }
    }

    #[test]
    fn service_enable_proof_provenance_and_artifact_match() {
        let proof_raw = include_str!("../../../contracts/service-enable-proof.json");
        let prov_raw = include_str!("../../../contracts/service-enable-proof.provenance.json");
        assert_eq!(proof_raw.len(), 180);
        let sha256_hex = hex(digest::digest(&digest::SHA256, proof_raw.as_bytes()).as_ref());
        let prov: serde_json::Value = serde_json::from_str(prov_raw).expect("provenance JSON");
        assert_eq!(prov["sha256"], sha256_hex);
        assert_eq!(
            sha256_hex,
            "a24f8c962261b539cf1dbf2ae05f016825dfea92aeddf5cf67c8b2f8a1cbb583"
        );

        let proof = service_enable_proof();
        assert_eq!(proof.version, 1);
        assert_eq!(proof.alg, "ES256");
        assert_eq!(proof.typ, "home-reach");
        assert_eq!(proof.aud, "solstone-reach");
        assert_eq!(proof.scope, "services.enable");
        assert_eq!(proof.services, vec!["spl", "spb", "spp", "sme"]);
        assert_eq!(proof.lifetime_seconds, 1800);
        assert_eq!(proof.clock_skew_seconds, 60);
    }

    #[test]
    fn fresh_service_enable_sign_produces_valid_assertion() {
        let temp = TempDir::new();
        let (committed, _) = write_committed_fixture(temp.path());
        let wall = 1_700_000_000_i64;
        let nonce = "2222222222222222222222222222222222222222222222222222";

        let assertion =
            sign_service_enable_assertion(&committed, "spl", nonce, wall).expect("signs");
        assert!(!assertion.ca_pubkey_pem.ends_with("\n"));
        assert!(
            assertion
                .ca_pubkey_pem
                .ends_with("-----END PUBLIC KEY-----")
        );

        let (signing_input, claims) = claims_of(&assertion.compact);
        assert_eq!(claims["iss"], format!("home:{}", committed.instance_id()));
        assert_eq!(claims["aud"], "solstone-reach");
        assert_eq!(claims["scope"], "services.enable");
        assert_eq!(claims["instance_id"], committed.instance_id());
        assert_eq!(claims["nonce"], nonce);
        assert_eq!(claims["service"], "spl");
        assert_eq!(claims["iat"], wall);
        assert_eq!(claims["exp"], wall + 1800);
        assert_eq!(claims.get("acme_account_uri"), None);

        let parts: Vec<&str> = signing_input.split('.').collect();
        let header_bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(parts[0])
            .expect("header b64");
        let header: serde_json::Value = serde_json::from_slice(&header_bytes).expect("header json");
        assert_eq!(header["alg"], "ES256");
        assert_eq!(header["typ"], "home-reach");

        assert!(verify_service_enable_compact(
            &assertion.ca_pubkey_pem,
            &assertion.compact
        ));

        // Unknown service returns UnlistedService
        let err =
            sign_service_enable_assertion(&committed, "unknown_svc", nonce, wall).unwrap_err();
        assert_eq!(err, HomeReachAssertionError::UnlistedService);
    }

    #[test]
    fn service_enable_assertion_tamper_fails_verification() {
        let temp = TempDir::new();
        let (committed, _) = write_committed_fixture(temp.path());
        let wall = 1_700_000_000_i64;
        let nonce = "2222222222222222222222222222222222222222222222222222";

        let assertion =
            sign_service_enable_assertion(&committed, "spl", nonce, wall).expect("signs");

        // Flip signature bit
        let parts: Vec<&str> = assertion.compact.split('.').collect();
        let tampered_sig = flip_one_bit(parts[2]);
        let tampered_compact = format!("{}.{}.{}", parts[0], parts[1], tampered_sig);
        assert!(!verify_service_enable_compact(
            &assertion.ca_pubkey_pem,
            &tampered_compact
        ));

        // Tamper nonce in claims
        for field in ["nonce", "service", "instance_id"] {
            let claims_bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(parts[1])
                .expect("claims b64");
            let mut claims: serde_json::Value =
                serde_json::from_slice(&claims_bytes).expect("claims json");
            claims[field] = serde_json::Value::String("tampered".into());
            let tampered_claims_bytes = serde_json::to_vec(&claims).expect("json");
            let tampered_claims =
                base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&tampered_claims_bytes);
            let tampered_compact = format!("{}.{}.{}", parts[0], tampered_claims, parts[2]);
            assert!(!verify_service_enable_compact(
                &assertion.ca_pubkey_pem,
                &tampered_compact
            ));
        }
    }

    #[test]
    fn service_enable_portal_url_round_trips_and_formats_correctly() {
        let pem = "-----BEGIN PUBLIC KEY-----\nMFkwEwYHKoZIzj0CAQYIKoZIzj0DAQcDQgAE\n-----END PUBLIC KEY-----";
        let compact = "eyJhbGciOiJFUzI1NiJ9.eyJpc3MiOiJob21lIn0.signature";
        let url = service_enable_portal_url(
            "https://services.solstone.app/",
            "spl",
            "NONCE123",
            "inst-456",
            compact,
            pem,
        );

        assert!(url.starts_with("https://services.solstone.app/enable/spl?"));
        let query = url.split_once('?').unwrap().1;
        let pairs: Vec<(&str, &str)> = query
            .split('&')
            .map(|p| p.split_once('=').unwrap())
            .collect();

        assert_eq!(pairs[0].0, "nonce");
        assert_eq!(pairs[0].1, "NONCE123");
        assert_eq!(pairs[1].0, "instance");
        assert_eq!(pairs[1].1, "inst-456");
        assert_eq!(pairs[2].0, "assertion");
        assert_eq!(
            solstone_core_auth_flow::percent_decode(pairs[2].1).unwrap(),
            compact
        );
        assert_eq!(pairs[3].0, "ca_pubkey");
        assert_eq!(
            solstone_core_auth_flow::percent_decode(pairs[3].1).unwrap(),
            pem
        );
    }

    // Generator function for the committed test fixture. Fixed clock: 1_700_000_000.
    // ECDSA with SystemRandom is not byte-stable across runs, so this test is #[ignore]
    // and must not be recaptured during normal CI.
    #[test]
    #[ignore]
    fn capture_service_enable_fixture() {
        let temp = TempDir::new();
        let (committed, _) = write_committed_fixture(temp.path());
        let wall = 1_700_000_000_i64;
        let nonce = "2222222222222222222222222222222222222222222222222222";
        let assertion =
            sign_service_enable_assertion(&committed, "spl", nonce, wall).expect("signs");

        let fixture = serde_json::json!({
            "generator": "capture_service_enable_fixture",
            "clock_unix_seconds": wall,
            "nonce": nonce,
            "service": "spl",
            "instance_id": committed.instance_id(),
            "iat": wall,
            "exp": wall + 1800,
            "ca_pubkey_pem": assertion.ca_pubkey_pem,
            "compact": assertion.compact,
        });

        let target_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
        fs::create_dir_all(&target_dir).expect("fixtures dir");
        fs::write(
            target_dir.join("service_enable_assertion.json"),
            serde_json::to_string_pretty(&fixture).expect("json"),
        )
        .expect("writes fixture");
    }

    #[test]
    fn service_enable_fixture_verifies_at_fixed_clock() {
        let fixture_raw = include_str!("../tests/fixtures/service_enable_assertion.json");
        let fixture: serde_json::Value = serde_json::from_str(fixture_raw).expect("fixture JSON");
        assert_eq!(fixture["generator"], "capture_service_enable_fixture");
        assert_eq!(fixture["clock_unix_seconds"], 1_700_000_000);
        assert_eq!(
            fixture["nonce"],
            "2222222222222222222222222222222222222222222222222222"
        );
        assert_eq!(fixture["service"], "spl");
        assert_eq!(fixture["iat"], 1_700_000_000);
        assert_eq!(fixture["exp"], 1_700_001_800);

        let pem = fixture["ca_pubkey_pem"].as_str().expect("pem");
        let compact = fixture["compact"].as_str().expect("compact");
        assert!(!pem.contains("PRIVATE KEY"));
        assert!(verify_service_enable_compact(pem, compact));
    }
}
