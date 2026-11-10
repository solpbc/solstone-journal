// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Oura's HTTP transport, the journal's Oura settings, and token refresh, with the
//! renewal held in `config/oura-renewal.json` until config has it.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::Path;
use std::thread;
use std::time::Duration;

use chrono::Utc;
use serde_json::{Map, Value, json};
use solstone_core_journal_config::read_journal_config;
use solstone_core_journal_config_write::{JournalConfigMutation, mutate_journal_config};
use solstone_core_journal_io::{FileLock, LockOptions, create_directory_with_mode, hold_lock};

use crate::{BodyIngestError, BodyIngestErrorKind};

pub const API_BASE: &str = "https://api.ouraring.com/v2/usercollection";
pub const TOKEN_URL: &str = "https://api.ouraring.com/oauth/token";

const MAX_RESPONSE_BYTES: u64 = 16 * 1024 * 1024;

pub struct HttpResponse {
    pub status: u16,
    pub body: String,
}

pub trait OuraHttp {
    fn get(
        &mut self,
        endpoint: &str,
        query: &BTreeMap<String, String>,
        authorization: &str,
    ) -> Result<HttpResponse, BodyIngestError>;

    fn post_form(
        &mut self,
        url: &str,
        form: &BTreeMap<String, String>,
    ) -> Result<HttpResponse, BodyIngestError>;

    fn backoff(&mut self, attempt: u32) {
        thread::sleep(Duration::from_secs(1_u64 << attempt.min(4)));
    }
}

pub struct LiveOuraHttp {
    agent: ureq::Agent,
}

impl LiveOuraHttp {
    fn new() -> Self {
        let timeout = Duration::from_secs(30);
        let config = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_connect(Some(timeout))
            .timeout_recv_response(Some(timeout))
            .timeout_recv_body(Some(timeout))
            .timeout_global(Some(Duration::from_secs(90)))
            .build();
        Self {
            agent: ureq::Agent::new_with_config(config),
        }
    }

    pub fn response(
        response: ureq::http::Response<ureq::Body>,
    ) -> Result<HttpResponse, BodyIngestError> {
        let status = response.status().as_u16();
        let mut reader = response
            .into_body()
            .into_reader()
            .take(MAX_RESPONSE_BYTES + 1);
        let mut bytes = Vec::new();
        reader
            .read_to_end(&mut bytes)
            .map_err(|_| failure(BodyIngestErrorKind::Source, "http_read"))?;
        if bytes.len() as u64 > MAX_RESPONSE_BYTES {
            return Err(failure(
                BodyIngestErrorKind::Source,
                "http_response_too_large",
            ));
        }
        let body = String::from_utf8(bytes)
            .map_err(|_| failure(BodyIngestErrorKind::Source, "http_utf8"))?;
        Ok(HttpResponse { status, body })
    }
}

impl OuraHttp for LiveOuraHttp {
    fn get(
        &mut self,
        endpoint: &str,
        query: &BTreeMap<String, String>,
        authorization: &str,
    ) -> Result<HttpResponse, BodyIngestError> {
        let url = format!("{API_BASE}/{endpoint}?{}", encode_form(query));
        let response = self
            .agent
            .get(&url)
            .header("Authorization", authorization)
            .header("Accept", "application/json")
            .call()
            .map_err(|_| failure(BodyIngestErrorKind::Source, "http_get"))?;
        Self::response(response)
    }

    fn post_form(
        &mut self,
        url: &str,
        form: &BTreeMap<String, String>,
    ) -> Result<HttpResponse, BodyIngestError> {
        let response = self
            .agent
            .post(url)
            .header("Accept", "application/json")
            .header("Content-Type", "application/x-www-form-urlencoded")
            .send(encode_form(form))
            .map_err(|_| failure(BodyIngestErrorKind::Source, "http_post"))?;
        Self::response(response)
    }
}

pub(crate) fn post_oura_token_form(
    form: &BTreeMap<String, String>,
) -> Result<Value, BodyIngestError> {
    let mut http = LiveOuraHttp::new();
    let response = http.post_form(TOKEN_URL, form)?;
    if response.status != 200 {
        return Err(failure(
            BodyIngestErrorKind::Source,
            "authorization_exchange",
        ));
    }
    serde_json::from_str(&response.body)
        .map_err(|_| failure(BodyIngestErrorKind::Source, "authorization_response"))
}

