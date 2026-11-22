// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Optional ATProto / RFC 9449 DPoP (ES256) and RFC 9126 Pushed Authorization Request (PAR)
//! extensions with automatic server `DPoP-Nonce` single-retry handling.

use std::collections::BTreeMap;
use std::fmt;
use std::time::Duration;

use ring::rand::SystemRandom;
use ring::signature::{ECDSA_P256_SHA256_FIXED_SIGNING, EcdsaKeyPair, KeyPair};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::ssrf::{
    DEFAULT_MAX_BODY_BYTES, DEFAULT_REQUEST_TIMEOUT, HttpMethod, HttpRequest, HttpResponse,
    OAuthHttpClient, SsrfFetchError, validate_https_url,
};
use crate::{
    base64_url_no_pad, encode_form, extract_oauth_error_code, percent_encode, random_token,
};

/// Errors from DPoP key handling or proof generation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DpopError {
    KeyGeneration,
    InvalidKey,
    SigningFailed,
    InvalidUrl,
}

impl fmt::Display for DpopError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::KeyGeneration => "failed to generate ES256 DPoP keypair",
            Self::InvalidKey => "invalid PKCS#8 ES256 DPoP key",
            Self::SigningFailed => "failed to sign DPoP proof JWT",
            Self::InvalidUrl => "invalid HTTPS URL for DPoP htu",
        })
    }
}

impl std::error::Error for DpopError {}

/// An ES256 (P-256) keypair used for RFC 9449 DPoP proof-of-possession JWTs.
pub struct DpopKeyPair {
    pkcs8: Vec<u8>,
    key_pair: EcdsaKeyPair,
}

impl fmt::Debug for DpopKeyPair {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DpopKeyPair")
            .field("jkt", &self.thumbprint())
            .finish_non_exhaustive()
    }
}

