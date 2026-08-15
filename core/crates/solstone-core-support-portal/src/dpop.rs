// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! DPoP proof construction and compact JWT signing.

use ring::rand::SystemRandom;
use ring::signature::{RSA_PKCS1_SHA256, RsaKeyPair};
use serde::Serialize;

use crate::errors::PortalClientError;
use crate::jwk::{Jwk, base64url};
use crate::token::sha256_b64url;

pub(crate) fn jwt_encode<H: Serialize, P: Serialize>(
    header: &H,
    payload: &P,
    keypair: &RsaKeyPair,
) -> Result<String, PortalClientError> {
    let header = serde_json::to_vec(header).map_err(|error| PortalClientError::State {
        message: error.to_string(),
    })?;
    let payload = serde_json::to_vec(payload).map_err(|error| PortalClientError::State {
        message: error.to_string(),
    })?;
    let input = format!("{}.{}", base64url(&header), base64url(&payload));
    let mut signature = vec![0; keypair.public().modulus_len()];
    keypair
        .sign(
            &RSA_PKCS1_SHA256,
            &SystemRandom::new(),
            input.as_bytes(),
            &mut signature,
        )
        .map_err(|error| PortalClientError::Signing {
            message: error.to_string(),
        })?;
    Ok(format!("{input}.{}", base64url(&signature)))
}

#[derive(Serialize)]
struct DpopHeader<'a> {
    typ: &'static str,
    alg: &'static str,
    jwk: &'a Jwk,
}

#[derive(Serialize)]
struct DpopPayload<'a> {
    jti: &'a str,
    htm: &'a str,
    htu: &'a str,
    iat: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    ath: Option<String>,
}

pub(crate) fn create_dpop_proof(
    keypair: &RsaKeyPair,
    jwk: &Jwk,
    method: &str,
    url: &str,
    jti: &str,
    iat: i64,
    access_token: Option<&str>,
) -> Result<String, PortalClientError> {
    // Reference compatibility: despite its "query/fragment" comment, Python splits
    // only at `?`; a bare fragment is signed and is server-verified that way.
    let htu = url.split('?').next().unwrap_or(url);
    jwt_encode(
        &DpopHeader {
            typ: "dpop+jwt",
            alg: "RS256",
            jwk,
        },
        &DpopPayload {
            jti,
            htm: method,
            htu,
            iat,
            ath: access_token.map(sha256_b64url),
        },
        keypair,
    )
}
