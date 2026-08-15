// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The two deliberately different JSON views of a portal public key.

use base64::Engine;
use rsa::RsaPublicKey;
use rsa::traits::PublicKeyParts;
use serde::Serialize;
use sha2::{Digest, Sha256};

/// The declaration order is the portal JWK wire order: kty, e, n.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct Jwk {
    kty: String,
    e: String,
    n: String,
}

impl Jwk {
    pub(crate) fn public_key(key: &RsaPublicKey) -> Self {
        Self {
            kty: "RSA".to_owned(),
            e: base64url(&key.e().to_bytes_be()),
            n: base64url(&key.n().to_bytes_be()),
        }
    }
}

/// RFC 7638's lexical ordering is explicit: serde_json preserves insertion order here.
#[derive(Serialize)]
struct ThumbprintJwk<'a> {
    e: &'a str,
    kty: &'a str,
    n: &'a str,
}

pub(crate) fn thumbprint(jwk: &Jwk) -> String {
    let canonical = serde_json::to_string(&ThumbprintJwk {
        e: &jwk.e,
        kty: &jwk.kty,
        n: &jwk.n,
    })
    .expect("the borrowed JWK shape is serializable");
    base64url(&Sha256::digest(canonical.as_bytes()))
}

pub(crate) fn base64url(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}