#[derive(Clone)]
pub struct OuraSettings {
    pub client_id: String,
    pub client_secret: Option<String>,
    pub access_token: String,
    pub refresh_token: String,
    pub token_type: String,
    pub timezone: String,
}

pub fn hold_oura_lock(journal: &Path) -> Result<FileLock, BodyIngestError> {
    create_directory_with_mode(&journal.join("imports"), 0o700)
        .map_err(|_| failure(BodyIngestErrorKind::Publication, "sync_lock"))?;
    hold_lock(
        journal.join("imports/oura.json"),
        LockOptions {
            mode: Some(0o600),
            ..LockOptions::default()
        },
    )
    .map_err(|_| failure(BodyIngestErrorKind::Publication, "sync_lock"))
}

pub fn settings(journal: &Path) -> Result<OuraSettings, BodyIngestError> {
    let read = read_journal_config(journal)
        .map_err(|_| failure(BodyIngestErrorKind::Source, "journal_config"))?;
    let config = read
        .config
        .ok_or_else(|| failure(BodyIngestErrorKind::Source, "journal_config_missing"))?;
    let section = config
        .get("oura")
        .and_then(Value::as_object)
        .ok_or_else(|| failure(BodyIngestErrorKind::Source, "oura_config_missing"))?;
    let tokens = section
        .get("tokens")
        .and_then(Value::as_object)
        .ok_or_else(|| failure(BodyIngestErrorKind::Source, "authorization_needed"))?;
    let required = |object: &Map<String, Value>, key: &str, stage| {
        object
            .get(key)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
            .ok_or_else(|| failure(BodyIngestErrorKind::Source, stage))
    };
    let timezone = solstone_core_journal_config::owner_zone(journal)
        .name()
        .to_owned();
    Ok(OuraSettings {
        client_id: required(section, "client_id", "client_id_missing")?,
        client_secret: section
            .get("client_secret")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_owned),
        access_token: required(tokens, "access_token", "tokens_invalid")?,
        refresh_token: required(tokens, "refresh_token", "tokens_invalid")?,
        token_type: tokens
            .get("token_type")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .unwrap_or("Bearer")
            .to_owned(),
        timezone,
    })
}

/// OAuth token types are case-insensitive, but Oura's API accepts only the
/// canonical `Bearer` scheme spelling, and its token endpoint may return `bearer`.
fn authorization_header(settings: &OuraSettings) -> String {
    let scheme = if settings.token_type.eq_ignore_ascii_case("bearer") {
        "Bearer"
    } else {
        settings.token_type.as_str()
    };
    format!("{scheme} {}", settings.access_token)
}

pub fn request_with_retry(
    http: &mut dyn OuraHttp,
    endpoint: &str,
    query: &BTreeMap<String, String>,
    settings: &mut OuraSettings,
    refreshed: &mut bool,
    journal: &Path,
    persist_refresh: bool,
) -> Result<HttpResponse, BodyIngestError> {
    let mut retryable = 0_u32;
    loop {
        let authorization = authorization_header(settings);
        let response = http.get(endpoint, query, &authorization)?;
        if response.status == 401 && !*refreshed {
            if !persist_refresh {
                return Err(failure(
                    BodyIngestErrorKind::Source,
                    "authorization_refresh_requires_save",
                ));
            }
            refresh(http, settings, journal, persist_refresh)?;
            *refreshed = true;
            continue;
        }
        if response.status == 429 || (500..=599).contains(&response.status) {
            retryable += 1;
            if retryable >= 3 {
                return Err(failure(BodyIngestErrorKind::Source, "api_retry_exhausted"));
            }
            http.backoff(retryable - 1);
            continue;
        }
        return Ok(response);
    }
}

#[derive(Debug)]
pub enum ResponseClass {
    Page(Map<String, Value>),
    Permission,
}

