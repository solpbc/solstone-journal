// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Token refresh and credential manager implementation for ChatGPT auth.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use solstone_core_journal_io::LockError;

use crate::authorize::API_RESOURCE;
use crate::credential::{ChatGptCredential, CredentialError};
use crate::exchange::TOKEN_PATH;
use crate::overrides::auth_base_url;
use crate::revoke::revoke_refresh_token;
use crate::store::{
    ChatGptSignInDoc, DIRECT_USE_SCOPE, LoadResult, Tokens, access_token_lock_options,
    acquire_credential_lock, credential_lock_options, credential_path, load_credential_file,
    save_credential_file,
};
use crate::transport::{ChatGptTransport, TransportError, UreqTransport};

pub const REFRESH_TIMEOUT: Duration = Duration::from_secs(30);
pub const EXPIRATION_MARGIN_SECS: u64 = 300; // 5 minutes

#[derive(Debug, PartialEq, Eq)]
enum RefreshStep {
    Token(String),
    Withdrawn {
        refresh_token: String,
        client_id: String,
        save_failed: bool,
    },
}

pub struct ChatGptAuthManager {
    journal: PathBuf,
    transport: Arc<dyn ChatGptTransport>,
}

impl ChatGptAuthManager {
    pub fn new(journal: PathBuf, transport: Arc<dyn ChatGptTransport>) -> Self {
        Self { journal, transport }
    }

    pub fn with_default_transport(journal: PathBuf) -> Self {
        Self::new(journal, Arc::new(UreqTransport))
    }

    pub fn journal(&self) -> &Path {
        &self.journal
    }

    fn now_secs() -> u64 {
        crate::store::now_secs()
    }

    fn is_token_valid(tokens: &Tokens, now: u64) -> bool {
        tokens.expires_at > now.saturating_add(EXPIRATION_MARGIN_SECS)
    }

    fn perform_refresh(&self, doc: &mut ChatGptSignInDoc) -> Result<RefreshStep, CredentialError> {
        let Some(existing_tokens) = &doc.tokens else {
            return Err(CredentialError::SignInRequired);
        };
        let Some(registration) = &doc.registration else {
            return Err(CredentialError::Storage(credential_path(&self.journal)));
        };
        let client_id = &registration.client_id;

        let auth_base = auth_base_url();
        let token_url = format!("{auth_base}{TOKEN_PATH}");

        let mut form = BTreeMap::new();
        form.insert("grant_type".to_string(), "refresh_token".to_string());
        form.insert(
            "refresh_token".to_string(),
            existing_tokens.refresh_token.clone(),
        );
        form.insert("client_id".to_string(), client_id.clone());
        form.insert("resource".to_string(), API_RESOURCE.to_string());

        let response = match self.transport.post_form(&token_url, &form, REFRESH_TIMEOUT) {
            Ok(resp) => resp,
            Err(TransportError::Network | TransportError::Timeout) => {
                return Err(CredentialError::Network);
            }
        };

        match response.status {
            200 => {
                let Ok(json) = serde_json::from_str::<serde_json::Value>(&response.body) else {
                    return Err(CredentialError::Malformed(Some(
                        "refresh_response".to_string(),
                    )));
                };

                let Some(new_refresh) = json.get("refresh_token").and_then(|v| v.as_str()) else {
                    return Err(CredentialError::Malformed(Some(
                        "refresh_token".to_string(),
                    )));
                };
                if new_refresh.is_empty() {
                    return Err(CredentialError::Malformed(Some(
                        "refresh_token".to_string(),
                    )));
                }

                let scopes: Vec<String> =
                    if let Some(scope_str) = json.get("scope").and_then(|v| v.as_str()) {
                        scope_str
                            .split_whitespace()
                            .map(ToString::to_string)
                            .collect()
                    } else {
                        existing_tokens.scopes.clone()
                    };

                if !scopes.iter().any(|s| s == DIRECT_USE_SCOPE) {
                    doc.tokens = None;
                    doc.plan_usage_declined = true;
                    let save_failed = save_credential_file(&self.journal, doc).is_err();
                    return Ok(RefreshStep::Withdrawn {
                        refresh_token: new_refresh.to_string(),
                        client_id: client_id.clone(),
                        save_failed,
                    });
                }

                let access_token_opt = json
                    .get("access_token")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty());
                let expires_in_opt = json
                    .get("expires_in")
                    .and_then(|v| v.as_u64())
                    .filter(|v| *v > 0);

                let mut malformed_field = None;
                let (access_token, expires_at) =
                    if let (Some(at), Some(ei)) = (access_token_opt, expires_in_opt) {
                        (at.to_string(), crate::store::now_secs().saturating_add(ei))
                    } else {
                        if access_token_opt.is_none() {
                            malformed_field = Some("access_token".to_string());
                        } else {
                            malformed_field = Some("expires_in".to_string());
                        }
                        (existing_tokens.access_token.clone(), 0)
                    };

                let updated_tokens = Tokens {
                    access_token: access_token.clone(),
                    refresh_token: new_refresh.to_string(),
                    expires_at,
                    scopes,
                };

                doc.tokens = Some(updated_tokens);

                #[cfg(any(test, feature = "test-hooks"))]
                if crate::test_support::take_fail_next_grant_write() {
                    // Residual case: crash between receive and save, or response lost after OpenAI rotated.
                    return Err(CredentialError::GrantNotSaved);
                }

                if save_credential_file(&self.journal, doc).is_err() {
                    // Residual case: crash between receive and save, or response lost after OpenAI rotated.
                    return Err(CredentialError::GrantNotSaved);
                }

                if let Some(bad_field) = malformed_field {
                    return Err(CredentialError::Malformed(Some(bad_field)));
                }

                Ok(RefreshStep::Token(access_token))
            }
            400..=499 => {
                if response.status == 429 {
                    return Err(CredentialError::Unavailable);
                }
                let err_code = serde_json::from_str::<serde_json::Value>(&response.body)
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
                    });