impl DpopKeyPair {
    /// Generate a fresh P-256 keypair suitable for ES256 DPoP proofs.
    pub fn generate() -> Result<Self, DpopError> {
        let rng = SystemRandom::new();
        let pkcs8 = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &rng)
            .map_err(|_| DpopError::KeyGeneration)?;
        Self::from_pkcs8_bytes(pkcs8.as_ref())
    }

    /// Reconstruct from PKCS#8 DER bytes.
    pub fn from_pkcs8_bytes(bytes: &[u8]) -> Result<Self, DpopError> {
        let rng = SystemRandom::new();
        let key_pair = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, bytes, &rng)
            .map_err(|_| DpopError::InvalidKey)?;
        if key_pair.public_key().as_ref().len() != 65 || key_pair.public_key().as_ref()[0] != 0x04 {
            return Err(DpopError::InvalidKey);
        }
        Ok(Self {
            pkcs8: bytes.to_vec(),
            key_pair,
        })
    }

    /// Reconstruct from unpadded Base64URL-encoded PKCS#8 bytes (stored in `0o600` credential files).
    pub fn from_pkcs8_b64(encoded: &str) -> Result<Self, DpopError> {
        let bytes = base64_url_decode(encoded).ok_or(DpopError::InvalidKey)?;
        Self::from_pkcs8_bytes(&bytes)
    }

    /// Export PKCS#8 DER bytes.
    pub fn to_pkcs8_bytes(&self) -> &[u8] {
        &self.pkcs8
    }

    /// Export unpadded Base64URL-encoded PKCS#8 bytes.
    pub fn to_pkcs8_b64(&self) -> String {
        base64_url_no_pad(&self.pkcs8)
    }

    /// Raw uncompressed SEC1 P-256 public key bytes (`0x04 || x[32] || y[32]`).
    pub fn public_key_uncompressed(&self) -> &[u8] {
        self.key_pair.public_key().as_ref()
    }

    /// Canonical RFC 7638 public JWK JSON value (`crv`, `kty`, `x`, `y` sorted).
    pub fn public_jwk(&self) -> Value {
        let bytes = self.key_pair.public_key().as_ref();
        let x = base64_url_no_pad(&bytes[1..33]);
        let y = base64_url_no_pad(&bytes[33..65]);
        json!({
            "crv": "P-256",
            "kty": "EC",
            "x": x,
            "y": y,
        })
    }

    /// RFC 7638 SHA-256 JWK thumbprint (`jkt`).
    pub fn thumbprint(&self) -> String {
        let bytes = self.key_pair.public_key().as_ref();
        let x = base64_url_no_pad(&bytes[1..33]);
        let y = base64_url_no_pad(&bytes[33..65]);
        let canonical = format!(r#"{{"crv":"P-256","kty":"EC","x":"{x}","y":"{y}"}}"#);
        base64_url_no_pad(&Sha256::digest(canonical.as_bytes()))
    }

    /// Sign one RFC 9449 `dpop+jwt` proof for `htm` (`GET`/`POST`) and HTTPS `url`.
    ///
    /// Query strings and fragments are stripped from `htu` per RFC 9449 § 4.2.
    /// When `access_token` is supplied, includes `ath` = `base64url(sha256(access_token))`.
    pub fn sign_proof(
        &self,
        htm: &str,
        url: &str,
        iat: u64,
        jti: &str,
        nonce: Option<&str>,
        access_token: Option<&str>,
    ) -> Result<String, DpopError> {
        let parsed = validate_https_url(url).map_err(|_| DpopError::InvalidUrl)?;
        let htu = parsed.without_query();

        let header = json!({
            "alg": "ES256",
            "jwk": self.public_jwk(),
            "typ": "dpop+jwt",
        });
        let mut claims = serde_json::Map::new();
        claims.insert("htm".to_owned(), Value::String(htm.to_ascii_uppercase()));
        claims.insert("htu".to_owned(), Value::String(htu));
        claims.insert("iat".to_owned(), Value::from(iat));
        claims.insert("jti".to_owned(), Value::String(jti.to_owned()));
        if let Some(nonce) = nonce.filter(|n| !n.is_empty()) {
            claims.insert("nonce".to_owned(), Value::String(nonce.to_owned()));
        }
        if let Some(token) = access_token.filter(|t| !t.is_empty()) {
            let ath = base64_url_no_pad(&Sha256::digest(token.as_bytes()));
            claims.insert("ath".to_owned(), Value::String(ath));
        }

        let header_json = serde_json::to_vec(&header).map_err(|_| DpopError::SigningFailed)?;
        let claims_json =
            serde_json::to_vec(&Value::Object(claims)).map_err(|_| DpopError::SigningFailed)?;
        let signing_input = format!(
            "{}.{}",
            base64_url_no_pad(&header_json),
            base64_url_no_pad(&claims_json)
        );

        let rng = SystemRandom::new();
        let signature = self
            .key_pair
            .sign(&rng, signing_input.as_bytes())
            .map_err(|_| DpopError::SigningFailed)?;
        Ok(format!(
            "{signing_input}.{}",
            base64_url_no_pad(signature.as_ref())
        ))
    }
}

/// Errors from DPoP-protected requests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DpopRequestError {
    Dpop(DpopError),
    Fetch(SsrfFetchError),
    Random,
}

impl fmt::Display for DpopRequestError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Dpop(err) => err.fmt(formatter),
            Self::Fetch(err) => err.fmt(formatter),
            Self::Random => formatter.write_str("failed to generate random DPoP jti"),
        }
    }
}

impl std::error::Error for DpopRequestError {}

