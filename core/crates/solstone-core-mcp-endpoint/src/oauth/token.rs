// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Token endpoint for journal-hosted MCP OAuth.

use serde::Serialize;

use super::store::{IssuedTokens, OAuthStoreError};
use super::{
    MAX_CLIENT_ID_BYTES, MAX_CODE_BYTES, MAX_REDIRECT_URI_BYTES, OAuthRuntime,
    TOKEN_MAX_BODY_BYTES, parse_urlencoded_pairs, reject_non_identity_encoding,
};
use crate::http1::{HttpRequest, HttpResponse};

/// POST `/token`.
pub(crate) fn token(request: &HttpRequest, oauth: &OAuthRuntime) -> HttpResponse {
    if let Err(response) = reject_non_identity_encoding(request) {
        return cache_headers(response);
    }
    if request.body.len() > TOKEN_MAX_BODY_BYTES {
        return cache_headers(HttpResponse::error(
            413,
            "Payload Too Large",
            "token request exceeds its size limit",
        ));
    }
    let Ok(body) = std::str::from_utf8(&request.body) else {
        return oauth_error("invalid_request");
    };
    let Some(pairs) = parse_urlencoded_pairs(body) else {
        return oauth_error("invalid_request");
    };
    match field(&pairs, "grant_type") {
        Some("authorization_code") => authorization_code(&pairs, request, oauth),
        Some("refresh_token") => refresh_token(&pairs, request, oauth),
        _ => oauth_error("unsupported_grant_type"),
    }
}

fn authorization_code(
    pairs: &[(String, String)],
    request: &HttpRequest,
    oauth: &OAuthRuntime,
) -> HttpResponse {
    let Some(code) = required(pairs, "code", MAX_CODE_BYTES) else {
        return oauth_error("invalid_request");
    };
    let Some(redirect_uri) = required(pairs, "redirect_uri", MAX_REDIRECT_URI_BYTES) else {
        return oauth_error("invalid_request");
    };
    let Some(code_verifier) = required(pairs, "code_verifier", MAX_CODE_BYTES) else {
        return oauth_error("invalid_request");
    };
    let Some(client_id) = required(pairs, "client_id", MAX_CLIENT_ID_BYTES) else {
        return oauth_error("invalid_request");
    };
    let Some(resource) = required(pairs, "resource", MAX_CLIENT_ID_BYTES) else {
        return oauth_error("invalid_request");
    };
    if !valid_code_verifier(code_verifier) {
        return oauth_error("invalid_request");
    }
    let binding = oauth.binding();
    let store_resource = match &oauth.resource_origin {
        super::ResourceOrigin::Fixed(_) => resource,
        super::ResourceOrigin::Request => {
            let origin = match oauth.published_origin(request) {
                Ok(origin) => origin,
                Err(_) => return oauth_error("invalid_request"),
            };
            if resource != format!("{origin}/mcp") {
                return oauth_error("invalid_request");
            }
            binding.canonical()
        }
    };
    match oauth.store.redeem_authorization_code(
        code,
        client_id,
        redirect_uri,
        store_resource,
        code_verifier,
        &oauth.binding(),
    ) {
        Ok(tokens) => token_response(tokens),
        Err(
            OAuthStoreError::InvalidToken
            | OAuthStoreError::CodeExpired
            | OAuthStoreError::BindingMismatch,
        ) => oauth_error("invalid_grant"),
        Err(_) => cache_headers(HttpResponse::error(
            503,
            "Service Unavailable",
            "token request is unavailable",
        )),
    }
}