                let is_invalid_grant = matches!(
                    err_code.as_deref(),
                    Some(
                        "invalid_grant"
                            | "invalid_refresh_token"
                            | "token_expired"
                            | "refresh_token_expired"
                            | "refresh_token_invalidated"
                            | "refresh_token_reused"
                    )
                );

                if is_invalid_grant {
                    doc.tokens = None;
                    if let Err(e) = save_credential_file(&self.journal, doc) {
                        return Err(CredentialError::Io(e.to_string()));
                    }
                    return Err(CredentialError::SignInRequired);
                }

                if err_code.as_deref() == Some("invalid_client") {
                    doc.tokens = None;
                    if let Some(reg) = &mut doc.registration {
                        reg.client_refused = true;
                    }
                    if let Err(e) = save_credential_file(&self.journal, doc) {
                        return Err(CredentialError::Io(e.to_string()));
                    }
                    return Err(CredentialError::SignInRequired);
                }

                if let Some(code) = err_code {
                    Err(CredentialError::Refused(Some(code)))
                } else {
                    Err(CredentialError::Refused(None))
                }
            }
            500..=599 => Err(CredentialError::Unavailable),
            _ => Err(CredentialError::Refused(None)),
        }
    }
}

impl ChatGptCredential for ChatGptAuthManager {
    /// Retrieve a valid access token.
    ///
    /// If an unexpired token with at least 5 minutes of remaining lifetime is available,
    /// it is returned lock-free. Otherwise, a single token refresh is attempted under the lock.
    /// Bound: up to 75s (45s lock wait + 30s network timeout).
    /// If an unknown HTTP response status is received during refresh, the owner remains signed in.
    fn access_token(&self) -> Result<String, CredentialError> {
        let now = Self::now_secs();

        // 1. Lock-free read
        match load_credential_file(&self.journal) {
            LoadResult::Present(doc) => {
                if let Some(tokens) = &doc.tokens {
                    if Self::is_token_valid(tokens, now) {
                        return Ok(tokens.access_token.clone());
                    }
                } else {
                    return Err(CredentialError::SignInRequired);
                }
            }
            LoadResult::Absent => return Err(CredentialError::SignInRequired),
            LoadResult::Unreadable => {
                return Err(CredentialError::Storage(credential_path(&self.journal)));
            }
        }

        // Lock-free check determined refresh is needed
        #[cfg(any(test, feature = "test-hooks"))]
        crate::test_support::record_refresh_marker_if_configured();

        // 2. Acquire lock
        let _lock = acquire_credential_lock(&self.journal, access_token_lock_options()).map_err(
            |e| match e {
                LockError::Timeout(_) => CredentialError::Busy,
                other => CredentialError::Io(other.to_string()),
            },
        )?;

        // 3. Re-read under lock
        let mut doc = match load_credential_file(&self.journal) {
            LoadResult::Present(doc) => doc,
            LoadResult::Absent => return Err(CredentialError::SignInRequired),
            LoadResult::Unreadable => {
                return Err(CredentialError::Storage(credential_path(&self.journal)));
            }
        };

        let now = Self::now_secs();
        if let Some(tokens) = &doc.tokens {
            if Self::is_token_valid(tokens, now) {
                return Ok(tokens.access_token.clone());
            }
        } else {
            return Err(CredentialError::SignInRequired);
        }

        // 4. Perform refresh
        let step = self.perform_refresh(&mut doc)?;
        drop(_lock);
        match step {
            RefreshStep::Token(token) => Ok(token),
            RefreshStep::Withdrawn {
                refresh_token,
                client_id,
                save_failed,
            } => {
                let auth_base = auth_base_url();
                let _ = revoke_refresh_token(
                    self.transport.as_ref(),
                    &auth_base,
                    &refresh_token,
                    &client_id,
                );
                if save_failed {
                    Err(CredentialError::GrantNotSaved)
                } else {
                    Err(CredentialError::SignInRequired)
                }
            }
        }
    }

