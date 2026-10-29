// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! ES256 home attestations returned by the pairing ceremony.

use std::fmt;

use base64::Engine as _;
use ring::rand::{SecureRandom, SystemRandom};
use ring::signature::{
    ECDSA_P256_SHA256_FIXED, ECDSA_P256_SHA256_FIXED_SIGNING, EcdsaKeyPair, UnparsedPublicKey,
};
use serde::Serialize;

use crate::ca::LocalCa;

const ATTESTATION_LIFETIME_SECONDS: i64 = 240;

#[derive(Debug)]
pub enum AttestationError {
    Key(rcgen::Error),
    Signing,
    Serialization(serde_json::Error),
    Randomness,
}

impl fmt::Display for AttestationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Key(error) => write!(
                formatter,
                "could not load committed CA key for attestation: {error}"
            ),
            Self::Signing => formatter.write_str("could not sign home attestation"),
            Self::Serialization(error) => {
                write!(formatter, "could not serialize home attestation: {error}")
            }
            Self::Randomness => {
                formatter.write_str("could not generate home attestation identifier")
            }
        }
    }
}

impl std::error::Error for AttestationError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Key(error) => Some(error),
            Self::Serialization(error) => Some(error),
            Self::Signing | Self::Randomness => None,
        }
    }
}

#[derive(Serialize)]
struct Header {
    alg: &'static str,
    typ: &'static str,
}

#[derive(Serialize)]
struct Claims<'a> {
    iss: String,
    aud: &'static str,
    scope: &'static str,
    instance_id: &'a str,
    device_fp: &'a str,
    iat: i64,
    exp: i64,
    jti: String,
}

/// Mint a compact ES256 attestation using the already-loaded committed CA.
pub fn mint_home_attestation(
    ca: &LocalCa,
    instance_id: &str,
    device_fp: &str,
    now: i64,
) -> Result<String, AttestationError> {
    let claims = Claims {
        iss: format!("home:{instance_id}"),
        aud: "spl-relay",
        scope: "device.enroll",
        instance_id,
        device_fp,
        iat: now,
        exp: now + ATTESTATION_LIFETIME_SECONDS,
        jti: random_jti()?,
    };
    sign_compact(ca, "home-attest", &claims)
}

#[derive(Serialize)]
struct EnrollProofClaims<'a> {
    iss: String,
    aud: &'a str,
    scope: &'static str,
    instance_id: &'a str,
    iat: i64,
    exp: i64,
    jti: String,
}

/// Mint the relay enrollment proof: an ES256 JWS proving this home holds its
/// CA private key, which the relay requires before it rotates the home's
/// service token. `audience` is the host of the relay it is sent to, so it
/// cannot be redeemed at another. Every enrollment call mints a fresh one; the
/// relay accepts each proof once.
pub fn mint_home_enroll_proof(
    ca: &LocalCa,
    instance_id: &str,
    audience: &str,
    now: i64,
) -> Result<String, AttestationError> {
    let claims = EnrollProofClaims {
        iss: format!("home:{instance_id}"),
        aud: audience,
        scope: "home.enroll",
        instance_id,
        iat: now,
        exp: now + ATTESTATION_LIFETIME_SECONDS,
        jti: random_jti()?,
    };
    sign_compact(ca, "home-enroll", &claims)
}

fn random_jti() -> Result<String, AttestationError> {
    let mut random = [0_u8; 16];
    SystemRandom::new()
        .fill(&mut random)
        .map_err(|_| AttestationError::Randomness)?;
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(random))
}

fn sign_compact(
    ca: &LocalCa,
    typ: &'static str,
    claims: &impl Serialize,
) -> Result<String, AttestationError> {
    let header = Header { alg: "ES256", typ };
    let header = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(serde_json::to_vec(&header).map_err(AttestationError::Serialization)?);
    let claims = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(serde_json::to_vec(claims).map_err(AttestationError::Serialization)?);
    let signing_input = format!("{header}.{claims}");
    let key = rcgen::KeyPair::from_pem_and_sign_algo(
        &ca.private_key_pem(),
        &rcgen::PKCS_ECDSA_P256_SHA256,
    )
    .map_err(AttestationError::Key)?;
    let key = EcdsaKeyPair::from_pkcs8(
        &ECDSA_P256_SHA256_FIXED_SIGNING,
        &key.serialize_der(),
        &SystemRandom::new(),
    )
    .map_err(|_| AttestationError::Signing)?;
    let signature = key
        .sign(&SystemRandom::new(), signing_input.as_bytes())
        .map_err(|_| AttestationError::Signing)?;
    Ok(format!(
        "{signing_input}.{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(signature.as_ref())
    ))
}