/// Send one HTTP request with a DPoP proof, automatically retrying **once** when the server
/// challenges with `use_dpop_nonce` and returns a fresh `DPoP-Nonce` response header.
#[allow(clippy::too_many_arguments)]
pub fn send_with_dpop_nonce_retry(
    http: &dyn OAuthHttpClient,
    dpop_key: &DpopKeyPair,
    initial_nonce: Option<&str>,
    method: HttpMethod,
    url: &str,
    extra_headers: &[(String, String)],
    body: Option<Vec<u8>>,
    access_token: Option<&str>,
    now_unix: u64,
    timeout: Duration,
) -> Result<(HttpResponse, Option<String>), DpopRequestError> {
    let mut current_nonce = initial_nonce.map(ToOwned::to_owned);
    for attempt in 0..2 {
        let jti = random_token(16).map_err(|_| DpopRequestError::Random)?;
        let proof = dpop_key
            .sign_proof(
                method.as_str(),
                url,
                now_unix,
                &jti,
                current_nonce.as_deref(),
                access_token,
            )
            .map_err(DpopRequestError::Dpop)?;

        let mut headers = extra_headers.to_vec();
        headers.push(("DPoP".to_owned(), proof));
        let req = HttpRequest {
            method,
            url: url.to_owned(),
            headers,
            body: body.clone(),
            timeout,
            max_body_bytes: DEFAULT_MAX_BODY_BYTES,
        };
        let response = http.send(&req).map_err(DpopRequestError::Fetch)?;
        let server_nonce = response
            .header("dpop-nonce")
            .filter(|n| !n.is_empty())
            .map(ToOwned::to_owned);
        if let Some(ref fresh) = server_nonce {
            current_nonce = Some(fresh.clone());
        }

        if attempt == 0 && is_use_dpop_nonce_challenge(&response) && server_nonce.is_some() {
            continue;
        }
        return Ok((response, current_nonce));
    }
    unreachable!("loop always returns on attempt 0 or 1")
}

fn is_use_dpop_nonce_challenge(response: &HttpResponse) -> bool {
    if response.status != 400 && response.status != 401 {
        return false;
    }
    if extract_oauth_error_code(&response.body).as_deref() == Some("use_dpop_nonce") {
        return true;
    }
    response
        .headers_named("www-authenticate")
        .iter()
        .any(|val| val.contains("use_dpop_nonce"))
}

/// Successful RFC 9126 Pushed Authorization Request response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParResponse {
    pub request_uri: String,
    pub expires_in: u64,
    pub dpop_nonce: Option<String>,
}

/// Errors from Pushed Authorization Requests (RFC 9126).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParError {
    InvalidEndpoint,
    Dpop(DpopError),
    Fetch(SsrfFetchError),
    Random,
    Rejected { status: u16, error: Option<String> },
    MalformedResponse,
}

impl fmt::Display for ParError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidEndpoint => formatter.write_str("invalid PAR endpoint URL"),
            Self::Dpop(err) => err.fmt(formatter),
            Self::Fetch(err) => err.fmt(formatter),
            Self::Random => formatter.write_str("failed to generate random DPoP jti"),
            Self::Rejected { status, error } => match error {
                Some(code) => write!(
                    formatter,
                    "PAR endpoint rejected request ({status}: {code})"
                ),
                None => write!(formatter, "PAR endpoint rejected request ({status})"),
            },
            Self::MalformedResponse => formatter.write_str("malformed PAR JSON response"),
        }
    }
}

impl std::error::Error for ParError {}

