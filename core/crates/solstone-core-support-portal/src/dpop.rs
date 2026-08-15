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
    let header = json_ascii(header)?;
    let payload = json_ascii(payload)?;
    let input = format!(
        "{}.{}",
        base64url(header.as_bytes()),
        base64url(payload.as_bytes())
    );
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

/// Match Python json.dumps's default ensure_ascii=True wire encoding.
pub(crate) fn json_ascii<T: Serialize>(value: &T) -> Result<String, PortalClientError> {
    let json = serde_json::to_string(value).map_err(|error| PortalClientError::State {
        message: error.to_string(),
    })?;
    let mut output = String::with_capacity(json.len());
    for character in json.chars() {
        if character.is_ascii() {
            output.push(character);
            continue;
        }
        let value = character as u32;
        if value <= 0xffff {
            output.push_str(&format!("\\u{value:04x}"));
        } else {
            let value = value - 0x1_0000;
            output.push_str(&format!(
                "\\u{:04x}\\u{:04x}",
                0xd800 + (value >> 10),
                0xdc00 + (value & 0x3ff)
            ));
        }
    }
    Ok(output)
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
