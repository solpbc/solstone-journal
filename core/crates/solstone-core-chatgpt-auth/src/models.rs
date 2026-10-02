// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Models endpoint client for ChatGPT.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::credential::{ChatGptCredential, CredentialError};
use crate::overrides::api_base_url;
use crate::transport::{ChatGptTransport, HttpResponse, TransportError};

pub const MODELS_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatGptModel {
    pub slug: String,
    pub display_name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub priority: Option<i32>,
}

#[derive(Debug, Deserialize)]
struct ModelsRawItem {
    slug: String,
    #[serde(default)]
    display_name: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    visibility: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    priority: Option<i32>,
}

#[derive(Debug, Deserialize)]
struct ModelsResponse {
    models: Vec<ModelsRawItem>,
}

fn parse_provider_error_code(body: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|json| {
            json.get("error")
                .and_then(|e| {
                    if let Some(s) = e.as_str() {
                        Some(s.to_string())
                    } else if let Some(obj) = e.as_object() {
                        obj.get("code")
                            .and_then(|c| c.as_str())
                            .map(ToString::to_string)
                    } else {
                        None
                    }
                })
                .or_else(|| {
                    json.get("code")
                        .and_then(|c| c.as_str())
                        .map(ToString::to_string)
                })
        })
}

pub fn fetch_models(
    auth: &dyn ChatGptCredential,
    transport: &dyn ChatGptTransport,
    timeout: Duration,
) -> Result<Vec<ChatGptModel>, CredentialError> {
    let api_base = api_base_url();
    let url = format!("{api_base}/v1/models");

    let token = auth.access_token()?;
    let response = match transport.get_json(&url, &token, timeout) {
        Ok(resp) => resp,
        Err(TransportError::Network | TransportError::Timeout) => {
            return Err(CredentialError::Network);
        }
    };

    if response.status == 401 {
        let err_code = parse_provider_error_code(&response.body);
        if matches!(
            err_code.as_deref(),
            Some("subscription_sharing_invalid_user")
                | Some("subscription_sharing_v2_invalid_user")
        ) {
            auth.mark_token_rejected(&token)?;
            return Err(CredentialError::SignInRequired);
        }

        // First 401: call access_token_after_rejection once and retry
        let new_token = auth.access_token_after_rejection(&token)?;
        let retry_response = match transport.get_json(&url, &new_token, timeout) {
            Ok(resp) => resp,
            Err(TransportError::Network | TransportError::Timeout) => {
                return Err(CredentialError::Network);
            }
        };
        if retry_response.status == 401 {
            auth.mark_token_rejected(&new_token)?;
            return Err(CredentialError::SignInRequired);
        }
        return classify_models_response(retry_response, auth, &new_token);
    }

    classify_models_response(response, auth, &token)
}

