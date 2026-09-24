// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Host helper for signing compact home-reach assertions.

use std::fmt;

use base64::Engine as _;
use ring::rand::SystemRandom;
use ring::signature::{ECDSA_P256_SHA256_FIXED_SIGNING, EcdsaKeyPair};
use serde::Serialize;

use crate::committed::CommittedIdentity;

const ASSERTION_CAP_BYTES: usize = 8_192;
const ASSERTION_LIFETIME_SECONDS: i64 = 240;

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
}

/// Sign a compact home-reach assertion with the committed CA private key.
pub fn sign_home_reach_assertion(
    scope: &str,
    committed: &CommittedIdentity,
    wall_unix_seconds: i64,
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
    })
    .map_err(|_| HomeReachAssertionError::ClaimsJsonSerialization)?;
    let claims = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&claims_bytes);

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
    use ring::signature::{ECDSA_P256_SHA256_FIXED, UnparsedPublicKey};

    use super::*;
    use crate::ca::{generate_ca, jid_from_spki};
    use crate::committed::load_committed_identity;

    const P256_SPKI_PREFIX: &[u8] = &[
        0x30, 0x59, 0x30, 0x13, 0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01, 0x06, 0x08,
        0x2a, 0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07, 0x03, 0x42, 0x00,
    ];

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
        ] {
            let display = format!("{error}");
            let debug = format!("{error:?}");
            assert!(!display.contains(canary));
            assert!(!debug.contains(canary));
            assert!(!display.is_empty());
        }
    }
}