pub fn classify_response(
    _endpoint: &str,
    response: &HttpResponse,
) -> Result<ResponseClass, BodyIngestError> {
    match response.status {
        200 => {
            let page: Value = serde_json::from_str(&response.body)
                .map_err(|_| failure(BodyIngestErrorKind::Source, "api_json"))?;
            page.as_object()
                .cloned()
                .map(ResponseClass::Page)
                .ok_or_else(|| failure(BodyIngestErrorKind::Source, "api_object"))
        }
        401 => Ok(ResponseClass::Permission),
        403 => {
            let lower = response.body.to_ascii_lowercase();
            if ["membership", "subscription", "not active", "expired member"]
                .iter()
                .any(|needle| lower.contains(needle))
            {
                Err(failure(BodyIngestErrorKind::Source, "membership_lost"))
            } else if ["scope", "permission", "not authorized", "access denied"]
                .iter()
                .any(|needle| lower.contains(needle))
            {
                Ok(ResponseClass::Permission)
            } else {
                Err(failure(BodyIngestErrorKind::Source, "unknown_forbidden"))
            }
        }
        _ => Err(failure(BodyIngestErrorKind::Source, "api_status")),
    }
}

pub fn refresh(
    http: &mut dyn OuraHttp,
    settings: &mut OuraSettings,
    journal: &Path,
    persist: bool,
) -> Result<(), BodyIngestError> {
    let mut form = BTreeMap::from([
        ("grant_type".to_owned(), "refresh_token".to_owned()),
        ("refresh_token".to_owned(), settings.refresh_token.clone()),
        ("client_id".to_owned(), settings.client_id.clone()),
    ]);
    if let Some(secret) = &settings.client_secret {
        form.insert("client_secret".to_owned(), secret.clone());
    }
    let response = http.post_form(TOKEN_URL, &form)?;
    if response.status != 200 {
        return Err(failure(
            BodyIngestErrorKind::Source,
            "authorization_refresh",
        ));
    }
    let payload: Value = serde_json::from_str(&response.body)
        .map_err(|_| failure(BodyIngestErrorKind::Source, "authorization_response"))?;
    let object = payload
        .as_object()
        .ok_or_else(|| failure(BodyIngestErrorKind::Source, "authorization_response"))?;
    let access_token = token_field(object, "access_token")?;
    let refresh_token = token_field(object, "refresh_token")?;
    let token_type = object
        .get("token_type")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .unwrap_or("Bearer")
        .to_owned();
    let expires_at = object
        .get("expires_at")
        .and_then(Value::as_f64)
        .or_else(|| {
            object
                .get("expires_in")
                .and_then(Value::as_f64)
                .map(|seconds| Utc::now().timestamp() as f64 + seconds)
        })
        .ok_or_else(|| failure(BodyIngestErrorKind::Source, "authorization_response"))?;
    if persist {
        let next_access = access_token.clone();
        let next_refresh = refresh_token.clone();
        let next_type = token_type.clone();
        hold_renewal(
            journal,
            &settings.refresh_token,
            &json!({
                "access_token": access_token,
                "refresh_token": refresh_token,
                "expires_at": expires_at,
                "token_type": token_type,
            }),
        );
        mutate_journal_config(
            journal,
            solstone_core_journal_config_write::LockOptions::default(),
            move |config| {
                let section = config
                    .entry("oura".to_owned())
                    .or_insert_with(|| Value::Object(Map::new()))
                    .as_object_mut()
                    .expect("checked settings preserve an object section");
                let next = json!({
                    "access_token": next_access,
                    "refresh_token": next_refresh,
                    "expires_at": expires_at,
                    "token_type": next_type,
                });
                let changed = section.get("tokens") != Some(&next);
                section.insert("tokens".to_owned(), next);
                JournalConfigMutation { changed, value: () }
            },
        )
        .map_err(|_| failure(BodyIngestErrorKind::Publication, "token_store"))?;
        clear_held_renewal(journal);
    }
    settings.access_token = access_token;
    settings.refresh_token = refresh_token;
    settings.token_type = token_type;
    Ok(())
}

/// Oura's answer to a refresh, held until it's in config. A refresh token works
/// once, so an answer lost between Oura and config would end the connection.
const RENEWAL_HOLD: &str = "config/oura-renewal.json";

/// Keep Oura's answer before config is touched. A hold that can't be written
/// doesn't stop the config write: that would be no better off than without it.
fn hold_renewal(journal: &Path, spent: &str, tokens: &Value) {
    let path = journal.join(RENEWAL_HOLD);
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let body = json!({"spent_refresh_token": spent, "tokens": tokens}).to_string();
    let _ = solstone_core_journal_io::atomic_replace(
        &path,
        body.as_bytes(),
        solstone_core_journal_io::AtomicWriteOptions { mode: Some(0o600) },
    );
}