/// Submit a Pushed Authorization Request (RFC 9126), optionally DPoP-signed with automatic
/// `DPoP-Nonce` retry (required by ATProto PDS authorization servers).
pub fn push_authorization_request(
    http: &dyn OAuthHttpClient,
    par_endpoint: &str,
    params: &BTreeMap<String, String>,
    dpop: Option<(&DpopKeyPair, Option<&str>, u64)>,
) -> Result<ParResponse, ParError> {
    validate_https_url(par_endpoint).map_err(|_| ParError::InvalidEndpoint)?;
    let form_body = encode_form(params).into_bytes();
    let content_headers = vec![(
        "Content-Type".to_owned(),
        "application/x-www-form-urlencoded".to_owned(),
    )];

    let (response, dpop_nonce) = match dpop {
        Some((key, nonce, now_unix)) => send_with_dpop_nonce_retry(
            http,
            key,
            nonce,
            HttpMethod::Post,
            par_endpoint,
            &content_headers,
            Some(form_body),
            None,
            now_unix,
            DEFAULT_REQUEST_TIMEOUT,
        )
        .map_err(|err| match err {
            DpopRequestError::Dpop(e) => ParError::Dpop(e),
            DpopRequestError::Fetch(e) => ParError::Fetch(e),
            DpopRequestError::Random => ParError::Random,
        })?,
        None => {
            let req =
                HttpRequest::post(par_endpoint, "application/x-www-form-urlencoded", form_body);
            let resp = http.send(&req).map_err(ParError::Fetch)?;
            (resp, None)
        }
    };

    if response.status != 200 && response.status != 201 {
        return Err(ParError::Rejected {
            status: response.status,
            error: extract_oauth_error_code(&response.body),
        });
    }

    let json: Value =
        serde_json::from_slice(&response.body).map_err(|_| ParError::MalformedResponse)?;
    let request_uri = json
        .get("request_uri")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or(ParError::MalformedResponse)?
        .to_owned();
    let expires_in = json
        .get("expires_in")
        .and_then(Value::as_u64)
        .filter(|v| *v > 0)
        .ok_or(ParError::MalformedResponse)?;

    Ok(ParResponse {
        request_uri,
        expires_in,
        dpop_nonce,
    })
}

/// Build the authorization redirect URL for a PAR `request_uri`.
pub fn build_par_authorize_url(
    authorization_endpoint: &str,
    client_id: &str,
    request_uri: &str,
) -> Result<String, ParError> {
    validate_https_url(authorization_endpoint).map_err(|_| ParError::InvalidEndpoint)?;
    let separator = if authorization_endpoint.contains('?') {
        '&'
    } else {
        '?'
    };
    Ok(format!(
        "{authorization_endpoint}{separator}client_id={}&request_uri={}",
        percent_encode(client_id),
        percent_encode(request_uri)
    ))
}