pub fn classify_models_response(
    response: HttpResponse,
    auth: &dyn ChatGptCredential,
    token: &str,
) -> Result<Vec<ChatGptModel>, CredentialError> {
    match response.status {
        200 => {
            let parsed: ModelsResponse = serde_json::from_str(&response.body)
                .map_err(|_| CredentialError::Malformed(Some("models".to_string())))?;

            let models: Vec<ChatGptModel> = parsed
                .models
                .into_iter()
                .filter(|m| m.visibility.as_deref() == Some("list"))
                .map(|m| {
                    let display_name = m.display_name.or(m.title).unwrap_or_else(|| m.slug.clone());
                    ChatGptModel {
                        slug: m.slug,
                        display_name,
                        description: m.description,
                        priority: m.priority,
                    }
                })
                .collect();

            Ok(models)
        }
        400..=403 => {
            let err_code = parse_provider_error_code(&response.body);

            match err_code.as_deref() {
                Some("subscription_sharing_invalid_user")
                | Some("subscription_sharing_v2_invalid_user") => {
                    auth.mark_token_rejected(token)?;
                    Err(CredentialError::SignInRequired)
                }
                Some("subscription_sharing_user_not_eligible")
                | Some("subscription_sharing_v2_user_not_eligible") => {
                    Err(CredentialError::NotEligible)
                }
                _ if response.status == 403 && err_code.is_none() => {
                    Err(CredentialError::NotEligible)
                }
                _ => Err(CredentialError::Refused(err_code)),
            }
        }
        429 | 500..=599 => Err(CredentialError::Unavailable),
        _ => Err(CredentialError::Refused(None)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct DummyAuth;
    impl ChatGptCredential for DummyAuth {
        fn access_token(&self) -> Result<String, CredentialError> {
            Ok("dummy-token".to_string())
        }
        fn access_token_after_rejection(&self, _rejected: &str) -> Result<String, CredentialError> {
            Ok("dummy-retried-token".to_string())
        }
        fn mark_token_rejected(&self, _rejected: &str) -> Result<(), CredentialError> {
            Ok(())
        }
    }

    #[derive(Default)]
    struct MockAuth {
        access_token_calls: AtomicUsize,
        rejection_calls: Mutex<Vec<String>>,
        mark_rejected_calls: Mutex<Vec<String>>,
    }

    impl ChatGptCredential for MockAuth {
        fn access_token(&self) -> Result<String, CredentialError> {
            self.access_token_calls.fetch_add(1, Ordering::SeqCst);
            Ok("orig-token".to_string())
        }
        fn access_token_after_rejection(&self, rejected: &str) -> Result<String, CredentialError> {
            self.rejection_calls
                .lock()
                .unwrap()
                .push(rejected.to_string());
            Ok("retried-token".to_string())
        }
        fn mark_token_rejected(&self, rejected: &str) -> Result<(), CredentialError> {
            self.mark_rejected_calls
                .lock()
                .unwrap()
                .push(rejected.to_string());
            Ok(())
        }
    }

    struct MockTransport {
        responses: Mutex<Vec<HttpResponse>>,
    }

    impl ChatGptTransport for MockTransport {
        fn post_form(
            &self,
            _url: &str,
            _form: &BTreeMap<String, String>,
            _timeout: Duration,
        ) -> Result<HttpResponse, TransportError> {
            unimplemented!()
        }
        fn get_json(
            &self,
            _url: &str,
            _token: &str,
            _timeout: Duration,
        ) -> Result<HttpResponse, TransportError> {
            let mut guard = self.responses.lock().unwrap();
            if guard.is_empty() {
                return Err(TransportError::Network);
            }
            Ok(guard.remove(0))
        }
    }

    #[test]
    fn parse_models_filters_exact_list_and_rejects_data_array() {
        let auth = DummyAuth;
        let body = r#"{
            "models": [
                {
                    "slug": "gpt-4o",
                    "display_name": "GPT-4o",
                    "visibility": "list",
                    "description": "Smart model"
                },
                {
                    "slug": "gpt-hidden",
                    "display_name": "Hidden Model",
                    "visibility": "hide"
                },
                {
                    "slug": "gpt-no-vis",
                    "display_name": "No Vis Model"
                }
            ]
        }"#;

        let response = HttpResponse {
            status: 200,
            body: body.to_string(),
            headers: BTreeMap::new(),
        };

        let models = classify_models_response(response, &auth, "dummy").expect("parse models");
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].slug, "gpt-4o");
        assert_eq!(models[0].display_name, "GPT-4o");

        // Reject data array format
        let bad_body = r#"{"data":[{"id":"gpt-4"}]}"#;
        let bad_resp = HttpResponse {
            status: 200,
            body: bad_body.to_string(),
            headers: BTreeMap::new(),
        };
        assert!(matches!(
            classify_models_response(bad_resp, &auth, "dummy"),
            Err(CredentialError::Malformed(_))
        ));
    }

    #[test]
    fn fetch_models_invalid_user_401_does_not_retry_and_marks_token_rejected() {
        for code in [
            "subscription_sharing_invalid_user",
            "subscription_sharing_v2_invalid_user",
        ] {
            let auth = MockAuth::default();
            let transport = MockTransport {
                responses: Mutex::new(vec![HttpResponse {
                    status: 401,
                    body: format!(r#"{{"error":{{"code":"{code}"}}}}"#),
                    headers: BTreeMap::new(),
                }]),
            };

            let res = fetch_models(&auth, &transport, Duration::from_secs(5));
            assert!(matches!(res, Err(CredentialError::SignInRequired)));
            assert_eq!(auth.access_token_calls.load(Ordering::SeqCst), 1);
            assert!(
                auth.rejection_calls.lock().unwrap().is_empty(),
                "access_token_after_rejection must not be called for {code}"
            );
            assert_eq!(
                *auth.mark_rejected_calls.lock().unwrap(),
                vec!["orig-token".to_string()]
            );
        }
    }

    #[test]
    fn fetch_models_generic_401_retries_and_succeeds() {
        let auth = MockAuth::default();
        let transport = MockTransport {
            responses: Mutex::new(vec![
                HttpResponse {
                    status: 401,
                    body: "{}".to_string(),
                    headers: BTreeMap::new(),
                },
                HttpResponse {
                    status: 200,
                    body: r#"{
                        "models": [
                            {
                                "slug": "gpt-4o",
                                "display_name": "GPT-4o",
                                "visibility": "list"
                            }
                        ]
                    }"#
                    .to_string(),
                    headers: BTreeMap::new(),
                },
            ]),
        };

        let res = fetch_models(&auth, &transport, Duration::from_secs(5));
        let models = res.expect("models after 401 retry");
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].slug, "gpt-4o");
        assert_eq!(auth.access_token_calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            *auth.rejection_calls.lock().unwrap(),
            vec!["orig-token".to_string()]
        );
        assert!(
            auth.mark_rejected_calls.lock().unwrap().is_empty(),
            "mark_token_rejected must not be called on successful retry"
        );
    }
}
