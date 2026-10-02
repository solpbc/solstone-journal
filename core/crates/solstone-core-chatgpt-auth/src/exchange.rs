// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Token exchange implementation for ChatGPT OAuth flow.

use std::collections::BTreeMap;
use std::time::Duration;

use crate::credential::ClosedOutcome;
use crate::store::{DIRECT_USE_SCOPE, Tokens};
use crate::transport::{ChatGptTransport, HttpResponse, TransportError};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedExchange {
    pub tokens: Option<Tokens>,
    pub refresh_token: String,
    pub subject: Option<String>,
    pub email: Option<String>,
    pub plan_usage_granted: bool,
}

fn base64_url_decode(input: &str) -> Result<Vec<u8>, ()> {
    let unpadded = input.trim_end_matches('=');
    let mut table = [0xFFu8; 256];
    for (i, &b) in b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_"
        .iter()
        .enumerate()
    {
        table[b as usize] = i as u8;
    }
    let mut bytes = Vec::with_capacity((unpadded.len() * 3) / 4);
    let mut buf = 0u32;
    let mut bits = 0;
    for &b in unpadded.as_bytes() {
        let val = table[b as usize];
        if val == 0xFF {
            return Err(());
        }
        buf = (buf << 6) | (val as u32);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            bytes.push((buf >> bits) as u8);
        }
    }
    Ok(bytes)
}

#[allow(clippy::result_unit_err)]
pub fn parse_and_validate_id_token(
    id_token: &str,
    expected_client_id: &str,
    expected_nonce: &str,
    now: u64,
) -> Result<(Option<String>, Option<String>), ()> {
    // OpenID Connect Core § 3.1.3.7 item 6: The ID token is received directly from the
    // token endpoint over TLS, so signature validation is not required.
    let mut parts = id_token.split('.');
    let _header = parts.next().ok_or(())?;
    let payload_b64 = parts.next().ok_or(())?;
    let bytes = base64_url_decode(payload_b64).map_err(|_| ())?;
    let json: serde_json::Value = serde_json::from_slice(&bytes).map_err(|_| ())?;

    // Validate iss
    let iss = json.get("iss").and_then(|v| v.as_str()).ok_or(())?;
    if iss != "https://auth.openai.com" {
        return Err(());
    }

    // Validate aud contains expected_client_id
    match json.get("aud") {
        Some(serde_json::Value::String(s)) => {
            if s != expected_client_id {
                return Err(());
            }
        }
        Some(serde_json::Value::Array(arr)) => {
            let matched = arr.iter().any(|v| v.as_str() == Some(expected_client_id));
            if !matched {
                return Err(());
            }
        }
        _ => return Err(()),
    }

    // Validate exp
    let exp = json.get("exp").and_then(|v| v.as_u64()).ok_or(())?;
    if exp <= now {
        return Err(());
    }

    // Validate nonce
    let nonce = json.get("nonce").and_then(|v| v.as_str()).ok_or(())?;
    if nonce != expected_nonce {
        return Err(());
    }

    let sub = json
        .get("sub")
        .and_then(|v| v.as_str())
        .map(ToString::to_string);
    let email = json
        .get("email")
        .and_then(|v| v.as_str())
        .map(ToString::to_string);

    Ok((sub, email))
}

#[allow(clippy::too_many_arguments)]
pub fn exchange_code_for_tokens(
    transport: &dyn ChatGptTransport,
    auth_base: &str,
    code: &str,
    redirect_uri: &str,
    client_id: &str,
    verifier: &str,
    expected_nonce: &str,
    saved_subject: Option<&str>,
    timeout: Duration,
) -> Result<ValidatedExchange, ClosedOutcome> {
    let token_url = format!("{auth_base}/oauth/token");
    let mut form = BTreeMap::new();
    form.insert("grant_type".to_string(), "authorization_code".to_string());
    form.insert("code".to_string(), code.to_string());
    form.insert("redirect_uri".to_string(), redirect_uri.to_string());
    form.insert("client_id".to_string(), client_id.to_string());
    form.insert("code_verifier".to_string(), verifier.to_string());

    let response = match transport.post_form(&token_url, &form, timeout) {
        Ok(resp) => resp,
        Err(TransportError::Network | TransportError::Timeout) => {
            return Err(ClosedOutcome::ExchangeFailed);
        }
    };

    let classified = classify_exchange_response(response, client_id, expected_nonce, saved_subject);
    match classified {
        Ok(val) => Ok(val),
        Err((outcome, token_opt)) => {
            if outcome == ClosedOutcome::AccountMismatch
                && let Some(token) = token_opt
            {
                crate::revoke::revoke_refresh_token(transport, auth_base, &token, client_id);
            }
            Err(outcome)
        }
    }
}