fn refresh_token(
    pairs: &[(String, String)],
    request: &HttpRequest,
    oauth: &OAuthRuntime,
) -> HttpResponse {
    let Some(refresh) = required(pairs, "refresh_token", MAX_CODE_BYTES) else {
        return oauth_error("invalid_request");
    };
    let Some(client_id) = required(pairs, "client_id", MAX_CLIENT_ID_BYTES) else {
        return oauth_error("invalid_request");
    };
    match &oauth.resource_origin {
        super::ResourceOrigin::Fixed(_) => {
            if let Some(resource) = field(pairs, "resource")
                && (resource.len() > MAX_CLIENT_ID_BYTES || resource != oauth.binding().canonical())
            {
                return oauth_error("invalid_request");
            }
        }
        super::ResourceOrigin::Request => {
            if let Some(resource) = field(pairs, "resource") {
                if resource.len() > MAX_CLIENT_ID_BYTES {
                    return oauth_error("invalid_request");
                }
                let origin = match oauth.published_origin(request) {
                    Ok(origin) => origin,
                    Err(_) => return oauth_error("invalid_request"),
                };
                if resource != format!("{origin}/mcp") {
                    return oauth_error("invalid_request");
                }
            }
        }
    }
    match oauth
        .store
        .refresh_grant(refresh, client_id, &oauth.binding())
    {
        Ok(tokens) => token_response(tokens),
        Err(
            OAuthStoreError::InvalidToken
            | OAuthStoreError::CodeExpired
            | OAuthStoreError::BindingMismatch,
        ) => oauth_error("invalid_grant"),
        Err(_) => cache_headers(HttpResponse::error(
            503,
            "Service Unavailable",
            "token request is unavailable",
        )),
    }
}

fn valid_code_verifier(value: &str) -> bool {
    (43..=128).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~'))
}

fn required<'a>(pairs: &'a [(String, String)], name: &str, max_bytes: usize) -> Option<&'a str> {
    let value = field(pairs, name).filter(|value| !value.is_empty())?;
    (value.len() <= max_bytes).then_some(value)
}

fn field<'a>(pairs: &'a [(String, String)], name: &str) -> Option<&'a str> {
    pairs
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.as_str())
}

fn token_response(tokens: IssuedTokens) -> HttpResponse {
    let body = TokenBody {
        access_token: &tokens.access_token,
        refresh_token: &tokens.refresh_token,
        token_type: "Bearer",
        expires_in: tokens.expires_in,
    };
    cache_headers(HttpResponse::json(
        200,
        "OK",
        serde_json::to_vec(&body).expect("token JSON serializes"),
    ))
}

#[derive(Serialize)]
struct TokenBody<'a> {
    access_token: &'a str,
    refresh_token: &'a str,
    token_type: &'static str,
    expires_in: i64,
}

fn oauth_error(error: &'static str) -> HttpResponse {
    cache_headers(HttpResponse::json(
        400,
        "Bad Request",
        serde_json::to_vec(&serde_json::json!({ "error": error })).expect("error JSON serializes"),
    ))
}

fn cache_headers(response: HttpResponse) -> HttpResponse {
    response
        .with_header("Cache-Control", "no-store".to_owned())
        .with_header("Pragma", "no-cache".to_owned())
}

#[cfg(all(test, not(feature = "full-tests")))]
mod tests {
    use crate::http1::{HttpMethod, HttpRequest, HttpResponse};
    use crate::oauth::{OAuthRuntime, TOKEN_MAX_BODY_BYTES};
    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
    use sha2::{Digest, Sha256};

    const CIMD_URL: &str = "https://client.example/cimd.json";
    const ORIGIN: &str = "https://mcp.test";
    const REDIRECT: &str = "http://127.0.0.1/callback";
    const VERIFIER: &str = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-._~";

    fn journal_root() -> tempfile::TempDir {
        tempfile::Builder::new()
            .prefix("solstone-mcp-token-")
            .tempdir_in("/var/tmp")
            .unwrap()
    }

    fn runtime(journal: &tempfile::TempDir) -> OAuthRuntime {
        OAuthRuntime::new(journal.path(), ORIGIN.to_owned())
    }

    fn pkce_challenge(verifier: &str) -> String {
        URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
    }