/// Forget any held answer: it reached config, or config has moved past it.
pub fn clear_held_renewal(journal: &Path) {
    let _ = std::fs::remove_file(journal.join(RENEWAL_HOLD));
}

/// Put a held answer into config, while config still holds the token it spent.
/// Config having moved on, by a reconnect or a later renewal, drops the hold.
/// The caller holds `imports/oura.json`. A hold that can't be read says nothing.
///
/// # Errors
///
/// The answer belongs in config and config can't be written.
pub fn apply_held_renewal(journal: &Path) -> Result<(), BodyIngestError> {
    let Ok(text) = std::fs::read_to_string(journal.join(RENEWAL_HOLD)) else {
        return Ok(());
    };
    let Ok(hold) = serde_json::from_str::<Value>(&text) else {
        clear_held_renewal(journal);
        return Ok(());
    };
    let (Some(spent), Some(tokens)) = (
        hold.get("spent_refresh_token")
            .and_then(Value::as_str)
            .map(str::to_owned),
        hold.get("tokens").cloned(),
    ) else {
        clear_held_renewal(journal);
        return Ok(());
    };
    mutate_journal_config(
        journal,
        solstone_core_journal_config_write::LockOptions::default(),
        move |config| {
            let section = config
                .entry("oura".to_owned())
                .or_insert_with(|| Value::Object(Map::new()));
            let current = section
                .get("tokens")
                .and_then(|tokens| tokens.get("refresh_token"))
                .and_then(Value::as_str);
            if current != Some(spent.as_str()) {
                return JournalConfigMutation {
                    changed: false,
                    value: (),
                };
            }
            if let Some(section) = section.as_object_mut() {
                section.insert("tokens".to_owned(), tokens);
            }
            JournalConfigMutation {
                changed: true,
                value: (),
            }
        },
    )
    .map_err(|_| failure(BodyIngestErrorKind::Publication, "token_store"))?;
    clear_held_renewal(journal);
    Ok(())
}

fn token_field(object: &Map<String, Value>, key: &str) -> Result<String, BodyIngestError> {
    object
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| failure(BodyIngestErrorKind::Source, "authorization_response"))
}

pub fn encode_form(values: &BTreeMap<String, String>) -> String {
    values
        .iter()
        .map(|(key, value)| format!("{}={}", percent_encode(key), percent_encode(value)))
        .collect::<Vec<_>>()
        .join("&")
}