    fn access_token_after_rejection(&self, rejected: &str) -> Result<String, CredentialError> {
        let _lock = acquire_credential_lock(&self.journal, credential_lock_options()).map_err(
            |e| match e {
                LockError::Timeout(_) => CredentialError::Busy,
                other => CredentialError::Io(other.to_string()),
            },
        )?;

        let mut doc = match load_credential_file(&self.journal) {
            LoadResult::Present(doc) => doc,
            LoadResult::Absent => return Err(CredentialError::SignInRequired),
            LoadResult::Unreadable => {
                return Err(CredentialError::Storage(credential_path(&self.journal)));
            }
        };

        let now = Self::now_secs();
        if let Some(tokens) = &doc.tokens {
            if tokens.access_token != rejected && Self::is_token_valid(tokens, now) {
                return Ok(tokens.access_token.clone());
            }
        } else {
            return Err(CredentialError::SignInRequired);
        }

        let step = self.perform_refresh(&mut doc)?;
        drop(_lock);
        match step {
            RefreshStep::Token(token) => Ok(token),
            RefreshStep::Withdrawn {
                refresh_token,
                client_id,
                save_failed,
            } => {
                let auth_base = auth_base_url();
                let _ = revoke_refresh_token(
                    self.transport.as_ref(),
                    &auth_base,
                    &refresh_token,
                    &client_id,
                );
                if save_failed {
                    Err(CredentialError::GrantNotSaved)
                } else {
                    Err(CredentialError::SignInRequired)
                }
            }
        }
    }