    fn header<'a>(response: &'a HttpResponse, name: &str) -> Option<&'a str> {
        response
            .extra_headers
            .iter()
            .find(|(candidate, _)| candidate.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    fn json_body(response: &HttpResponse) -> serde_json::Value {
        serde_json::from_slice(&response.body).unwrap_or(serde_json::Value::Null)
    }

    fn form_request(body: &str) -> HttpRequest {
        HttpRequest::from_test_parts(HttpMethod::Post, Vec::new(), body.as_bytes().to_vec())
    }

    fn issue_code(oauth: &OAuthRuntime, verifier: &str) -> String {
        let client = oauth
            .store
            .register_client(CIMD_URL, vec![REDIRECT.to_owned()], None, "198.51.100.10")
            .unwrap();
        let pairing = oauth.store.generate_pairing_code().unwrap();
        let transaction = oauth
            .store
            .create_transaction(
                &client.id,
                REDIRECT,
                "https://mcp.test/mcp",
                ORIGIN,
                &pkce_challenge(verifier),
                "S256",
                None,
                "198.51.100.10",
            )
            .unwrap();
        oauth
            .store
            .complete_pairing(&transaction, &pairing.code, &oauth.binding())
            .unwrap()
            .code
    }

    #[test]
    fn authorization_code_happy_path_issues_usable_tokens() {
        let journal = journal_root();
        let oauth = runtime(&journal);
        let code = issue_code(&oauth, VERIFIER);
        let response = super::token(
            &form_request(&format!(
                "grant_type=authorization_code&code={code}&redirect_uri={REDIRECT}&code_verifier={VERIFIER}&client_id={CIMD_URL}&resource=https://mcp.test/mcp"
            )),
            &oauth,
        );
        assert_eq!(response.status, 200);
        assert_eq!(header(&response, "Cache-Control"), Some("no-store"));
        assert_eq!(header(&response, "Pragma"), Some("no-cache"));
        let body = json_body(&response);
        let access = body["access_token"].as_str().unwrap();
        let refresh = body["refresh_token"].as_str().unwrap();
        assert_eq!(body["token_type"], "Bearer");
        oauth
            .store
            .verify_access_token(access, &oauth.binding())
            .unwrap();
        oauth
            .store
            .refresh_grant(refresh, CIMD_URL, &oauth.binding())
            .unwrap();
    }

    #[test]
    fn invalid_verifier_syntax_is_invalid_request() {
        let journal = journal_root();
        let oauth = runtime(&journal);
        let short = "shortshortshortshortshortshortshortshortsh";
        assert_eq!(short.len(), 42);
        let code = issue_code(&oauth, short);
        let response = super::token(
            &form_request(&format!(
                "grant_type=authorization_code&code={code}&redirect_uri={REDIRECT}&code_verifier={short}&client_id={CIMD_URL}&resource=https://mcp.test/mcp"
            )),
            &oauth,
        );
        assert_eq!(response.status, 400);
        assert_eq!(json_body(&response)["error"], "invalid_request");
        assert_eq!(header(&response, "Cache-Control"), Some("no-store"));
        assert!(
            oauth
                .store
                .redeem_authorization_code(
                    code.as_str(),
                    CIMD_URL,
                    REDIRECT,
                    "https://mcp.test/mcp",
                    short,
                    &oauth.binding(),
                )
                .is_ok(),
            "store hash would have accepted the short verifier"
        );
    }

    #[test]
    fn refresh_token_happy_path_and_bad_resource() {
        let journal = journal_root();
        let oauth = runtime(&journal);
        let code = issue_code(&oauth, VERIFIER);
        let issued = oauth
            .store
            .redeem_authorization_code(
                code.as_str(),
                CIMD_URL,
                REDIRECT,
                "https://mcp.test/mcp",
                VERIFIER,
                &oauth.binding(),
            )
            .unwrap();
        let bad = super::token(
            &form_request(&format!(
                "grant_type=refresh_token&refresh_token={}&client_id={CIMD_URL}&resource=https://mcp.test/other",
                issued.refresh_token
            )),
            &oauth,
        );
        assert_eq!(json_body(&bad)["error"], "invalid_request");
        assert_eq!(header(&bad, "Cache-Control"), Some("no-store"));
        let ok = super::token(
            &form_request(&format!(
                "grant_type=refresh_token&refresh_token={}&client_id={CIMD_URL}",
                issued.refresh_token
            )),
            &oauth,
        );
        assert_eq!(ok.status, 200);
        assert_eq!(header(&ok, "Cache-Control"), Some("no-store"));
    }

    #[test]
    fn unsupported_grant_type_is_rejected() {
        let journal = journal_root();
        let oauth = runtime(&journal);
        let response = super::token(&form_request("grant_type=password"), &oauth);
        assert_eq!(json_body(&response)["error"], "unsupported_grant_type");
        assert_eq!(header(&response, "Cache-Control"), Some("no-store"));
    }

    #[test]
    fn ingress_bounds_are_enforced_first() {
        let journal = journal_root();
        let oauth = runtime(&journal);
        let mut oversize = b"grant_type=authorization_code&pad=".to_vec();
        oversize.resize(TOKEN_MAX_BODY_BYTES + 1, b'x');
        let too_large = super::token(
            &HttpRequest::from_test_parts(HttpMethod::Post, Vec::new(), oversize),
            &oauth,
        );
        assert_eq!(too_large.status, 413);
        assert_eq!(header(&too_large, "Cache-Control"), Some("no-store"));
        let encoded = super::token(
            &HttpRequest::from_test_parts(
                HttpMethod::Post,
                vec![("Content-Encoding".to_owned(), "gzip".to_owned())],
                b"grant_type=authorization_code".to_vec(),
            ),
            &oauth,
        );
        assert_eq!(encoded.status, 400);
        assert_eq!(header(&encoded, "Cache-Control"), Some("no-store"));
        assert!(!journal.path().join("mcp-endpoint/oauth.json").exists());
    }

    #[test]
    fn refresh_without_resource_fails_across_runtimes_and_survives_on_issuer() {
        let journal = journal_root();
        let unbound = OAuthRuntime::new(journal.path(), "https://mcp.test".to_owned());
        let bound = OAuthRuntime::new_bound(journal.path(), "http://127.0.0.1:7659".to_owned());

        // Direction 1: Unbound -> Bound
        let code_u = issue_code(&unbound, VERIFIER);
        let resp_u = super::token(
            &form_request(&format!(
                "grant_type=authorization_code&code={code_u}&redirect_uri={REDIRECT}&code_verifier={VERIFIER}&client_id={CIMD_URL}&resource=https://mcp.test/mcp"
            )),
            &unbound,
        );
        assert_eq!(resp_u.status, 200);
        let refresh_u = json_body(&resp_u)["refresh_token"]
            .as_str()
            .unwrap()
            .to_owned();

        let fail_bound = super::token(
            &form_request(&format!(
                "grant_type=refresh_token&refresh_token={refresh_u}&client_id={CIMD_URL}"
            )),
            &bound,
        );
        assert_eq!(
            fail_bound.status, 400,
            "actual cross-runtime refresh status on bound is {}",
            fail_bound.status
        );
        assert_eq!(json_body(&fail_bound)["error"], "invalid_grant");

        let ok_unbound = super::token(
            &form_request(&format!(
                "grant_type=refresh_token&refresh_token={refresh_u}&client_id={CIMD_URL}"
            )),
            &unbound,
        );
        assert_eq!(ok_unbound.status, 200);

        // Direction 2: Bound -> Unbound
        let code_b = issue_code_bound(&bound, VERIFIER);
        let resp_b = super::token(
            &form_request(&format!(
                "grant_type=authorization_code&code={code_b}&redirect_uri={REDIRECT}&code_verifier={VERIFIER}&client_id={CIMD_URL}&resource=http://127.0.0.1:7659/mcp"
            )),
            &bound,
        );
        assert_eq!(resp_b.status, 200);
        let refresh_b = json_body(&resp_b)["refresh_token"]
            .as_str()
            .unwrap()
            .to_owned();

        let fail_unbound = super::token(
            &form_request(&format!(
                "grant_type=refresh_token&refresh_token={refresh_b}&client_id={CIMD_URL}"
            )),
            &unbound,
        );
        assert_eq!(
            fail_unbound.status, 400,
            "actual cross-runtime refresh status on unbound is {}",
            fail_unbound.status
        );
        assert_eq!(json_body(&fail_unbound)["error"], "invalid_grant");

        let ok_bound = super::token(
            &form_request(&format!(
                "grant_type=refresh_token&refresh_token={refresh_b}&client_id={CIMD_URL}"
            )),
            &bound,
        );
        assert_eq!(ok_bound.status, 200);
    }

    #[test]
    fn code_redeem_with_issuer_resource_fails_across_runtimes_and_burns_code() {
        let journal = journal_root();
        let unbound = OAuthRuntime::new(journal.path(), "https://mcp.test".to_owned());
        let bound = OAuthRuntime::new_bound(journal.path(), "http://127.0.0.1:7659".to_owned());

        // Direction 1: issued on unbound (A), redeemed on bound (B) with resource = A's resource
        let code_a = issue_code(&unbound, VERIFIER);
        let cross_b = super::token(
            &form_request(&format!(
                "grant_type=authorization_code&code={code_a}&redirect_uri={REDIRECT}&code_verifier={VERIFIER}&client_id={CIMD_URL}&resource=https://mcp.test/mcp"
            )),
            &bound,
        );
        assert_eq!(
            cross_b.status, 400,
            "actual redeem status across runtimes on bound is {}",
            cross_b.status
        );
        assert_eq!(json_body(&cross_b)["error"], "invalid_grant");

        let retry_a = super::token(
            &form_request(&format!(
                "grant_type=authorization_code&code={code_a}&redirect_uri={REDIRECT}&code_verifier={VERIFIER}&client_id={CIMD_URL}&resource=https://mcp.test/mcp"
            )),
            &unbound,
        );
        assert_eq!(
            retry_a.status, 400,
            "second redeem on issuing runtime should fail because code is burned, got {}",
            retry_a.status
        );
        assert_eq!(json_body(&retry_a)["error"], "invalid_grant");

        // Direction 2: issued on bound (B), redeemed on unbound (A) with resource = B's resource
        let code_b = issue_code_bound(&bound, VERIFIER);
        let cross_a = super::token(
            &form_request(&format!(
                "grant_type=authorization_code&code={code_b}&redirect_uri={REDIRECT}&code_verifier={VERIFIER}&client_id={CIMD_URL}&resource=http://127.0.0.1:7659/mcp"
            )),
            &unbound,
        );
        assert_eq!(
            cross_a.status, 400,
            "actual redeem status across runtimes on unbound is {}",
            cross_a.status
        );
        assert_eq!(json_body(&cross_a)["error"], "invalid_grant");

        let retry_b = super::token(
            &form_request(&format!(
                "grant_type=authorization_code&code={code_b}&redirect_uri={REDIRECT}&code_verifier={VERIFIER}&client_id={CIMD_URL}&resource=http://127.0.0.1:7659/mcp"
            )),
            &bound,
        );
        assert_eq!(
            retry_b.status, 400,
            "second redeem on issuing runtime should fail because code is burned, got {}",
            retry_b.status
        );
        assert_eq!(json_body(&retry_b)["error"], "invalid_grant");
    }

    fn issue_code_bound(oauth: &OAuthRuntime, verifier: &str) -> String {
        let client_id = oauth
            .store
            .register_client(CIMD_URL, vec![REDIRECT.to_owned()], None, "198.51.100.10")
            .unwrap()
            .id;
        let challenge = pkce_challenge(verifier);
        let transaction = oauth
            .store
            .create_transaction(
                &client_id,
                REDIRECT,
                "http://127.0.0.1:7659/mcp",
                "http://127.0.0.1:7659",
                &challenge,
                "S256",
                None,
                "198.51.100.10",
            )
            .unwrap();
        let pairing = oauth.store.generate_pairing_code().unwrap();
        oauth
            .store
            .complete_pairing(&transaction, &pairing.code, &oauth.binding())
            .unwrap()
            .code
    }
}