pub fn classify_exchange_response(
    response: HttpResponse,
    client_id: &str,
    expected_nonce: &str,
    saved_subject: Option<&str>,
) -> Result<ValidatedExchange, (ClosedOutcome, Option<String>)> {
    if response.status == 200 {
        let Ok(json) = serde_json::from_str::<serde_json::Value>(&response.body) else {
            return Err((ClosedOutcome::ExchangeFailed, None));
        };

        // Validate token_type (must be present and Bearer, case-insensitive)
        let Some(token_type) = json.get("token_type").and_then(|v| v.as_str()) else {
            return Err((ClosedOutcome::ExchangeFailed, None));
        };
        if !token_type.eq_ignore_ascii_case("Bearer") {
            return Err((ClosedOutcome::ExchangeFailed, None));
        }

        // Validate expires_in (must be positive integer)
        let Some(expires_in) = json.get("expires_in").and_then(|v| v.as_u64()) else {
            return Err((ClosedOutcome::ExchangeFailed, None));
        };
        if expires_in == 0 {
            return Err((ClosedOutcome::ExchangeFailed, None));
        }

        let Some(access_token) = json.get("access_token").and_then(|v| v.as_str()) else {
            return Err((ClosedOutcome::ExchangeFailed, None));
        };
        let Some(refresh_token) = json.get("refresh_token").and_then(|v| v.as_str()) else {
            return Err((ClosedOutcome::ExchangeFailed, None));
        };

        if access_token.is_empty() || refresh_token.is_empty() {
            return Err((ClosedOutcome::ExchangeFailed, None));
        }

        let now = crate::store::now_secs();

        // ID token validation (must be present)
        let Some(id_tok) = json.get("id_token").and_then(|v| v.as_str()) else {
            return Err((ClosedOutcome::ExchangeFailed, None));
        };
        let (sub, email) = match parse_and_validate_id_token(id_tok, client_id, expected_nonce, now)
        {
            Ok(claims) => claims,
            Err(()) => return Err((ClosedOutcome::ExchangeFailed, None)),
        };

        // Check account mismatch
        if let Some(saved_sub) = saved_subject
            && let Some(ref current_sub) = sub
            && current_sub != saved_sub
        {
            return Err((
                ClosedOutcome::AccountMismatch,
                Some(refresh_token.to_string()),
            ));
        }

        let scopes: Vec<String> = json
            .get("scope")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .split_whitespace()
            .map(ToString::to_string)
            .collect();

        let plan_usage_granted = scopes.iter().any(|s| s == DIRECT_USE_SCOPE);
        let expires_at = now.saturating_add(expires_in);

        let tokens = if plan_usage_granted {
            Some(Tokens {
                access_token: access_token.to_string(),
                refresh_token: refresh_token.to_string(),
                expires_at,
                scopes,
            })
        } else {
            None
        };

        Ok(ValidatedExchange {
            tokens,
            refresh_token: refresh_token.to_string(),
            subject: sub,
            email,
            plan_usage_granted,
        })
    } else {
        if let Ok(err_json) = serde_json::from_str::<serde_json::Value>(&response.body) {
            let error_code = err_json.get("error").and_then(|v| v.as_str());
            if error_code == Some("invalid_client") {
                return Err((ClosedOutcome::RegistrationRefused, None));
            }
        }
        Err((ClosedOutcome::ExchangeFailed, None))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_id_token_checks_iss_aud_exp_nonce() {
        let now = 1000;
        // payload: {"iss":"https://auth.openai.com","aud":"test-client","exp":2000,"nonce":"test-nonce","sub":"user-1","email":"u@e.com"}
        let payload = r#"{"iss":"https://auth.openai.com","aud":"test-client","exp":2000,"nonce":"test-nonce","sub":"user-1","email":"u@e.com"}"#;
        let mut b64 = String::new();
        for chunk in payload.as_bytes().chunks(3) {
            let mut buf = 0u32;
            for &b in chunk {
                buf = (buf << 8) | (b as u32);
            }
            let pad = 3 - chunk.len();
            buf <<= pad * 8;
            let alphabet = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
            b64.push(alphabet[((buf >> 18) & 0x3F) as usize] as char);
            b64.push(alphabet[((buf >> 12) & 0x3F) as usize] as char);
            if pad < 2 {
                b64.push(alphabet[((buf >> 6) & 0x3F) as usize] as char);
            }
            if pad < 1 {
                b64.push(alphabet[(buf & 0x3F) as usize] as char);
            }
        }
        let jwt = format!("eyJhbGciOiJub25lIn0.{b64}.");

        let claims = parse_and_validate_id_token(&jwt, "test-client", "test-nonce", now)
            .expect("valid token");
        assert_eq!(claims.0.as_deref(), Some("user-1"));
        assert_eq!(claims.1.as_deref(), Some("u@e.com"));

        // Wrong iss
        assert!(parse_and_validate_id_token(&jwt, "other-client", "test-nonce", now).is_err());
        assert!(parse_and_validate_id_token(&jwt, "test-client", "wrong-nonce", now).is_err());
        assert!(parse_and_validate_id_token(&jwt, "test-client", "test-nonce", 3000).is_err());
    }

    fn make_test_jwt(client_id: &str, nonce: &str, sub: &str, email: &str) -> String {
        let payload = serde_json::json!({
            "iss": "https://auth.openai.com",
            "aud": client_id,
            "exp": 2000000000u64,
            "nonce": nonce,
            "sub": sub,
            "email": email,
        });
        let bytes = serde_json::to_vec(&payload).unwrap();
        let mut b64 = String::new();
        for chunk in bytes.chunks(3) {
            let mut buf = 0u32;
            for &b in chunk {
                buf = (buf << 8) | (b as u32);
            }
            let pad = 3 - chunk.len();
            buf <<= pad * 8;
            let alphabet = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
            b64.push(alphabet[((buf >> 18) & 0x3F) as usize] as char);
            b64.push(alphabet[((buf >> 12) & 0x3F) as usize] as char);
            if pad < 2 {
                b64.push(alphabet[((buf >> 6) & 0x3F) as usize] as char);
            }
            if pad < 1 {
                b64.push(alphabet[(buf & 0x3F) as usize] as char);
            }
        }
        format!("eyJhbGciOiJub25lIn0.{b64}.")
    }

    #[test]
    fn classify_exchange_response_validations() {
        let jwt = make_test_jwt(
            "test-client",
            "test-nonce",
            "user-sub-1",
            "user@example.com",
        );

        // 1. Missing refresh token -> ExchangeFailed
        let resp1 = HttpResponse {
            status: 200,
            body: format!(
                r#"{{"token_type":"Bearer","access_token":"a","expires_in":3600,"scope":"openid {DIRECT_USE_SCOPE}","id_token":"{jwt}"}}"#
            ),
            headers: BTreeMap::new(),
        };
        assert_eq!(
            classify_exchange_response(resp1, "test-client", "test-nonce", None),
            Err((ClosedOutcome::ExchangeFailed, None))
        );

        // 2. Non-Bearer token_type -> ExchangeFailed
        let resp2 = HttpResponse {
            status: 200,
            body: format!(
                r#"{{"token_type":"Basic","access_token":"a","refresh_token":"r","expires_in":3600,"scope":"openid {DIRECT_USE_SCOPE}","id_token":"{jwt}"}}"#
            ),
            headers: BTreeMap::new(),
        };
        assert_eq!(
            classify_exchange_response(resp2, "test-client", "test-nonce", None),
            Err((ClosedOutcome::ExchangeFailed, None))
        );

        // 3. expires_in <= 0 -> ExchangeFailed
        let resp3 = HttpResponse {
            status: 200,
            body: format!(
                r#"{{"token_type":"Bearer","access_token":"a","refresh_token":"r","expires_in":0,"scope":"openid {DIRECT_USE_SCOPE}","id_token":"{jwt}"}}"#
            ),
            headers: BTreeMap::new(),
        };
        assert_eq!(
            classify_exchange_response(resp3, "test-client", "test-nonce", None),
            Err((ClosedOutcome::ExchangeFailed, None))
        );

        // 4. Account mismatch revoking unused refresh
        let resp4 = HttpResponse {
            status: 200,
            body: format!(
                r#"{{"token_type":"Bearer","access_token":"a","refresh_token":"r-mismatch","expires_in":3600,"scope":"openid {DIRECT_USE_SCOPE}","id_token":"{jwt}"}}"#
            ),
            headers: BTreeMap::new(),
        };
        assert_eq!(
            classify_exchange_response(resp4, "test-client", "test-nonce", Some("different-sub")),
            Err((
                ClosedOutcome::AccountMismatch,
                Some("r-mismatch".to_string())
            ))
        );

        // 5. Scope without direct use -> plan_usage_granted = false, tokens = None
        let resp5 = HttpResponse {
            status: 200,
            body: format!(
                r#"{{"token_type":"Bearer","access_token":"a","refresh_token":"r","expires_in":3600,"scope":"openid profile","id_token":"{jwt}"}}"#
            ),
            headers: BTreeMap::new(),
        };
        let val5 = classify_exchange_response(resp5, "test-client", "test-nonce", None).unwrap();
        assert!(!val5.plan_usage_granted);
        assert!(val5.tokens.is_none());
        assert_eq!(val5.refresh_token, "r");

        // 6. 400 invalid_client -> RegistrationRefused
        let resp6 = HttpResponse {
            status: 400,
            body: r#"{"error":"invalid_client"}"#.to_string(),
            headers: BTreeMap::new(),
        };
        assert_eq!(
            classify_exchange_response(resp6, "test-client", "test-nonce", None),
            Err((ClosedOutcome::RegistrationRefused, None))
        );
    }
}