    fn mark_token_rejected(&self, rejected: &str) -> Result<(), CredentialError> {
        let _lock = acquire_credential_lock(&self.journal, credential_lock_options()).map_err(
            |e| match e {
                LockError::Timeout(_) => CredentialError::Busy,
                other => CredentialError::Io(other.to_string()),
            },
        )?;

        let mut doc = match load_credential_file(&self.journal) {
            LoadResult::Present(doc) => doc,
            LoadResult::Absent => return Ok(()),
            LoadResult::Unreadable => {
                return Err(CredentialError::Storage(credential_path(&self.journal)));
            }
        };

        if let Some(tokens) = &doc.tokens
            && tokens.access_token == rejected
        {
            doc.tokens = None;
            if let Err(e) = save_credential_file(&self.journal, &doc) {
                return Err(CredentialError::Io(e.to_string()));
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{ChatGptSignInDoc, Registration, Tokens};
    use crate::transport::{HttpResponse, TransportError};
    use std::sync::Mutex;

    struct MockTransport {
        resp: Mutex<Result<HttpResponse, TransportError>>,
    }

    impl MockTransport {
        fn new(resp: Result<HttpResponse, TransportError>) -> Self {
            Self {
                resp: Mutex::new(resp),
            }
        }
    }

    impl ChatGptTransport for MockTransport {
        fn post_form(
            &self,
            _url: &str,
            _form: &BTreeMap<String, String>,
            _timeout: Duration,
        ) -> Result<HttpResponse, TransportError> {
            self.resp.lock().unwrap().clone()
        }

        fn get_json(
            &self,
            _url: &str,
            _bearer_token: &str,
            _timeout: Duration,
        ) -> Result<HttpResponse, TransportError> {
            self.resp.lock().unwrap().clone()
        }
    }

    fn make_test_doc() -> ChatGptSignInDoc {
        let mut doc = ChatGptSignInDoc::new_initial("test-host".to_string());
        doc.registration = Some(Registration {
            client_id: "test-client".to_string(),
            client_refused: false,
        });
        doc.tokens = Some(Tokens {
            access_token: "old-access".to_string(),
            refresh_token: "old-refresh".to_string(),
            expires_at: 100,
            scopes: vec![DIRECT_USE_SCOPE.to_string()],
        });
        doc
    }

    #[test]
    fn refresh_missing_refresh_token_returns_malformed_no_save() {
        let temp = tempfile::tempdir().unwrap();
        let journal = temp.path().to_path_buf();
        let mut doc = make_test_doc();
        save_credential_file(&journal, &doc).unwrap();

        let resp = HttpResponse {
            status: 200,
            body: r#"{"access_token":"new-a","expires_in":3600}"#.to_string(),
            headers: BTreeMap::new(),
        };
        let transport = Arc::new(MockTransport::new(Ok(resp)));
        let manager = ChatGptAuthManager::new(journal.clone(), transport);

        let err = manager.perform_refresh(&mut doc).unwrap_err();
        assert_eq!(
            err,
            CredentialError::Malformed(Some("refresh_token".to_string()))
        );

        // Doc unchanged in storage
        let loaded = match load_credential_file(&journal) {
            LoadResult::Present(d) => d,
            _ => panic!("doc present"),
        };
        assert_eq!(loaded.tokens.unwrap().refresh_token, "old-refresh");
    }

    #[test]
    fn refresh_bad_access_token_saves_rotated_refresh_with_zero_expires_at() {
        let temp = tempfile::tempdir().unwrap();
        let journal = temp.path().to_path_buf();
        let mut doc = make_test_doc();
        save_credential_file(&journal, &doc).unwrap();

        let resp = HttpResponse {
            status: 200,
            body: r#"{"refresh_token":"new-rotated-r","expires_in":3600}"#.to_string(),
            headers: BTreeMap::new(),
        };
        let transport = Arc::new(MockTransport::new(Ok(resp)));
        let manager = ChatGptAuthManager::new(journal.clone(), transport);

        let err = manager.perform_refresh(&mut doc).unwrap_err();
        assert_eq!(
            err,
            CredentialError::Malformed(Some("access_token".to_string()))
        );

        // Rotated refresh saved with expires_at: 0 and prior access token kept
        let loaded = match load_credential_file(&journal) {
            LoadResult::Present(d) => d,
            _ => panic!("doc present"),
        };
        let tokens = loaded.tokens.unwrap();
        assert_eq!(tokens.refresh_token, "new-rotated-r");
        assert_eq!(tokens.access_token, "old-access");
        assert_eq!(tokens.expires_at, 0);
    }

    #[test]
    fn refresh_missing_direct_use_scope_clears_tokens_and_declines_plan() {
        let temp = tempfile::tempdir().unwrap();
        let journal = temp.path().to_path_buf();
        let mut doc = make_test_doc();
        save_credential_file(&journal, &doc).unwrap();

        let resp = HttpResponse {
            status: 200,
            body: r#"{"access_token":"new-a","refresh_token":"new-r","expires_in":3600,"scope":"openid profile"}"#.to_string(),
            headers: BTreeMap::new(),
        };
        let transport = Arc::new(MockTransport::new(Ok(resp)));
        let manager = ChatGptAuthManager::new(journal.clone(), transport);

        let step = manager.perform_refresh(&mut doc).unwrap();
        assert_eq!(
            step,
            RefreshStep::Withdrawn {
                refresh_token: "new-r".to_string(),
                client_id: "test-client".to_string(),
                save_failed: false,
            }
        );
        assert!(doc.plan_usage_declined);
        assert!(doc.tokens.is_none());

        let loaded = match load_credential_file(&journal) {
            LoadResult::Present(d) => d,
            _ => panic!("doc present"),
        };
        assert!(loaded.plan_usage_declined);
        assert!(loaded.tokens.is_none());
    }

    #[test]
    fn refresh_clear_token_error_codes_clear_tokens_and_save() {
        let codes = [
            "invalid_grant",
            "invalid_refresh_token",
            "token_expired",
            "refresh_token_expired",
            "refresh_token_invalidated",
            "refresh_token_reused",
        ];

        for code in codes {
            let temp = tempfile::tempdir().unwrap();
            let journal = temp.path().to_path_buf();
            let mut doc = make_test_doc();
            save_credential_file(&journal, &doc).unwrap();

            // Test string error format: {"error": "<code>"}
            let resp = HttpResponse {
                status: 400,
                body: format!(r#"{{"error":"{code}"}}"#),
                headers: BTreeMap::new(),
            };
            let transport = Arc::new(MockTransport::new(Ok(resp)));
            let manager = ChatGptAuthManager::new(journal.clone(), transport);

            let err = manager.perform_refresh(&mut doc).unwrap_err();
            assert_eq!(err, CredentialError::SignInRequired);

            let loaded = match load_credential_file(&journal) {
                LoadResult::Present(d) => d,
                _ => panic!("doc present"),
            };
            assert!(loaded.tokens.is_none(), "code {code} should clear tokens");
        }
    }

    #[test]
    fn refresh_invalid_client_clears_tokens_sets_client_refused() {
        let temp = tempfile::tempdir().unwrap();
        let journal = temp.path().to_path_buf();
        let mut doc = make_test_doc();
        save_credential_file(&journal, &doc).unwrap();

        let resp = HttpResponse {
            status: 400,
            body: r#"{"error":"invalid_client"}"#.to_string(),
            headers: BTreeMap::new(),
        };
        let transport = Arc::new(MockTransport::new(Ok(resp)));
        let manager = ChatGptAuthManager::new(journal.clone(), transport);

        let err = manager.perform_refresh(&mut doc).unwrap_err();
        assert_eq!(err, CredentialError::SignInRequired);

        let loaded = match load_credential_file(&journal) {
            LoadResult::Present(d) => d,
            _ => panic!("doc present"),
        };
        assert!(loaded.tokens.is_none());
        assert!(loaded.registration.unwrap().client_refused);
    }

    #[test]
    fn refresh_unlisted_error_codes_leave_file_unchanged_and_return_refused() {
        let unlisted_codes = [
            "invalid_token",
            "token_revoked",
            "revoked_token",
            "expired_token",
            "unauthorized_client",
        ];

        for code in unlisted_codes {
            let temp = tempfile::tempdir().unwrap();
            let journal = temp.path().to_path_buf();
            let mut doc = make_test_doc();
            save_credential_file(&journal, &doc).unwrap();
            let file_bytes_before = std::fs::read(crate::store::credential_path(&journal)).unwrap();

            let resp = HttpResponse {
                status: 400,
                body: format!(r#"{{"error":"{code}"}}"#),
                headers: BTreeMap::new(),
            };
            let transport = Arc::new(MockTransport::new(Ok(resp)));
            let manager = ChatGptAuthManager::new(journal.clone(), transport);

            let err = manager.perform_refresh(&mut doc).unwrap_err();
            assert_eq!(err, CredentialError::Refused(Some(code.to_string())));

            let file_bytes_after = std::fs::read(crate::store::credential_path(&journal)).unwrap();
            assert_eq!(
                file_bytes_before, file_bytes_after,
                "unlisted code {code} must leave credential file byte-identical"
            );
        }
    }
}