pub(crate) fn base64_url_decode(input: &str) -> Option<Vec<u8>> {
    let mut output = Vec::with_capacity(input.len() * 3 / 4);
    let mut buffer: u32 = 0;
    let mut bits: u8 = 0;
    for byte in input.bytes() {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'-' => 62,
            b'_' => 63,
            b'=' => break,
            _ => return None,
        };
        buffer = (buffer << 6) | u32::from(value);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            output.push((buffer >> bits) as u8);
        }
    }
    Some(output)
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::Mutex;

    use ring::signature::{ECDSA_P256_SHA256_FIXED, UnparsedPublicKey};

    use super::*;

    struct ScriptedHttp {
        responses: Mutex<VecDeque<HttpResponse>>,
        requests: Mutex<Vec<HttpRequest>>,
    }

    impl ScriptedHttp {
        fn new(responses: Vec<HttpResponse>) -> Self {
            Self {
                responses: Mutex::new(VecDeque::from(responses)),
                requests: Mutex::new(Vec::new()),
            }
        }
    }

    impl OAuthHttpClient for ScriptedHttp {
        fn send(&self, request: &HttpRequest) -> Result<HttpResponse, SsrfFetchError> {
            self.requests.lock().unwrap().push(request.clone());
            self.responses
                .lock()
                .unwrap()
                .pop_front()
                .ok_or(SsrfFetchError::Connect)
        }
    }

    #[test]
    fn dpop_keypair_signs_verifiable_es256_jwt_with_stripped_htu_nonce_and_ath() {
        let key = DpopKeyPair::generate().expect("generate P-256 key");
        let b64 = key.to_pkcs8_b64();
        let reloaded = DpopKeyPair::from_pkcs8_b64(&b64).expect("reload pkcs8");
        assert_eq!(key.thumbprint(), reloaded.thumbprint());

        let proof = reloaded
            .sign_proof(
                "post",
                "https://pds.example.com/oauth/par?ignored=1",
                1_760_000_000,
                "jti-42",
                Some("server-nonce-1"),
                Some("access-token-xyz"),
            )
            .expect("sign dpop proof");

        let parts: Vec<&str> = proof.split('.').collect();
        assert_eq!(parts.len(), 3);
        let signing_input = format!("{}.{}", parts[0], parts[1]);
        let sig_bytes = base64_url_decode(parts[2]).expect("sig b64");
        assert_eq!(sig_bytes.len(), 64, "ES256 JWS signature must be 64 bytes");

        UnparsedPublicKey::new(&ECDSA_P256_SHA256_FIXED, reloaded.public_key_uncompressed())
            .verify(signing_input.as_bytes(), &sig_bytes)
            .expect("ES256 signature verifies");

        let header: Value = serde_json::from_slice(&base64_url_decode(parts[0]).unwrap()).unwrap();
        assert_eq!(header["typ"], "dpop+jwt");
        assert_eq!(header["alg"], "ES256");
        assert_eq!(header["jwk"]["kty"], "EC");
        assert_eq!(header["jwk"]["crv"], "P-256");

        let claims: Value = serde_json::from_slice(&base64_url_decode(parts[1]).unwrap()).unwrap();
        assert_eq!(claims["htm"], "POST");
        assert_eq!(claims["htu"], "https://pds.example.com/oauth/par");
        assert_eq!(claims["iat"], 1_760_000_000);
        assert_eq!(claims["jti"], "jti-42");
        assert_eq!(claims["nonce"], "server-nonce-1");
        assert_eq!(
            claims["ath"],
            base64_url_no_pad(&Sha256::digest(b"access-token-xyz"))
        );
    }

    #[test]
    fn par_retries_once_on_use_dpop_nonce_and_builds_authorize_url() {
        let key = DpopKeyPair::generate().unwrap();
        let http = ScriptedHttp::new(vec![
            HttpResponse {
                status: 400,
                headers: vec![("dpop-nonce".to_owned(), "fresh-nonce-99".to_owned())],
                body: br#"{"error":"use_dpop_nonce"}"#.to_vec(),
            },
            HttpResponse {
                status: 201,
                headers: vec![("dpop-nonce".to_owned(), "next-nonce-100".to_owned())],
                body: br#"{"request_uri":"urn:ietf:params:oauth:request_uri:abc","expires_in":60}"#
                    .to_vec(),
            },
        ]);

        let mut params = BTreeMap::new();
        params.insert(
            "client_id".to_owned(),
            "https://solstone.app/oauth-client-metadata.json".to_owned(),
        );
        params.insert("response_type".to_owned(), "code".to_owned());
        params.insert(
            "redirect_uri".to_owned(),
            "http://127.0.0.1:54321/callback".to_owned(),
        );

        let par = push_authorization_request(
            &http,
            "https://bsky.social/oauth/par",
            &params,
            Some((&key, None, 1_760_000_000)),
        )
        .expect("PAR succeeds after nonce retry");

        assert_eq!(par.request_uri, "urn:ietf:params:oauth:request_uri:abc");
        assert_eq!(par.expires_in, 60);
        assert_eq!(par.dpop_nonce.as_deref(), Some("next-nonce-100"));

        let reqs = http.requests.lock().unwrap();
        assert_eq!(reqs.len(), 2);
        // Second attempt must include fresh-nonce-99 in its DPoP proof claims.
        let second_dpop = reqs[1]
            .headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("dpop"))
            .map(|(_, v)| v.as_str())
            .expect("dpop header");
        let payload_b64 = second_dpop.split('.').nth(1).unwrap();
        let claims: Value =
            serde_json::from_slice(&base64_url_decode(payload_b64).unwrap()).unwrap();
        assert_eq!(claims["nonce"], "fresh-nonce-99");

        let auth_url = build_par_authorize_url(
            "https://bsky.social/oauth/authorize",
            "https://solstone.app/oauth-client-metadata.json",
            &par.request_uri,
        )
        .unwrap();
        assert!(auth_url.starts_with("https://bsky.social/oauth/authorize?client_id="));
        assert!(auth_url.contains("request_uri=urn%3Aietf%3Aparams%3Aoauth%3Arequest_uri%3Aabc"));
    }
}
