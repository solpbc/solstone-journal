// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Credential seam, sign-out and token-free surface tests over an injected fake OpenAI.

use std::collections::{BTreeMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use serde_json::json;
use solstone_core_auth_flow::{base64_url_no_pad, random_token};
use solstone_core_journal_io::LockOptions;

use crate::attempt::{
    ATTEMPT_EXPIRATION_SECS, AttemptState, AttemptStatus, SignInAttempt, active_attempt_named,
    attempt_status, finish_active_sign_in, finish_sign_in, generate_host_id, get_status,
    registration, set_active_attempt, sign_out,
};
use crate::authorize::{DYNAMIC_CLIENT_ID, build_authorize_params};
use crate::credential::{ChatGptCredential, ClosedOutcome, CredentialError};
use crate::overrides::auth_base_url;
use crate::refresh::ChatGptAuthManager;
use crate::store::{
    ChatGptSignInDoc, DIRECT_USE_SCOPE, LoadResult, Registration, Tokens, acquire_credential_lock,
    credential_lock_options, credential_path, load_credential_file, now_secs, save_credential_file,
    with_test_access_token_lock_timeout,
};
use crate::test_support::{set_fail_next_grant_write, write_test_credential};
use crate::transport::{ChatGptTransport, HttpResponse, TransportError};

/// The pending-attempt registry is process-wide, so every test that sets the active attempt
/// holds this.
static ATTEMPT_REGISTRY: Mutex<()> = Mutex::new(());

pub(crate) fn hold_attempt_registry() -> std::sync::MutexGuard<'static, ()> {
    ATTEMPT_REGISTRY
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

pub(crate) const TEST_CLIENT: &str = "oaiapp_test";
pub(crate) const TEST_HOST: &str = "solstone-test-host";
pub(crate) const TEST_SUBJECT: &str = "user-sub-123";
pub(crate) const TEST_REDIRECT_URI: &str = "http://127.0.0.1:1455/auth/callback";

/// What the fake revocation endpoint saw when a revoke arrived.
#[derive(Debug, Clone)]
pub(crate) struct RevokeSeen {
    pub token: String,
    pub client_id: String,
    pub file_held_tokens: bool,
    pub lock_was_free: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Held {
    Refresh,
    Exchange,
}

struct Gate {
    kind: Held,
    entered: mpsc::Sender<()>,
    release: mpsc::Receiver<()>,
}

#[derive(Default)]
struct FakeState {
    codes: BTreeMap<String, (String, String)>,
    exchanges: Vec<BTreeMap<String, String>>,
    refreshes: Vec<BTreeMap<String, String>>,
    revokes: Vec<RevokeSeen>,
    refresh_script: VecDeque<Result<HttpResponse, TransportError>>,
    revoke_script: VecDeque<Result<u16, TransportError>>,
    urls: Vec<String>,
    exchange_scope: Option<String>,
    exchange_refusal: Option<String>,
}

type DuringExchange = Box<dyn FnOnce() + Send>;

/// An in-process stand-in for the OpenAI token and revocation endpoints.
pub(crate) struct FakeOpenAi {
    journal: PathBuf,
    state: Mutex<FakeState>,
    gate: Mutex<Option<Gate>>,
    during_exchange: Mutex<Option<DuringExchange>>,
}

pub(crate) fn response(status: u16, body: &str) -> HttpResponse {
    HttpResponse {
        status,
        body: body.to_string(),
        headers: BTreeMap::new(),
    }
}

pub(crate) fn id_token(client_id: &str, nonce: &str, subject: &str) -> String {
    let payload = json!({
        "iss": "https://auth.openai.com",
        "aud": [client_id],
        "exp": 4_000_000_000u64,
        "nonce": nonce,
        "sub": subject,
        "email": "user@example.com",
    });
    format!(
        "{}.{}.",
        base64_url_no_pad(br#"{"alg":"none"}"#),
        base64_url_no_pad(&serde_json::to_vec(&payload).expect("payload serializes"))
    )
}

impl FakeOpenAi {
    pub(crate) fn new(journal: &Path) -> Arc<Self> {
        Arc::new(Self {
            journal: journal.to_path_buf(),
            state: Mutex::new(FakeState::default()),
            gate: Mutex::new(None),
            during_exchange: Mutex::new(None),
        })
    }

    /// Accept `code` for an exchange, answering with an ID token for `nonce` and `subject`.
    pub(crate) fn accept_code(&self, code: &str, nonce: &str, subject: &str) {
        self.state
            .lock()
            .unwrap()
            .codes
            .insert(code.to_string(), (nonce.to_string(), subject.to_string()));
    }

    pub(crate) fn script_refresh(&self, answer: Result<HttpResponse, TransportError>) {
        self.state.lock().unwrap().refresh_script.push_back(answer);
    }

    /// Answer exchanges with this `scope` instead of one granting plan usage.
    pub(crate) fn set_exchange_scope(&self, scope: &str) {
        self.state.lock().unwrap().exchange_scope = Some(scope.to_string());
    }

    /// Refuse every exchange with this OAuth `error` code.
    pub(crate) fn refuse_exchanges(&self, code: &str) {
        self.state.lock().unwrap().exchange_refusal = Some(code.to_string());
    }

    /// Run `action` while the next exchange is in flight, before it answers.
    pub(crate) fn during_next_exchange(&self, action: impl FnOnce() + Send + 'static) {
        *self.during_exchange.lock().unwrap() = Some(Box::new(action));
    }

    pub(crate) fn urls(&self) -> Vec<String> {
        self.state.lock().unwrap().urls.clone()
    }

    pub(crate) fn script_revoke(&self, answer: Result<u16, TransportError>) {
        self.state.lock().unwrap().revoke_script.push_back(answer);
    }

    /// Hold the next call of `kind` until the returned sender fires.
    #[cfg_attr(not(feature = "full-tests"), allow(dead_code))]
    pub(crate) fn hold_next(&self, kind: Held) -> (mpsc::Receiver<()>, mpsc::Sender<()>) {
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        *self.gate.lock().unwrap() = Some(Gate {
            kind,
            entered: entered_tx,
            release: release_rx,
        });
        (entered_rx, release_tx)
    }

    pub(crate) fn exchanges(&self) -> Vec<BTreeMap<String, String>> {
        self.state.lock().unwrap().exchanges.clone()
    }

    pub(crate) fn refreshes(&self) -> Vec<BTreeMap<String, String>> {
        self.state.lock().unwrap().refreshes.clone()
    }

    pub(crate) fn revokes(&self) -> Vec<RevokeSeen> {
        self.state.lock().unwrap().revokes.clone()
    }

    fn pass_gate(&self, kind: Held) {
        let gate = {
            let mut slot = self.gate.lock().unwrap();
            if slot.as_ref().is_some_and(|gate| gate.kind == kind) {
                slot.take()
            } else {
                None
            }
        };
        if let Some(gate) = gate {
            let _ = gate.entered.send(());
            let _ = gate.release.recv();
        }
    }
}

impl ChatGptTransport for FakeOpenAi {
    fn post_form(
        &self,
        url: &str,
        form: &BTreeMap<String, String>,
        _timeout: Duration,
    ) -> Result<HttpResponse, TransportError> {
        self.state.lock().unwrap().urls.push(url.to_string());
        if url.ends_with("/api/accounts/oauth/revoke") {
            let file_held_tokens = matches!(
                load_credential_file(&self.journal),
                LoadResult::Present(ref doc) if doc.tokens.is_some()
            );
            let lock_was_free = acquire_credential_lock(
                &self.journal,
                LockOptions {
                    timeout: Duration::ZERO,
                    ..Default::default()
                },
            )
            .is_ok();
            let mut state = self.state.lock().unwrap();
            state.revokes.push(RevokeSeen {
                token: form.get("token").cloned().unwrap_or_default(),
                client_id: form.get("client_id").cloned().unwrap_or_default(),
                file_held_tokens,
                lock_was_free,
            });
            return state
                .revoke_script
                .pop_front()
                .unwrap_or(Ok(200))
                .map(|status| response(status, ""));
        }
        if !url.ends_with("/api/accounts/oauth/token") {
            return Ok(response(404, ""));
        }
        match form.get("grant_type").map(String::as_str) {
            Some("refresh_token") => {
                let (count, scripted) = {
                    let mut state = self.state.lock().unwrap();
                    state.refreshes.push(form.clone());
                    (state.refreshes.len(), state.refresh_script.pop_front())
                };
                self.pass_gate(Held::Refresh);
                scripted.unwrap_or_else(|| {
                    Ok(response(
                        200,
                        &json!({
                            "access_token": format!("tok-refreshed-{count}"),
                            "refresh_token": format!("rt-refreshed-{count}"),
                            "expires_in": 3600,
                            "scope": format!("openid profile email {DIRECT_USE_SCOPE}"),
                        })
                        .to_string(),
                    ))
                })
            }
            Some("authorization_code") => {
                let client_id = form.get("client_id").cloned().unwrap_or_default();
                let code = form.get("code").cloned().unwrap_or_default();
                let (accepted, scope, refusal) = {
                    let mut state = self.state.lock().unwrap();
                    state.exchanges.push(form.clone());
                    (
                        state.codes.get(&code).cloned(),
                        state.exchange_scope.clone().unwrap_or_else(|| {
                            format!("openid profile email offline_access {DIRECT_USE_SCOPE}")
                        }),
                        state.exchange_refusal.clone(),
                    )
                };
                self.pass_gate(Held::Exchange);
                let during = self.during_exchange.lock().unwrap().take();
                if let Some(action) = during {
                    action();
                }
                if let Some(code) = refusal {
                    return Ok(response(400, &json!({ "error": code }).to_string()));
                }
                let Some((nonce, subject)) = accepted else {
                    return Ok(response(400, r#"{"error":"invalid_grant"}"#));
                };
                Ok(response(
                    200,
                    &json!({
                        "token_type": "Bearer",
                        "access_token": format!("tok-{client_id}-{code}"),
                        "refresh_token": format!("rt-{client_id}-{code}"),
                        "expires_in": 3600,
                        "scope": scope,
                        "id_token": id_token(&client_id, &nonce, &subject),
                    })
                    .to_string(),
                ))
            }
            _ => Ok(response(400, r#"{"error":"unsupported_grant_type"}"#)),
        }
    }

    fn get_json(
        &self,
        _url: &str,
        _bearer_token: &str,
        _timeout: Duration,
    ) -> Result<HttpResponse, TransportError> {
        Ok(response(404, ""))
    }
}

pub(crate) fn journal_dir() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(dir.path().join("config")).expect("config dir");
    dir
}

pub(crate) fn read_doc(journal: &Path) -> ChatGptSignInDoc {
    match load_credential_file(journal) {
        LoadResult::Present(doc) => doc,
        other => panic!("credential file not present: {other:?}"),
    }
}

pub(crate) fn file_bytes(journal: &Path) -> Vec<u8> {
    std::fs::read(credential_path(journal)).expect("credential file reads")
}

/// A signed-in file for the test client holding `access`/`refresh` until `expires_at`.
pub(crate) fn write_signed_in(journal: &Path, access: &str, refresh: &str, expires_at: u64) {
    let doc = ChatGptSignInDoc {
        version: 1,
        host_id: TEST_HOST.to_string(),
        epoch: 1,
        sign_in_id: Some("sid-1".to_string()),
        registration: Some(Registration {
            client_id: TEST_CLIENT.to_string(),
            client_refused: false,
        }),
        subject: Some(TEST_SUBJECT.to_string()),
        email: Some("user@example.com".to_string()),
        plan_usage_declined: false,
        tokens: Some(Tokens {
            access_token: access.to_string(),
            refresh_token: refresh.to_string(),
            expires_at,
            scopes: vec!["openid".to_string(), DIRECT_USE_SCOPE.to_string()],
        }),
    };
    save_credential_file(journal, &doc).expect("credential file saves");
}

/// An attempt begun by another journal process: it shares the credential file, but not this
/// process's registry of pending attempts.
pub(crate) fn begin_attempt_elsewhere(journal: &Path) -> SignInAttempt {
    let _lock = acquire_credential_lock(journal, credential_lock_options()).expect("lock");
    let doc = match load_credential_file(journal) {
        LoadResult::Present(doc) => doc,
        LoadResult::Absent => {
            let doc = ChatGptSignInDoc::new_initial(TEST_HOST.to_string());
            save_credential_file(journal, &doc).expect("initial file saves");
            doc
        }
        LoadResult::Unreadable => panic!("credential file unreadable"),
    };
    let params = build_authorize_params(
        &auth_base_url(),
        TEST_REDIRECT_URI,
        &doc.host_id,
        doc.registration.as_ref(),
        doc.email.as_deref(),
        doc.plan_usage_declined,
    )
    .expect("authorize params");
    SignInAttempt {
        attempt_id: random_token(16).expect("attempt id"),
        verifier: params.verifier,
        state: params.state,
        nonce: params.nonce,
        redirect_uri: params.redirect_uri,
        client_id: params.client_id,
        authorize_url: params.authorize_url,
        begin_epoch: doc.epoch,
        begin_registration: doc.registration.clone(),
        created_at_unix: now_secs(),
        listener: None,
    }
}

/// The address-bar text after consent, as an owner would paste it back.
pub(crate) fn pasted_callback(
    attempt: &SignInAttempt,
    code: &str,
    client_id: Option<&str>,
) -> String {
    let mut url = format!(
        "http://127.0.0.1:1455/auth/callback?code={code}&scope=openid&state={}",
        attempt.state
    );
    if let Some(client_id) = client_id {
        url.push_str(&format!("&client_id={client_id}"));
    }
    url
}

pub(crate) fn finish_pasted(
    journal: &Path,
    fake: &FakeOpenAi,
    attempt: &SignInAttempt,
    code: &str,
    client_id: Option<&str>,
    subject: &str,
) -> Result<ClosedOutcome, ClosedOutcome> {
    fake.accept_code(code, &attempt.nonce, subject);
    finish_sign_in(
        journal,
        fake,
        attempt,
        Some(&pasted_callback(attempt, code, client_id)),
        Duration::from_secs(5),
    )
    .map(|result| result.outcome)
}

fn manager(journal: &Path, fake: &Arc<FakeOpenAi>) -> ChatGptAuthManager {
    ChatGptAuthManager::new(journal.to_path_buf(), fake.clone())
}

fn refresh_answer(status: u16, body: serde_json::Value) -> Result<HttpResponse, TransportError> {
    Ok(response(status, &body.to_string()))
}

#[test]
fn refresh_grant_received_but_not_saved_is_grant_not_saved() {
    let dir = journal_dir();
    let journal = dir.path();
    write_signed_in(journal, "tok-a", "rt-a", 0);
    let before = file_bytes(journal);
    let fake = FakeOpenAi::new(journal);
    let auth = manager(journal, &fake);

    set_fail_next_grant_write(true);
    let result = auth.access_token();
    set_fail_next_grant_write(false);
    assert_eq!(result, Err(CredentialError::GrantNotSaved));
    assert_eq!(fake.refreshes().len(), 1);
    assert_eq!(file_bytes(journal), before);

    set_fail_next_grant_write(true);
    let result = auth.access_token_after_rejection("tok-a");
    set_fail_next_grant_write(false);
    assert_eq!(result, Err(CredentialError::GrantNotSaved));
    assert_eq!(fake.refreshes().len(), 2);
    assert_eq!(file_bytes(journal), before);
    assert!(get_status(journal).expect("status").signed_in);
}

#[test]
fn rejection_of_the_stored_token_refreshes() {
    let dir = journal_dir();
    let journal = dir.path();
    write_signed_in(journal, "tok-a", "rt-a", now_secs() + 3600);
    let fake = FakeOpenAi::new(journal);

    let token = manager(journal, &fake)
        .access_token_after_rejection("tok-a")
        .expect("refreshed token");
    assert_eq!(token, "tok-refreshed-1");
    let grants = fake.refreshes();
    assert_eq!(grants.len(), 1);
    assert_eq!(grants[0]["refresh_token"], "rt-a");
    assert_eq!(grants[0]["client_id"], TEST_CLIENT);
    let tokens = read_doc(journal).tokens.expect("tokens kept");
    assert_eq!(tokens.access_token, "tok-refreshed-1");
    assert_eq!(tokens.refresh_token, "rt-refreshed-1");
}

#[test]
fn rejection_of_the_stored_token_answered_invalid_grant_requires_sign_in() {
    let dir = journal_dir();
    let journal = dir.path();
    write_signed_in(journal, "tok-a", "rt-a", now_secs() + 3600);
    let fake = FakeOpenAi::new(journal);
    fake.script_refresh(refresh_answer(400, json!({"error": "invalid_grant"})));

    let result = manager(journal, &fake).access_token_after_rejection("tok-a");
    assert_eq!(result, Err(CredentialError::SignInRequired));
    assert_eq!(fake.refreshes().len(), 1);
    let doc = read_doc(journal);
    assert!(doc.tokens.is_none());
    assert_eq!(doc.host_id, TEST_HOST);
    assert_eq!(
        doc.registration.expect("registration kept").client_id,
        TEST_CLIENT
    );
    assert_eq!(doc.sign_in_id.as_deref(), Some("sid-1"));
    assert!(!get_status(journal).expect("status").signed_in);
}

#[test]
fn rejection_of_an_older_token_returns_a_fresh_stored_token_without_refresh() {
    let dir = journal_dir();
    let journal = dir.path();
    write_signed_in(journal, "tok-b", "rt-b", now_secs() + 3600);
    let before = file_bytes(journal);
    let fake = FakeOpenAi::new(journal);

    let token = manager(journal, &fake)
        .access_token_after_rejection("tok-a")
        .expect("stored token");
    assert_eq!(token, "tok-b");
    assert!(fake.refreshes().is_empty());
    assert_eq!(file_bytes(journal), before);
}

#[test]
fn rejection_of_an_older_token_refreshes_a_stored_token_near_expiry() {
    let dir = journal_dir();
    let journal = dir.path();
    write_signed_in(journal, "tok-b", "rt-b", now_secs() + 60);
    let fake = FakeOpenAi::new(journal);

    let token = manager(journal, &fake)
        .access_token_after_rejection("tok-a")
        .expect("refreshed token");
    assert_eq!(token, "tok-refreshed-1");
    let grants = fake.refreshes();
    assert_eq!(grants.len(), 1);
    assert_eq!(grants[0]["refresh_token"], "rt-b");
}

#[test]
fn marking_a_token_rejected_clears_only_the_token_still_stored() {
    let dir = journal_dir();
    let journal = dir.path();
    write_signed_in(journal, "tok-b", "rt-b", now_secs() + 3600);
    let before = file_bytes(journal);
    let fake = FakeOpenAi::new(journal);
    let auth = manager(journal, &fake);

    auth.mark_token_rejected("tok-a").expect("older token");
    assert_eq!(file_bytes(journal), before);
    assert!(get_status(journal).expect("status").signed_in);

    auth.mark_token_rejected("tok-b").expect("stored token");
    let doc = read_doc(journal);
    assert!(doc.tokens.is_none());
    assert_eq!(
        doc.registration.expect("registration kept").client_id,
        TEST_CLIENT
    );
    assert_eq!(doc.sign_in_id.as_deref(), Some("sid-1"));
    assert!(!get_status(journal).expect("status").signed_in);
    assert!(fake.refreshes().is_empty());
    assert!(fake.revokes().is_empty());
}

#[test]
fn refresh_error_clears_keep_the_sign_in_id() {
    let answers = [
        refresh_answer(400, json!({"error": "invalid_grant"})),
        refresh_answer(401, json!({"error": {"code": "refresh_token_expired"}})),
        refresh_answer(400, json!({"error": "invalid_client"})),
        refresh_answer(
            200,
            json!({
                "access_token": "tok-withdrawn",
                "refresh_token": "rt-withdrawn",
                "expires_in": 3600,
                "scope": "openid profile",
            }),
        ),
    ];
    for answer in answers {
        let dir = journal_dir();
        let journal = dir.path();
        write_signed_in(journal, "tok-a", "rt-a", 0);
        let fake = FakeOpenAi::new(journal);
        fake.script_refresh(answer);

        let result = manager(journal, &fake).access_token();
        assert_eq!(result, Err(CredentialError::SignInRequired));
        let doc = read_doc(journal);
        assert!(doc.tokens.is_none());
        assert_eq!(doc.sign_in_id.as_deref(), Some("sid-1"));
        assert_eq!(doc.host_id, TEST_HOST);
        assert!(doc.registration.is_some());
    }
}

#[test]
fn sign_in_id_is_new_after_each_sign_in_and_cleared_by_sign_out() {
    let dir = journal_dir();
    let journal = dir.path();
    let fake = FakeOpenAi::new(journal);

    let first = begin_attempt_elsewhere(journal);
    assert_eq!(first.client_id, DYNAMIC_CLIENT_ID);
    assert_eq!(
        finish_pasted(
            journal,
            &fake,
            &first,
            "ac_1",
            Some(TEST_CLIENT),
            TEST_SUBJECT
        ),
        Ok(ClosedOutcome::SignedIn)
    );
    let first_id = read_doc(journal).sign_in_id.expect("sign-in id written");

    let auth = manager(journal, &fake);
    auth.mark_token_rejected(&format!("tok-{TEST_CLIENT}-ac_1"))
        .expect("rejected");
    assert_eq!(
        read_doc(journal).sign_in_id.as_deref(),
        Some(first_id.as_str())
    );

    sign_out(journal, fake.as_ref(), false).expect("sign out");
    assert_eq!(read_doc(journal).sign_in_id, None);

    let second = begin_attempt_elsewhere(journal);
    assert_eq!(second.client_id, TEST_CLIENT);
    assert_eq!(
        finish_pasted(journal, &fake, &second, "ac_2", None, TEST_SUBJECT),
        Ok(ClosedOutcome::SignedIn)
    );
    let second_id = read_doc(journal).sign_in_id.expect("sign-in id written");
    assert_ne!(second_id, first_id);

    let third = begin_attempt_elsewhere(journal);
    assert_eq!(
        finish_pasted(journal, &fake, &third, "ac_3", None, TEST_SUBJECT),
        Ok(ClosedOutcome::SignedIn)
    );
    let third_id = read_doc(journal).sign_in_id.expect("sign-in id written");
    assert_ne!(third_id, second_id);

    sign_out(journal, fake.as_ref(), true).expect("sign out with forget");
    assert_eq!(read_doc(journal).sign_in_id, None);
}

#[test]
fn sign_in_after_a_declined_plan_clears_the_declined_mark() {
    let dir = journal_dir();
    let journal = dir.path();
    let mut doc = ChatGptSignInDoc::new_initial(TEST_HOST.to_string());
    doc.registration = Some(Registration {
        client_id: TEST_CLIENT.to_string(),
        client_refused: false,
    });
    doc.subject = Some(TEST_SUBJECT.to_string());
    doc.plan_usage_declined = true;
    save_credential_file(journal, &doc).expect("declined file saves");
    let fake = FakeOpenAi::new(journal);

    let attempt = begin_attempt_elsewhere(journal);
    assert!(attempt.authorize_url.contains("prompt=consent"));
    assert_eq!(
        finish_pasted(journal, &fake, &attempt, "ac_1", None, TEST_SUBJECT),
        Ok(ClosedOutcome::SignedIn)
    );
    let doc = read_doc(journal);
    assert!(doc.tokens.is_some());
    assert!(!doc.plan_usage_declined);
    assert!(!get_status(journal).expect("status").plan_usage_declined);
}

#[test]
fn a_new_registration_after_a_refused_client_accepts_another_account() {
    let dir = journal_dir();
    let journal = dir.path();
    let mut doc = ChatGptSignInDoc::new_initial(TEST_HOST.to_string());
    doc.registration = Some(Registration {
        client_id: "oaiapp_refused".to_string(),
        client_refused: true,
    });
    doc.subject = Some(TEST_SUBJECT.to_string());
    doc.email = Some("user@example.com".to_string());
    save_credential_file(journal, &doc).expect("refused file saves");
    let fake = FakeOpenAi::new(journal);

    let attempt = begin_attempt_elsewhere(journal);
    assert_eq!(attempt.client_id, DYNAMIC_CLIENT_ID);
    assert_eq!(
        finish_pasted(
            journal,
            &fake,
            &attempt,
            "ac_1",
            Some("oaiapp_new"),
            "user-sub-other"
        ),
        Ok(ClosedOutcome::SignedIn)
    );
    let doc = read_doc(journal);
    assert_eq!(
        doc.registration,
        Some(Registration {
            client_id: "oaiapp_new".to_string(),
            client_refused: false,
        })
    );
    assert_eq!(doc.subject.as_deref(), Some("user-sub-other"));
    assert_eq!(
        doc.tokens.expect("tokens saved").access_token,
        "tok-oaiapp_new-ac_1"
    );
    let exchanges = fake.exchanges();
    assert_eq!(exchanges.len(), 1);
    assert_eq!(exchanges[0]["client_id"], "oaiapp_new");
    assert!(fake.revokes().is_empty());
}

#[test]
fn sign_out_revokes_after_clearing_tokens_and_reports_the_result() {
    let cases: [(Vec<Result<u16, TransportError>>, bool); 3] = [
        (vec![Ok(200)], true),
        (vec![Err(TransportError::Network), Ok(200)], true),
        (vec![Ok(500), Ok(500)], false),
    ];
    for (script, revoked) in cases {
        let dir = journal_dir();
        let journal = dir.path();
        write_test_credential(journal, true).expect("signed-in file");
        let fake = FakeOpenAi::new(journal);
        let attempts = script.len();
        for answer in script {
            fake.script_revoke(answer);
        }

        let result = sign_out(journal, fake.as_ref(), false).expect("sign out");
        assert_eq!(result.revoked, revoked);
        let seen = fake.revokes();
        assert_eq!(seen.len(), attempts);
        for revoke in &seen {
            assert_eq!(revoke.token, "test-refresh-token");
            assert_eq!(revoke.client_id, TEST_CLIENT);
            assert!(!revoke.file_held_tokens);
            assert!(revoke.lock_was_free);
        }
        let doc = read_doc(journal);
        assert!(doc.tokens.is_none());
        assert_eq!(doc.sign_in_id, None);
        assert_eq!(doc.host_id, TEST_HOST);
        assert_eq!(
            registration(journal)
                .expect("registration")
                .map(|r| r.client_id),
            Some(TEST_CLIENT.to_string())
        );
        assert!(!get_status(journal).expect("status").signed_in);
    }
}

#[test]
fn sign_out_with_forget_clears_the_registration_and_keeps_the_host() {
    let dir = journal_dir();
    let journal = dir.path();
    write_test_credential(journal, true).expect("signed-in file");
    let fake = FakeOpenAi::new(journal);

    let result = sign_out(journal, fake.as_ref(), true).expect("sign out");
    assert!(result.revoked);
    let seen = fake.revokes();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].token, "test-refresh-token");
    assert!(!seen[0].file_held_tokens);
    assert!(seen[0].lock_was_free);
    let doc = read_doc(journal);
    assert_eq!(doc.host_id, TEST_HOST);
    assert!(doc.registration.is_none());
    assert!(doc.tokens.is_none());
    assert_eq!(doc.sign_in_id, None);

    let attempt = begin_attempt_elsewhere(journal);
    assert_eq!(attempt.client_id, DYNAMIC_CLIENT_ID);
    assert!(attempt.authorize_url.contains("agent_name_hint=solstone"));
    assert_eq!(
        finish_pasted(
            journal,
            &fake,
            &attempt,
            "ac_1",
            Some("oaiapp_other"),
            "user-sub-other"
        ),
        Ok(ClosedOutcome::SignedIn)
    );
    assert_eq!(read_doc(journal).subject.as_deref(), Some("user-sub-other"));
}

#[test]
fn sign_out_while_signed_out_sends_no_revoke() {
    let absent = journal_dir();
    let fake = FakeOpenAi::new(absent.path());
    assert!(
        sign_out(absent.path(), fake.as_ref(), false)
            .expect("sign out")
            .revoked
    );
    assert!(fake.revokes().is_empty());

    for forget in [false, true] {
        let dir = journal_dir();
        let journal = dir.path();
        write_test_credential(journal, false).expect("signed-out file");
        let fake = FakeOpenAi::new(journal);
        let result = sign_out(journal, fake.as_ref(), forget).expect("sign out");
        assert!(result.revoked);
        assert!(fake.revokes().is_empty());
        assert_eq!(read_doc(journal).host_id, TEST_HOST);
    }
}

#[test]
fn credential_surfaces_carry_no_token_code_or_verifier() {
    let dir = journal_dir();
    let journal = dir.path();
    write_test_credential(journal, true).expect("signed-in file");
    let fake = FakeOpenAi::new(journal);
    let auth = manager(journal, &fake);
    let pending = begin_attempt_elsewhere(journal);
    let code = "ac_secret_code";
    fake.accept_code(code, &pending.nonce, TEST_SUBJECT);
    let secrets = [
        "test-access-token".to_string(),
        "test-refresh-token".to_string(),
        "tok-new-secret".to_string(),
        "rt-new-secret".to_string(),
        "rt-unsaved-secret".to_string(),
        code.to_string(),
        pending.verifier.clone(),
        pending.state.clone(),
    ];
    let body_with_secrets = format!(
        "test-access-token test-refresh-token {code} {} {}",
        pending.verifier, pending.state
    );
    let mut surfaces = Vec::new();
    let mut errors = Vec::new();

    // A 200 refresh whose `expires_in` is a string still saves the rotated refresh token.
    fake.script_refresh(refresh_answer(
        200,
        json!({
            "access_token": "tok-new-secret",
            "refresh_token": "rt-new-secret",
            "expires_in": "3600",
            "scope": format!("openid {DIRECT_USE_SCOPE}"),
        }),
    ));
    let malformed = auth
        .access_token_after_rejection("test-access-token")
        .expect_err("malformed refresh");
    assert!(matches!(malformed, CredentialError::Malformed(_)));
    let tokens = read_doc(journal).tokens.expect("tokens kept");
    assert_eq!(tokens.refresh_token, "rt-new-secret");
    assert_eq!(tokens.expires_at, 0);
    errors.push(malformed);

    fake.script_refresh(refresh_answer(
        400,
        json!({"error": "unlisted_code", "error_description": body_with_secrets}),
    ));
    let refused = auth.access_token().expect_err("refused refresh");
    assert!(matches!(refused, CredentialError::Refused(_)));
    errors.push(refused);

    fake.script_refresh(Ok(response(503, &body_with_secrets)));
    let unavailable = auth.access_token().expect_err("unavailable refresh");
    assert_eq!(unavailable, CredentialError::Unavailable);
    errors.push(unavailable);

    fake.script_refresh(Err(TransportError::Network));
    let network = auth.access_token().expect_err("network refresh");
    assert_eq!(network, CredentialError::Network);
    errors.push(network);

    fake.script_refresh(refresh_answer(
        200,
        json!({
            "access_token": "tok-new-secret",
            "refresh_token": "rt-unsaved-secret",
            "expires_in": 3600,
        }),
    ));
    set_fail_next_grant_write(true);
    let not_saved = auth.access_token().expect_err("grant not saved");
    set_fail_next_grant_write(false);
    assert_eq!(not_saved, CredentialError::GrantNotSaved);
    errors.push(not_saved);

    let busy = {
        let _held = acquire_credential_lock(journal, credential_lock_options()).expect("lock");
        with_test_access_token_lock_timeout(Duration::ZERO, || auth.access_token())
            .expect_err("busy")
    };
    assert_eq!(busy, CredentialError::Busy);
    errors.push(busy);

    let status = get_status(journal).expect("status");
    surfaces.push(format!("{status:?}"));
    surfaces.push(serde_json::to_string(&status).expect("status serializes"));
    let doc = read_doc(journal);
    surfaces.push(format!("{doc:?}"));
    surfaces.push(format!("{:?}", doc.tokens));
    surfaces.push(format!("{pending:?}"));

    let wrong_state = pasted_callback(&pending, code, None).replace(&pending.state, "other");
    let outcome = finish_sign_in(
        journal,
        fake.as_ref(),
        &pending,
        Some(&wrong_state),
        Duration::from_secs(5),
    );
    assert_eq!(outcome, Err(ClosedOutcome::CallbackInvalid));
    surfaces.push(format!("{outcome:?}"));

    fake.script_refresh(refresh_answer(
        400,
        json!({"error": "invalid_grant", "error_description": body_with_secrets}),
    ));
    let sign_in_required = auth.access_token().expect_err("sign-in required");
    assert_eq!(sign_in_required, CredentialError::SignInRequired);
    errors.push(sign_in_required);

    std::fs::write(
        credential_path(journal),
        format!(r#"{{"tokens":"{body_with_secrets}"}}"#),
    )
    .expect("bad file writes");
    let storage = get_status(journal).expect_err("bad file");
    assert!(matches!(storage, CredentialError::Storage(_)));
    errors.push(storage);

    for error in &errors {
        surfaces.push(format!("{error}"));
        surfaces.push(format!("{error:?}"));
    }
    for surface in &surfaces {
        for secret in &secrets {
            assert!(
                !surface.contains(secret.as_str()),
                "a credential surface carries a secret value"
            );
        }
    }
}

fn api_path(url: &str) -> &str {
    let rest = url
        .strip_prefix("http://")
        .or_else(|| url.strip_prefix("https://"))
        .unwrap_or(url);
    rest.find('/').map_or("", |index| &rest[index..])
}

#[test]
fn sign_in_uses_the_published_endpoint_paths() {
    assert_eq!(crate::authorize::AUTHORIZE_PATH, "/api/accounts/authorize");
    assert_eq!(crate::exchange::TOKEN_PATH, "/api/accounts/oauth/token");
    assert_eq!(crate::revoke::REVOKE_PATH, "/api/accounts/oauth/revoke");

    let params = build_authorize_params(
        "http://127.0.0.1:9",
        TEST_REDIRECT_URI,
        TEST_HOST,
        None,
        None,
        false,
    )
    .expect("authorize params");
    assert!(
        params
            .authorize_url
            .starts_with("http://127.0.0.1:9/api/accounts/authorize?")
    );

    let dir = journal_dir();
    let journal = dir.path();
    let fake = FakeOpenAi::new(journal);
    let attempt = begin_attempt_elsewhere(journal);
    assert_eq!(
        finish_pasted(
            journal,
            &fake,
            &attempt,
            "ac_1",
            Some(TEST_CLIENT),
            TEST_SUBJECT
        ),
        Ok(ClosedOutcome::SignedIn)
    );
    manager(journal, &fake)
        .access_token_after_rejection(&format!("tok-{TEST_CLIENT}-ac_1"))
        .expect("refreshed");
    sign_out(journal, fake.as_ref(), false).expect("sign out");
    let paths: Vec<String> = fake
        .urls()
        .iter()
        .map(|url| api_path(url).to_string())
        .collect();
    assert_eq!(
        paths,
        [
            "/api/accounts/oauth/token",
            "/api/accounts/oauth/token",
            "/api/accounts/oauth/revoke",
        ]
    );
}

#[test]
fn exchange_and_refresh_forms_carry_the_api_resource() {
    let dir = journal_dir();
    let journal = dir.path();
    let fake = FakeOpenAi::new(journal);
    let attempt = begin_attempt_elsewhere(journal);
    assert_eq!(
        finish_pasted(
            journal,
            &fake,
            &attempt,
            "ac_1",
            Some(TEST_CLIENT),
            TEST_SUBJECT
        ),
        Ok(ClosedOutcome::SignedIn)
    );
    manager(journal, &fake)
        .access_token_after_rejection(&format!("tok-{TEST_CLIENT}-ac_1"))
        .expect("refreshed");

    let exchanges = fake.exchanges();
    assert_eq!(exchanges.len(), 1);
    assert_eq!(exchanges[0]["resource"], "https://api.openai.com/v1");
    let refreshes = fake.refreshes();
    assert_eq!(refreshes.len(), 1);
    assert_eq!(refreshes[0]["resource"], "https://api.openai.com/v1");
    assert!(!refreshes[0].contains_key("scope"));
}

fn assert_v4_uuid_urn(host_id: &str) {
    let uuid = host_id
        .strip_prefix("urn:uuid:")
        .unwrap_or_else(|| panic!("{host_id} is not a uuid urn"));
    let groups: Vec<&str> = uuid.split('-').collect();
    assert_eq!(
        groups.iter().map(|group| group.len()).collect::<Vec<_>>(),
        [8, 4, 4, 4, 12],
        "{host_id}"
    );
    assert!(
        groups
            .iter()
            .all(|group| group.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f'))),
        "{host_id}"
    );
    assert!(groups[2].starts_with('4'), "{host_id} is not version 4");
    assert!(
        groups[3].starts_with(['8', '9', 'a', 'b']),
        "{host_id} does not carry the RFC 4122 variant"
    );
}

#[test]
fn host_ids_are_random_version_4_uuid_urns() {
    let first = generate_host_id().expect("host id");
    let second = generate_host_id().expect("host id");
    assert_v4_uuid_urn(&first);
    assert_v4_uuid_urn(&second);
    assert_ne!(first, second);

    // The recovery for a bad file starts a new file with a new host id.
    let dir = journal_dir();
    let journal = dir.path();
    std::fs::write(credential_path(journal), b"not json").expect("bad file writes");
    let fake = FakeOpenAi::new(journal);
    sign_out(journal, fake.as_ref(), true).expect("forget");
    assert_v4_uuid_urn(&read_doc(journal).host_id);
}

fn registered_without_tokens(journal: &Path) {
    write_test_credential(journal, false).expect("registered file");
}

#[test]
fn declined_plan_is_saved_while_the_registration_holds() {
    let dir = journal_dir();
    let journal = dir.path();
    registered_without_tokens(journal);
    let fake = FakeOpenAi::new(journal);
    fake.set_exchange_scope("openid profile email");
    let attempt = begin_attempt_elsewhere(journal);

    assert_eq!(
        finish_pasted(journal, &fake, &attempt, "ac_1", None, TEST_SUBJECT),
        Err(ClosedOutcome::PlanUsageNotGranted)
    );
    let doc = read_doc(journal);
    assert!(doc.plan_usage_declined);
    assert!(doc.tokens.is_none());
    assert_eq!(
        doc.registration.expect("registration kept").client_id,
        TEST_CLIENT
    );
    let revokes = fake.revokes();
    assert_eq!(revokes.len(), 1);
    assert_eq!(revokes[0].token, format!("rt-{TEST_CLIENT}-ac_1"));
    assert!(revokes[0].lock_was_free);
}

#[test]
fn declined_plan_after_a_forget_is_superseded_without_a_write() {
    let dir = journal_dir();
    let journal = dir.path().to_path_buf();
    registered_without_tokens(&journal);
    let fake = FakeOpenAi::new(&journal);
    fake.set_exchange_scope("openid profile email");
    let attempt = begin_attempt_elsewhere(&journal);
    let after_forget = Arc::new(Mutex::new(None));
    {
        let journal = journal.clone();
        let after_forget = after_forget.clone();
        fake.during_next_exchange(move || {
            let other = FakeOpenAi::new(&journal);
            sign_out(&journal, other.as_ref(), true).expect("forget");
            *after_forget.lock().unwrap() = Some(file_bytes(&journal));
        });
    }

    assert_eq!(
        finish_pasted(&journal, &fake, &attempt, "ac_1", None, TEST_SUBJECT),
        Err(ClosedOutcome::Superseded)
    );
    assert_eq!(
        Some(file_bytes(&journal)),
        *after_forget.lock().unwrap(),
        "nothing is written past the forget"
    );
    let doc = read_doc(&journal);
    assert!(doc.registration.is_none());
    assert!(!doc.plan_usage_declined);
    assert!(doc.subject.is_none());
    let revokes = fake.revokes();
    assert_eq!(revokes.len(), 1);
    assert_eq!(revokes[0].token, format!("rt-{TEST_CLIENT}-ac_1"));
    assert!(revokes[0].lock_was_free);
}

#[test]
fn declined_plan_after_another_registration_is_superseded_without_a_write() {
    let dir = journal_dir();
    let journal = dir.path().to_path_buf();
    registered_without_tokens(&journal);
    let fake = FakeOpenAi::new(&journal);
    fake.set_exchange_scope("openid profile email");
    let attempt = begin_attempt_elsewhere(&journal);
    let after_other = Arc::new(Mutex::new(None));
    {
        let journal = journal.clone();
        let after_other = after_other.clone();
        fake.during_next_exchange(move || {
            let mut doc = read_doc(&journal);
            doc.registration = Some(Registration {
                client_id: "oaiapp_other".to_string(),
                client_refused: false,
            });
            save_credential_file(&journal, &doc).expect("other registration saves");
            *after_other.lock().unwrap() = Some(file_bytes(&journal));
        });
    }

    assert_eq!(
        finish_pasted(&journal, &fake, &attempt, "ac_1", None, TEST_SUBJECT),
        Err(ClosedOutcome::Superseded)
    );
    assert_eq!(Some(file_bytes(&journal)), *after_other.lock().unwrap());
    assert!(!read_doc(&journal).plan_usage_declined);
    assert_eq!(fake.revokes().len(), 1);
}

#[test]
fn status_reports_state_email_expiry_and_registration_flags() {
    let absent = journal_dir();
    assert_eq!(
        serde_json::to_value(get_status(absent.path()).expect("status")).expect("serializes"),
        json!({
            "state": "signed_out",
            "signed_in": false,
            "plan_usage_declined": false,
            "client_refused": false,
        })
    );

    let dir = journal_dir();
    let journal = dir.path();
    write_signed_in(journal, "tok-a", "rt-a", 1_900_000_000);
    let signed_in = serde_json::to_value(get_status(journal).expect("status")).expect("json");
    assert_eq!(
        signed_in,
        json!({
            "state": "signed_in",
            "signed_in": true,
            "email": "user@example.com",
            "expires_at": 1_900_000_000u64,
            "plan_usage_declined": false,
            "client_refused": false,
        })
    );
    assert!(!signed_in.to_string().contains("tok-a"));
    assert!(!signed_in.to_string().contains("rt-a"));

    let mut doc = read_doc(journal);
    doc.tokens = None;
    doc.plan_usage_declined = true;
    doc.registration = Some(Registration {
        client_id: TEST_CLIENT.to_string(),
        client_refused: true,
    });
    save_credential_file(journal, &doc).expect("signed-out file saves");
    assert_eq!(
        serde_json::to_value(get_status(journal).expect("status")).expect("json"),
        json!({
            "state": "signed_out",
            "signed_in": false,
            "email": "user@example.com",
            "plan_usage_declined": true,
            "client_refused": true,
        })
    );
}

fn pending() -> AttemptStatus {
    AttemptStatus {
        state: AttemptState::Pending,
        reason: None,
    }
}

fn ended(state: AttemptState, reason: ClosedOutcome) -> AttemptStatus {
    AttemptStatus {
        state,
        reason: Some(reason),
    }
}

#[test]
fn attempt_status_follows_an_attempt_to_its_outcome() {
    let _registry = hold_attempt_registry();
    let dir = journal_dir();
    let journal = dir.path();
    let fake = FakeOpenAi::new(journal);

    assert_eq!(
        attempt_status("unknown-attempt"),
        ended(AttemptState::Expired, ClosedOutcome::Expired)
    );
    assert_eq!(
        active_attempt_named("unknown-attempt").map(|a| a.attempt_id),
        Err(ClosedOutcome::Expired)
    );

    let first = begin_attempt_elsewhere(journal);
    set_active_attempt(first.clone());
    assert_eq!(attempt_status(&first.attempt_id), pending());
    assert_eq!(
        active_attempt_named(&first.attempt_id).map(|a| a.attempt_id),
        Ok(first.attempt_id.clone())
    );

    let second = begin_attempt_elsewhere(journal);
    set_active_attempt(second.clone());
    assert_eq!(
        attempt_status(&first.attempt_id),
        ended(AttemptState::Failed, ClosedOutcome::Cancelled)
    );
    assert_eq!(
        active_attempt_named(&first.attempt_id).map(|a| a.attempt_id),
        Err(ClosedOutcome::Cancelled)
    );
    assert_eq!(attempt_status(&second.attempt_id), pending());

    // A wrong state never reaches the attempt.
    let wrong_state =
        pasted_callback(&second, "ac_2", Some(TEST_CLIENT)).replace(&second.state, "other-state");
    assert_eq!(
        finish_sign_in(
            journal,
            fake.as_ref(),
            &second,
            Some(&wrong_state),
            Duration::ZERO
        ),
        Err(ClosedOutcome::CallbackInvalid)
    );
    assert_eq!(attempt_status(&second.attempt_id), pending());

    // A denial with the right state ends it, and it stays ended.
    let denied = format!(
        "http://127.0.0.1:1455/auth/callback?error=access_denied&state={}",
        second.state
    );
    assert_eq!(
        finish_sign_in(
            journal,
            fake.as_ref(),
            &second,
            Some(&denied),
            Duration::ZERO
        ),
        Err(ClosedOutcome::Denied)
    );
    assert_eq!(
        attempt_status(&second.attempt_id),
        ended(AttemptState::Failed, ClosedOutcome::Denied)
    );
    assert_eq!(
        finish_pasted(
            journal,
            &fake,
            &second,
            "ac_2",
            Some(TEST_CLIENT),
            TEST_SUBJECT
        ),
        Err(ClosedOutcome::CallbackInvalid)
    );
    assert_eq!(
        attempt_status(&second.attempt_id),
        ended(AttemptState::Failed, ClosedOutcome::Denied)
    );
    assert!(fake.exchanges().is_empty());

    let third = begin_attempt_elsewhere(journal);
    set_active_attempt(third.clone());
    assert_eq!(
        finish_pasted(
            journal,
            &fake,
            &third,
            "ac_3",
            Some(TEST_CLIENT),
            TEST_SUBJECT
        ),
        Ok(ClosedOutcome::SignedIn)
    );
    assert_eq!(
        attempt_status(&third.attempt_id),
        ended(AttemptState::SignedIn, ClosedOutcome::SignedIn)
    );
    assert_eq!(
        finish_pasted(journal, &fake, &third, "ac_4", None, TEST_SUBJECT),
        Err(ClosedOutcome::CallbackInvalid)
    );
    assert_eq!(fake.exchanges().len(), 1);

    // Replacing a finished attempt keeps its outcome.
    let mut stale = begin_attempt_elsewhere(journal);
    stale.created_at_unix = now_secs() - ATTEMPT_EXPIRATION_SECS;
    set_active_attempt(stale.clone());
    assert_eq!(
        attempt_status(&third.attempt_id),
        ended(AttemptState::SignedIn, ClosedOutcome::SignedIn)
    );
    assert_eq!(
        attempt_status(&stale.attempt_id),
        ended(AttemptState::Expired, ClosedOutcome::Expired)
    );
    assert_eq!(
        active_attempt_named(&stale.attempt_id).map(|a| a.attempt_id),
        Err(ClosedOutcome::Expired)
    );
    assert_eq!(
        finish_active_sign_in(journal, fake.as_ref(), None, Duration::ZERO),
        Err(ClosedOutcome::Expired)
    );
}

#[test]
fn a_refused_client_is_marked_while_the_registration_holds() {
    let dir = journal_dir();
    let journal = dir.path();
    registered_without_tokens(journal);
    let fake = FakeOpenAi::new(journal);
    fake.refuse_exchanges("invalid_client");
    let attempt = begin_attempt_elsewhere(journal);

    assert_eq!(
        finish_pasted(journal, &fake, &attempt, "ac_1", None, TEST_SUBJECT),
        Err(ClosedOutcome::RegistrationRefused)
    );
    let registration = read_doc(journal).registration.expect("registration kept");
    assert_eq!(registration.client_id, TEST_CLIENT);
    assert!(registration.client_refused);
    assert!(get_status(journal).expect("status").client_refused);
}

#[test]
fn a_refused_client_after_a_lost_race_is_superseded_without_a_write() {
    type Interference = fn(&Path);
    fn forget(journal: &Path) {
        let other = FakeOpenAi::new(journal);
        sign_out(journal, other.as_ref(), true).expect("forget");
    }
    fn register_another_client(journal: &Path) {
        let mut doc = read_doc(journal);
        doc.registration = Some(Registration {
            client_id: "oaiapp_other".to_string(),
            client_refused: false,
        });
        save_credential_file(journal, &doc).expect("other registration saves");
    }
    let interferences: [Interference; 2] = [forget, register_another_client];
    for interfere in interferences {
        let dir = journal_dir();
        let journal = dir.path().to_path_buf();
        registered_without_tokens(&journal);
        let fake = FakeOpenAi::new(&journal);
        fake.refuse_exchanges("invalid_client");
        let attempt = begin_attempt_elsewhere(&journal);
        let after_race = Arc::new(Mutex::new(None));
        {
            let journal = journal.clone();
            let after_race = after_race.clone();
            fake.during_next_exchange(move || {
                interfere(&journal);
                *after_race.lock().unwrap() = Some(file_bytes(&journal));
            });
        }

        assert_eq!(
            finish_pasted(&journal, &fake, &attempt, "ac_1", None, TEST_SUBJECT),
            Err(ClosedOutcome::Superseded)
        );
        assert_eq!(
            Some(file_bytes(&journal)),
            *after_race.lock().unwrap(),
            "nothing is written past the lost race"
        );
        assert!(
            read_doc(&journal)
                .registration
                .is_none_or(|registration| !registration.client_refused)
        );
    }
}
