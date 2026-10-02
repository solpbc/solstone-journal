// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! PKCE authorization URL generation for ChatGPT sign-in.

use std::fmt;

use solstone_core_auth_flow::{RandomError, code_challenge_s256, percent_encode, random_token};

use crate::store::Registration;

pub const DYNAMIC_CLIENT_ID: &str = "dynamic_agent_client";
pub const DEFAULT_SCOPE: &str =
    "openid profile email offline_access resource.invoke chatgpt.tokens.use.direct";
pub const API_RESOURCE: &str = "https://api.openai.com/v1";
/// Authorization endpoint path, as published by OpenAI's discovery document.
pub const AUTHORIZE_PATH: &str = "/api/accounts/authorize";

#[derive(Clone, PartialEq, Eq)]
pub struct AuthorizeParams {
    pub verifier: String,
    pub challenge: String,
    pub state: String,
    pub nonce: String,
    pub redirect_uri: String,
    pub authorize_url: String,
    pub client_id: String,
}

impl fmt::Debug for AuthorizeParams {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AuthorizeParams")
            .field("client_id", &self.client_id)
            .field("redirect_uri", &self.redirect_uri)
            .finish_non_exhaustive()
    }
}

pub fn build_authorize_params(
    auth_base: &str,
    redirect_uri: &str,
    host_id: &str,
    registration: Option<&Registration>,
    email: Option<&str>,
    plan_usage_declined: bool,
) -> Result<AuthorizeParams, RandomError> {
    let verifier = random_token(32)?;
    let challenge = code_challenge_s256(&verifier);
    let state = random_token(24)?;
    let nonce = random_token(24)?;

    let is_first_registration = match registration {
        None => true,
        Some(reg) => reg.client_refused,
    };

    let client_id_str = if is_first_registration {
        DYNAMIC_CLIENT_ID.to_owned()
    } else {
        registration.unwrap().client_id.clone()
    };

    let mut query_params = vec![
        ("response_type", "code".to_string()),
        ("client_id", client_id_str.clone()),
        ("redirect_uri", redirect_uri.to_string()),
        ("scope", DEFAULT_SCOPE.to_string()),
        ("resource", API_RESOURCE.to_string()),
        ("code_challenge", challenge.clone()),
        ("code_challenge_method", "S256".to_string()),
        ("state", state.clone()),
        ("nonce", nonce.clone()),
        ("ext_agent_host_id", host_id.to_string()),
    ];

    if is_first_registration {
        query_params.push(("agent_name_hint", "solstone".to_string()));
    } else {
        if let Some(hint) = email
            && !hint.is_empty()
        {
            query_params.push(("login_hint", hint.to_string()));
        }
        if plan_usage_declined {
            query_params.push(("prompt", "consent".to_string()));
        }
    }

    let query = query_params
        .into_iter()
        .map(|(key, val)| format!("{}={}", percent_encode(key), percent_encode(&val)))
        .collect::<Vec<_>>()
        .join("&");

    let authorize_url = format!("{auth_base}{AUTHORIZE_PATH}?{query}");

    Ok(AuthorizeParams {
        verifier,
        challenge,
        state,
        nonce,
        redirect_uri: redirect_uri.to_string(),
        authorize_url,
        client_id: client_id_str,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shape1_first_registration_no_reg() {
        let params = build_authorize_params(
            "https://auth.openai.com",
            "http://127.0.0.1:12345/auth/callback",
            "host-123",
            None,
            Some("ignored@example.com"),
            false,
        )
        .expect("params");

        assert_eq!(params.client_id, DYNAMIC_CLIENT_ID);
        assert!(
            params
                .authorize_url
                .contains("client_id=dynamic_agent_client")
        );
        assert!(params.authorize_url.contains("agent_name_hint=solstone"));
        assert!(
            params
                .authorize_url
                .contains("resource=https%3A%2F%2Fapi.openai.com%2Fv1")
        );
        assert!(params.authorize_url.contains("scope=openid%20profile%20email%20offline_access%20resource.invoke%20chatgpt.tokens.use.direct"));
        assert!(!params.authorize_url.contains("login_hint"));
        assert!(!params.authorize_url.contains("prompt"));
        assert!(!params.authorize_url.contains("model.request"));
        assert!(!params.authorize_url.contains("secret"));
    }

    #[test]
    fn shape2_first_registration_client_refused() {
        let reg = Registration {
            client_id: "old-refused-client".to_string(),
            client_refused: true,
        };
        let params = build_authorize_params(
            "https://auth.openai.com",
            "http://127.0.0.1:12345/auth/callback",
            "host-123",
            Some(&reg),
            Some("ignored@example.com"),
            true,
        )
        .expect("params");

        assert_eq!(params.client_id, DYNAMIC_CLIENT_ID);
        assert!(
            params
                .authorize_url
                .contains("client_id=dynamic_agent_client")
        );
        assert!(params.authorize_url.contains("agent_name_hint=solstone"));
        assert!(!params.authorize_url.contains("login_hint"));
        assert!(!params.authorize_url.contains("prompt"));
    }

    #[test]
    fn shape3_registered_with_saved_email() {
        let reg = Registration {
            client_id: "saved-client-id".to_string(),
            client_refused: false,
        };
        let params = build_authorize_params(
            "https://auth.openai.com",
            "http://127.0.0.1:12345/auth/callback",
            "host-123",
            Some(&reg),
            Some("user@example.com"),
            false,
        )
        .expect("params");

        assert_eq!(params.client_id, "saved-client-id");
        assert!(params.authorize_url.contains("client_id=saved-client-id"));
        assert!(!params.authorize_url.contains("agent_name_hint"));
        assert!(
            params
                .authorize_url
                .contains("login_hint=user%40example.com")
        );
        assert!(!params.authorize_url.contains("prompt"));
    }

    #[test]
    fn shape4_registered_plan_usage_declined() {
        let reg = Registration {
            client_id: "saved-client-id".to_string(),
            client_refused: false,
        };
        let params = build_authorize_params(
            "https://auth.openai.com",
            "http://127.0.0.1:12345/auth/callback",
            "host-123",
            Some(&reg),
            Some("user@example.com"),
            true,
        )
        .expect("params");

        assert_eq!(params.client_id, "saved-client-id");
        assert!(params.authorize_url.contains("client_id=saved-client-id"));
        assert!(!params.authorize_url.contains("agent_name_hint"));
        assert!(
            params
                .authorize_url
                .contains("login_hint=user%40example.com")
        );
        assert!(params.authorize_url.contains("prompt=consent"));
    }
}