fn percent_encode(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(char::from(byte));
        } else if byte == b' ' {
            encoded.push('+');
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

const fn failure(kind: BodyIngestErrorKind, stage: &'static str) -> BodyIngestError {
    BodyIngestError::new(kind, stage)
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::fs;

    use tempfile::TempDir;

    use super::*;

    #[derive(Default)]
    struct FakeHttp {
        gets: VecDeque<HttpResponse>,
        posts: VecDeque<HttpResponse>,
        calls: Vec<String>,
        post_calls: usize,
    }

    impl OuraHttp for FakeHttp {
        fn get(
            &mut self,
            _endpoint: &str,
            _query: &BTreeMap<String, String>,
            authorization: &str,
        ) -> Result<HttpResponse, BodyIngestError> {
            self.calls.push(authorization.to_owned());
            self.gets
                .pop_front()
                .ok_or_else(|| failure(BodyIngestErrorKind::Source, "test_get"))
        }

        fn post_form(
            &mut self,
            _url: &str,
            _form: &BTreeMap<String, String>,
        ) -> Result<HttpResponse, BodyIngestError> {
            self.post_calls += 1;
            self.posts
                .pop_front()
                .ok_or_else(|| failure(BodyIngestErrorKind::Source, "test_post"))
        }

        fn backoff(&mut self, _attempt: u32) {}
    }

    fn response(status: u16, body: Value) -> HttpResponse {
        HttpResponse {
            status,
            body: serde_json::to_string(&body).expect("response JSON"),
        }
    }

    fn journal(token_type: &str) -> TempDir {
        let temporary = TempDir::new().expect("temporary directory");
        fs::create_dir_all(temporary.path().join("config")).expect("config directory");
        fs::write(
            temporary.path().join("config/journal.json"),
            serde_json::to_vec(&json!({
                "identity": {"timezone": "America/Denver"},
                "oura": {
                    "client_id": "synthetic-client",
                    "tokens": {
                        "access_token": "synthetic-access",
                        "refresh_token": "synthetic-refresh",
                        "token_type": token_type,
                        "expires_at": 4102444800.0
                    }
                }
            }))
            .expect("journal config"),
        )
        .expect("write journal config");
        temporary
    }

    fn refreshing(token_type: &str) -> FakeHttp {
        FakeHttp {
            gets: VecDeque::from([
                response(401, json!({"error": "expired"})),
                response(200, json!({"data": [], "next_token": null})),
            ]),
            posts: VecDeque::from([response(
                200,
                json!({
                    "access_token": "refreshed-access",
                    "refresh_token": "refreshed-refresh",
                    "token_type": token_type,
                    "expires_in": 3600
                }),
            )]),
            ..FakeHttp::default()
        }
    }

    fn get(
        http: &mut FakeHttp,
        journal: &TempDir,
        persist: bool,
    ) -> Result<HttpResponse, BodyIngestError> {
        let mut current = settings(journal.path()).expect("settings");
        let mut refreshed = false;
        request_with_retry(
            http,
            "daily_readiness",
            &BTreeMap::new(),
            &mut current,
            &mut refreshed,
            journal.path(),
            persist,
        )
    }

    fn config_tokens(journal: &TempDir) -> Value {
        let text = fs::read_to_string(journal.path().join("config/journal.json")).expect("config");
        serde_json::from_str::<Value>(&text).expect("config JSON")["oura"]["tokens"].clone()
    }

    #[test]
    fn a_403_is_read_for_what_it_says() {
        assert_eq!(
            classify_response(
                "daily_sleep",
                &response(403, json!({"error": "opaque forbidden"}))
            )
            .unwrap_err()
            .stage(),
            "unknown_forbidden"
        );
        assert_eq!(
            classify_response(
                "daily_sleep",
                &response(403, json!({"error": "membership expired"}))
            )
            .unwrap_err()
            .stage(),
            "membership_lost"
        );
        assert!(matches!(
            classify_response(
                "daily_sleep",
                &response(403, json!({"error": "missing permission scope"}))
            ),
            Ok(ResponseClass::Permission)
        ));
    }

    #[test]
    fn a_401_without_persistence_leaves_the_rotating_grant_unspent() {
        let journal = journal("Bearer");
        let mut http = refreshing("Synthetic");
        let original = fs::read(journal.path().join("config/journal.json")).expect("config");
        let error = get(&mut http, &journal, false)
            .err()
            .expect("an unpersisted refresh must not spend the grant");
        assert_eq!(error.kind(), BodyIngestErrorKind::Source);
        assert_eq!(error.stage(), "authorization_refresh_requires_save");
        assert_eq!(http.post_calls, 0);
        assert_eq!(http.calls[0], "Bearer synthetic-access");
        assert_eq!(
            fs::read(journal.path().join("config/journal.json")).expect("config after"),
            original
        );
    }

    #[test]
    fn lowercase_bearer_token_type_sends_the_canonical_scheme() {
        let journal = journal("bearer");
        let mut http = refreshing("bearer");
        get(&mut http, &journal, true).expect("get after refresh");
        assert_eq!(http.calls[0], "Bearer synthetic-access");
        assert_eq!(http.calls[1], "Bearer refreshed-access");
    }

    #[test]
    fn a_rotated_refresh_token_is_persisted_before_the_request_retries() {
        let journal = journal("Bearer");
        let mut http = refreshing("Synthetic");
        get(&mut http, &journal, true).expect("get after refresh");
        assert_eq!(http.post_calls, 1);
        assert_eq!(http.calls[0], "Bearer synthetic-access");
        assert_eq!(http.calls[1], "Synthetic refreshed-access");
        assert_eq!(
            config_tokens(&journal)["refresh_token"],
            "refreshed-refresh"
        );
        assert!(!journal.path().join("config/oura-renewal.json").exists());
    }
}