/// Verify a compact home attestation under the committed CA certificate.
///
/// This is intentionally narrow: callers that need claim policy still inspect
/// the decoded claims themselves, while this proves the fixed ES256 signature.
pub fn home_attestation_verifies(certificate_der: &[u8], token: &str) -> bool {
    use x509_parser::prelude::FromDer as _;

    let mut parts = token.split('.');
    let Some(header) = parts.next() else {
        return false;
    };
    let Some(claims) = parts.next() else {
        return false;
    };
    let Some(signature) = parts.next() else {
        return false;
    };
    if parts.next().is_some() {
        return false;
    }
    let Ok(signature) = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(signature) else {
        return false;
    };
    let Ok((_, certificate)) = x509_parser::certificate::X509Certificate::from_der(certificate_der)
    else {
        return false;
    };
    UnparsedPublicKey::new(
        &ECDSA_P256_SHA256_FIXED,
        certificate.public_key().subject_public_key.data.as_ref(),
    )
    .verify(format!("{header}.{claims}").as_bytes(), &signature)
    .is_ok()
}

/// Decode the attestation's public header and claims while retaining the raw
/// signature length for protocol checks. Signature verification remains the
/// caller's separate, explicit step.
pub fn inspect_home_attestation(
    token: &str,
) -> Option<(serde_json::Value, serde_json::Value, usize)> {
    let mut parts = token.split('.');
    let header = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(parts.next()?)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())?;
    let claims = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(parts.next()?)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())?;
    let signature = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(parts.next()?)
        .ok()?;
    parts
        .next()
        .is_none()
        .then_some((header, claims, signature.len()))
}

#[cfg(all(test, not(feature = "full-tests")))]
mod tests {
    use base64::Engine as _;
    use ring::signature::{ECDSA_P256_SHA256_FIXED, UnparsedPublicKey};
    use x509_parser::pem::parse_x509_pem;
    use x509_parser::prelude::FromDer;

    use super::*;

    #[test]
    fn attestation_is_fixed_es256_and_verifies_with_the_committed_ca() {
        let ca = crate::ca::generate_ca().expect("ca");
        let token =
            mint_home_attestation(&ca, "instance", "sha256:device", 100).expect("attestation");
        let mut parts = token.split('.');
        let header = parts.next().expect("header");
        let claims = parts.next().expect("claims");
        let signature = parts.next().expect("signature");
        assert!(parts.next().is_none());
        let decoded_header: serde_json::Value = serde_json::from_slice(
            &base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(header)
                .expect("header bytes"),
        )
        .expect("header json");
        let decoded_claims: serde_json::Value = serde_json::from_slice(
            &base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(claims)
                .expect("claims bytes"),
        )
        .expect("claims json");
        assert_eq!(
            decoded_header,
            serde_json::json!({"alg":"ES256","typ":"home-attest"})
        );
        assert_eq!(decoded_claims["iss"], "home:instance");
        assert_eq!(
            decoded_claims["exp"].as_i64().expect("exp")
                - decoded_claims["iat"].as_i64().expect("iat"),
            240
        );
        let signature = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(signature)
            .expect("signature bytes");
        assert_eq!(signature.len(), 64);
        let (_, pem) = parse_x509_pem(ca.certificate_pem().as_bytes()).expect("pem");
        let (_, certificate) = x509_parser::certificate::X509Certificate::from_der(&pem.contents)
            .expect("certificate");
        UnparsedPublicKey::new(
            &ECDSA_P256_SHA256_FIXED,
            certificate.public_key().subject_public_key.data.as_ref(),
        )
        .verify(format!("{header}.{claims}").as_bytes(), &signature)
        .expect("signature verifies");
    }

    #[test]
    fn enroll_proof_is_a_distinct_short_lived_es256_proof_by_the_committed_ca() {
        let ca = crate::ca::generate_ca().expect("ca");
        let first =
            mint_home_enroll_proof(&ca, "instance", "link.solstone.app", 100).expect("proof");
        let second =
            mint_home_enroll_proof(&ca, "instance", "link.solstone.app", 100).expect("proof");
        let (header, claims, signature_len) = inspect_home_attestation(&first).expect("decodes");
        assert_eq!(
            header,
            serde_json::json!({"alg":"ES256","typ":"home-enroll"})
        );
        assert_eq!(claims["iss"], "home:instance");
        assert_eq!(claims["aud"], "link.solstone.app");
        assert_eq!(claims["scope"], "home.enroll");
        assert_eq!(claims["instance_id"], "instance");
        assert_eq!(claims["iat"], 100);
        assert_eq!(claims["exp"], 340);
        assert!(claims.get("device_fp").is_none());
        assert_eq!(signature_len, 64);
        let (_, second_claims, _) = inspect_home_attestation(&second).expect("decodes");
        assert_ne!(claims["jti"], second_claims["jti"]);
        let (_, pem) = parse_x509_pem(ca.certificate_pem().as_bytes()).expect("pem");
        assert!(home_attestation_verifies(&pem.contents, &first));
        let other = crate::ca::generate_ca().expect("ca");
        let (_, other_pem) = parse_x509_pem(other.certificate_pem().as_bytes()).expect("pem");
        assert!(!home_attestation_verifies(&other_pem.contents, &first));
    }
}
