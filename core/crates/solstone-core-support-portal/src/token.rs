// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Access-token and terms-signature construction.

use ring::rand::SystemRandom;
use ring::signature::{RSA_PKCS1_SHA256, RsaKeyPair};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::dpop::jwt_encode;
use crate::errors::PortalClientError;
use crate::jwk::base64url;

pub(crate) fn sha256_b64url(value: &str) -> String {
    base64url(&Sha256::digest(value.as_bytes()))
}

#[derive(Serialize)]
struct AccessHeader {
    typ: &'static str,
    alg: &'static str,
}

#[derive(Serialize)]
struct AccessPayload<'a> {
    jti: &'a str,
    tos_hash: String,
    aud: &'a str,
    cnf: Confirmation<'a>,
    iat: i64,
}

#[derive(Serialize)]
struct Confirmation<'a> {
    jkt: &'a str,
}

pub(crate) fn create_access_token(
    keypair: &RsaKeyPair,
    tos: &str,
    portal_url: &str,
    thumbprint: &str,
    jti: &str,
    iat: i64,
) -> Result<String, PortalClientError> {
    jwt_encode(
        &AccessHeader {
            typ: "wm+jwt",
            alg: "RS256",
        },
        &AccessPayload {
            jti,
            tos_hash: sha256_b64url(tos),
            aud: portal_url,
            cnf: Confirmation { jkt: thumbprint },
            iat,
        },
        keypair,
    )
}

pub(crate) fn sign_tos(keypair: &RsaKeyPair, tos: &str) -> Result<String, PortalClientError> {
    let mut signature = vec![0; keypair.public().modulus_len()];
    keypair
        .sign(
            &RSA_PKCS1_SHA256,
            &SystemRandom::new(),
            tos.as_bytes(),
            &mut signature,
        )
        .map_err(|error| PortalClientError::Signing {
            message: error.to_string(),
        })?;
    Ok(base64url(&signature))
}
