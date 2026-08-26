// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::{
    collections::HashMap,
    fs,
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

use axum::{
    extract::{FromRequestParts, Json, Path as AxumPath, Query, State},
    http::{HeaderMap, StatusCode, request::Parts},
    response::Response,
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{Local, Utc};
use serde_json::{Map, Value, json};
use solstone_core_convey_http::identity::AccessBasis;
use solstone_core_journal_io::{
    AtomicWriteOptions, LockOptions, append_jsonl, atomic_replace, contained_path, hold_lock,
};

use crate::{
    AppState,
    http::{error, html_auth_failure, json as json_response},
};

const STATE_AREAS: [&str; 5] = ["segments", "entities", "facets", "imports", "config"];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum IngestIdentityCase {
    MissingAuth,
    InvalidPlIdentity,
    PlRevoked,
    PlDisabled,
    InvalidApiKey,
    DlRevoked,
    PrefixMismatch,
}

impl IngestIdentityCase {
    fn response(self) -> (StatusCode, &'static str) {
        match self {
            Self::MissingAuth => (
                StatusCode::UNAUTHORIZED,
                "Missing or invalid authentication",
            ),
            Self::InvalidPlIdentity => (StatusCode::UNAUTHORIZED, "Invalid PL identity"),
            Self::PlRevoked => (StatusCode::FORBIDDEN, "Journal source has been revoked"),
            Self::PlDisabled => (StatusCode::FORBIDDEN, "Journal source is disabled"),
            Self::InvalidApiKey => (StatusCode::UNAUTHORIZED, "Invalid API key"),
            Self::DlRevoked => (StatusCode::FORBIDDEN, "API key has been revoked"),
            Self::PrefixMismatch => (StatusCode::FORBIDDEN, "Key prefix mismatch"),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DoorIdentity<'a> {
    Bearer(&'a str),
    PrivateLink(&'a str),
}

pub(crate) struct JournalSourceIdentity {
    source: Map<String, Value>,
    derived_prefix: String,
}

impl JournalSourceIdentity {
    pub(crate) fn prefix(&self) -> &str {
        &self.derived_prefix
    }

    pub(crate) fn provenance(&self) -> Value {
        json!({
            "imported_via": "peer_link",
            "link_id": self.source.get("fingerprint").cloned().unwrap_or(Value::Null),
            "sender_fingerprint": self.source.get("fingerprint").cloned().unwrap_or(Value::Null),
            "sender_instance_id": self.source.get("peer_instance_id").cloned().unwrap_or(Value::Null),
        })
    }
}

pub(crate) fn provenance_for_prefix(root: &Path, key_prefix: &str) -> Option<Value> {
    let source = records(root)
        .into_iter()
        .find(|record| state_prefix(record).as_deref() == Some(key_prefix))?;
    Some(json!({
        "imported_via": "peer_link",
        "link_id": source.get("fingerprint").cloned().unwrap_or(Value::Null),
        "sender_fingerprint": source.get("fingerprint").cloned().unwrap_or(Value::Null),
        "sender_instance_id": source.get("peer_instance_id").cloned().unwrap_or(Value::Null),
    }))
}

impl FromRequestParts<AppState> for JournalSourceIdentity {
    type Rejection = Response;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let supplied_prefix = parts
            .uri
            .path()
            .trim_matches('/')
            .split('/')
            .nth(3)
            .filter(|prefix| !prefix.is_empty())
            .ok_or_else(|| {
                html_auth_failure(
                    StatusCode::UNAUTHORIZED,
                    "Missing or invalid authentication",
                )
            })?;
        let (source, derived_prefix) = authorize_transport(&state.root, supplied_prefix, parts)
            .map_err(|response| *response)?;
        Ok(Self {
            source,
            derived_prefix,
        })
    }
}

fn records(root: &Path) -> Vec<Map<String, Value>> {
    let directory = root.join("apps/import/journal_sources");
    fs::read_dir(directory)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let text = fs::read_to_string(entry.path()).ok()?;
            serde_json::from_str::<Value>(&text)
                .ok()?
                .as_object()
                .cloned()
        })
        .collect()
}

fn source(root: &Path, name: &str) -> Option<Map<String, Value>> {
    records(root)
        .into_iter()
        .find(|record| record.get("name").and_then(Value::as_str) == Some(name))
}

fn source_by_key(root: &Path, key: &str) -> Option<Map<String, Value>> {
    records(root)
        .into_iter()
        .find(|record| record.get("key").and_then(Value::as_str) == Some(key))
}

fn source_by_fingerprint(root: &Path, fingerprint: &str) -> Option<Map<String, Value>> {
    records(root)
        .into_iter()
        .find(|record| record.get("fingerprint").and_then(Value::as_str) == Some(fingerprint))
}

fn prefix(record: &Map<String, Value>) -> Option<String> {
    record
        .get("key")
        .and_then(Value::as_str)
        .filter(|key| key.len() >= 8)
        .map(|key| key[..8].to_owned())
}

fn state_prefix(record: &Map<String, Value>) -> Option<String> {
    if record.get("pair_mode").and_then(Value::as_str) == Some("pl") {
        return record
            .get("fingerprint")
            .and_then(Value::as_str)
            .and_then(|fingerprint| fingerprint.strip_prefix("sha256:"))
            .filter(|fingerprint| {
                fingerprint.len() == 64 && fingerprint.bytes().all(|byte| byte.is_ascii_hexdigit())
            })
            .map(|fingerprint| fingerprint[..16].to_owned());
    }
    prefix(record)
}

fn authorize(
    root: &Path,
    supplied_prefix: &str,
    identity: DoorIdentity<'_>,
) -> Result<(Map<String, Value>, String), IngestIdentityCase> {
    let is_private_link = matches!(identity, DoorIdentity::PrivateLink(_));
    let source = match identity {
        DoorIdentity::Bearer(key) => {
            let source = source_by_key(root, key).ok_or(IngestIdentityCase::InvalidApiKey)?;
            if source.get("revoked") == Some(&Value::Bool(true)) {
                return Err(IngestIdentityCase::DlRevoked);
            }
            source
        }
        DoorIdentity::PrivateLink(fingerprint) => {
            let source = source_by_fingerprint(root, fingerprint)
                .ok_or(IngestIdentityCase::InvalidPlIdentity)?;
            if source.get("revoked") == Some(&Value::Bool(true)) {
                return Err(IngestIdentityCase::PlRevoked);
            }
            if source.get("enabled") == Some(&Value::Bool(false)) {
                return Err(IngestIdentityCase::PlDisabled);
            }
            source
        }
    };
    let derived_prefix = state_prefix(&source).ok_or(if is_private_link {
        IngestIdentityCase::InvalidPlIdentity
    } else {
        IngestIdentityCase::InvalidApiKey
    })?;
    if derived_prefix != supplied_prefix {
        return Err(IngestIdentityCase::PrefixMismatch);
    }
    Ok((source, derived_prefix))
}

fn bearer_identity(headers: &HeaderMap) -> Result<DoorIdentity<'_>, IngestIdentityCase> {
    let Some(value) = headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
    else {
        return Err(IngestIdentityCase::MissingAuth);
    };
    let Some(key) = value
        .strip_prefix("Bearer ")
        .map(str::trim)
        .filter(|key| !key.is_empty())
    else {
        return Err(IngestIdentityCase::MissingAuth);
    };
    Ok(DoorIdentity::Bearer(key))
}

fn authorize_transport(
    root: &Path,
    supplied_prefix: &str,
    parts: &Parts,
) -> Result<(Map<String, Value>, String), Box<Response>> {
    let identity = match parts.extensions.get::<AccessBasis>() {
        Some(AccessBasis::LinkedDevice { cid, .. }) => Ok(DoorIdentity::PrivateLink(cid.as_str())),
        // Localhost and pairing-window requests have no accepted device identity and must
        // authenticate with the journal-source key.
        Some(AccessBasis::Localhost | AccessBasis::PairingPeer { .. }) | None => {
            bearer_identity(&parts.headers)
        }
    }
    .map_err(|case| {
        let (status, description) = case.response();
        Box::new(html_auth_failure(status, description))
    })?;
    authorize(root, supplied_prefix, identity).map_err(|case| {
        let (status, description) = case.response();
        Box::new(html_auth_failure(status, description))
    })
}

fn problem_missing(name: &str) -> Response {
    error(
        StatusCode::NOT_FOUND,
        "I couldn't use that journal source.",
        "journal_source_problem",
        format!("Journal source '{name}' not found"),
    )
}

fn valid_name(name: &str) -> bool {
    !name.is_empty() && !matches!(name, "." | "..") && !name.contains(['/', '\\'])
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock after epoch")
        .as_millis() as i64
}

fn generated_key() -> Result<String, ()> {
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes).map_err(|_| ())?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

fn source_path(root: &Path, name: &str) -> std::path::PathBuf {
    root.join("apps/import/journal_sources")
        .join(format!("{name}.json"))
}

fn source_record_path(root: &Path, record: &Map<String, Value>) -> Option<std::path::PathBuf> {
    if let Some(name) = record
        .get("name")
        .and_then(Value::as_str)
        .filter(|name| valid_name(name))
    {
        return Some(source_path(root, name));
    }
    let prefix = state_prefix(record)?;
    Some(
        root.join("apps/import/journal_sources")
            .join(format!("{prefix}.json")),
    )
}

fn contained_source_record_path(
    root: &Path,
    record: &Map<String, Value>,
) -> Result<std::path::PathBuf, ()> {
    let path = source_record_path(root, record).ok_or(())?;
    let relative = path
        .strip_prefix(root)
        .map_err(|_| ())?
        .to_str()
        .ok_or(())?;
    contained_path(root, relative).map_err(|_| ())
}

fn save_journal_source(root: &Path, record: &Map<String, Value>) -> Result<(), ()> {
    let path = contained_source_record_path(root, record)?;
    fs::create_dir_all(path.parent().ok_or(())?).map_err(|_| ())?;
    let bytes = serde_json::to_vec_pretty(&Value::Object(record.clone())).map_err(|_| ())?;
    atomic_replace(path, &bytes, AtomicWriteOptions { mode: Some(0o600) }).map_err(|_| ())
}

#[cfg(test)]
use std::{cell::RefCell, rc::Rc};
#[cfg(test)]
thread_local! {
    static MUTATION_HOOK: RefCell<Option<Rc<dyn Fn()>>> = const { RefCell::new(None) };
}
#[cfg(test)]
struct MutationHookGuard(Option<Rc<dyn Fn()>>);
#[cfg(test)]
impl Drop for MutationHookGuard {
    fn drop(&mut self) {
        MUTATION_HOOK.with(|hook| *hook.borrow_mut() = self.0.take());
    }
}
#[cfg(test)]
fn install_mutation_hook(hook: Rc<dyn Fn()>) -> MutationHookGuard {
    MutationHookGuard(MUTATION_HOOK.with(|current| current.replace(Some(hook))))
}
#[cfg(test)]
fn run_mutation_hook() {
    MUTATION_HOOK.with(|hook| {
        if let Some(hook) = hook.borrow().as_ref() {
            hook();
        }
    });
}
#[cfg(not(test))]
fn run_mutation_hook() {}

pub(crate) fn record_received(
    root: &Path,
    identity: &JournalSourceIdentity,
    stat: &str,
    amount: usize,
) -> Result<(), String> {
    if amount == 0 {
        return Ok(());
    }
    let path = contained_source_record_path(root, &identity.source)
        .map_err(|_| "Journal source has no safe record path".to_owned())?;
    let _lock = hold_lock(
        &path,
        LockOptions {
            mode: Some(0o600),
            ..LockOptions::default()
        },
    )
    .map_err(|error| format!("Failed to lock journal source statistics: {error}"))?;
    let mut record = serde_json::from_slice::<Value>(
        &fs::read(&path)
            .map_err(|error| format!("Failed to read journal source statistics: {error}"))?,
    )
    .map_err(|error| format!("Failed to parse journal source statistics: {error}"))?
    .as_object()
    .cloned()
    .ok_or_else(|| "Journal source statistics record must be an object".to_owned())?;
    run_mutation_hook();
    let stats = record
        .entry("stats".to_owned())
        .or_insert_with(|| json!({}));
    let object = stats
        .as_object_mut()
        .ok_or_else(|| "Journal source statistics must be an object".to_owned())?;
    let prior = object.get(stat).and_then(Value::as_u64).unwrap_or(0);
    object.insert(stat.to_owned(), json!(prior.saturating_add(amount as u64)));
    save_journal_source(root, &record)
        .map_err(|_| "Failed to persist journal source statistics".to_owned())
}

fn create_state_directory(root: &Path, key_prefix: &str) -> Result<(), ()> {
    let state_dir = contained_path(root, &format!("imports/{key_prefix}")).map_err(|_| ())?;
    fs::create_dir_all(&state_dir).map_err(|_| ())?;
    let source_state =
        contained_path(root, &format!("imports/{key_prefix}/source.json")).map_err(|_| ())?;
    if !source_state.exists() {
        atomic_replace(&source_state, b"{}", AtomicWriteOptions::default()).map_err(|_| ())?;
    }
    for area in STATE_AREAS {
        let directory =
            contained_path(root, &format!("imports/{key_prefix}/{area}")).map_err(|_| ())?;
        fs::create_dir_all(&directory).map_err(|_| ())?;
        let state = contained_path(root, &format!("imports/{key_prefix}/{area}/state.json"))
            .map_err(|_| ())?;
        if !state.exists() {
            atomic_replace(&state, b"{}", AtomicWriteOptions::default()).map_err(|_| ())?;
        }
    }
    Ok(())
}

fn append_action(root: &Path, action: &str, params: Value) -> Result<(), ()> {
    let day = Local::now().format("%Y%m%d").to_string();
    append_jsonl(
        contained_path(root, &format!("config/actions/{day}.jsonl")).map_err(|_| ())?,
        &json!({
            "timestamp": Utc::now().to_rfc3339(),
            "source": "app",
            "actor": "import",
            "action": action,
            "params": params,
        }),
    )
    .map_err(|_| ())
}

#[derive(Debug)]
enum RevokeSourceError {
    Missing,
    AlreadyRevoked,
    Persist,
}

enum CreateSourceError {
    Exists,
    Persist,
}

fn create_source(root: &Path, record: &Map<String, Value>) -> Result<(), CreateSourceError> {
    let path =
        contained_source_record_path(root, record).map_err(|_| CreateSourceError::Persist)?;
    let _lock = hold_lock(
        &path,
        LockOptions {
            mode: Some(0o600),
            ..LockOptions::default()
        },
    )
    .map_err(|_| CreateSourceError::Persist)?;
    if path.exists() {
        return Err(CreateSourceError::Exists);
    }
    save_journal_source(root, record).map_err(|_| CreateSourceError::Persist)
}

fn revoke_source(root: &Path, name: &str) -> Result<String, RevokeSourceError> {
    let snapshot = source(root, name).ok_or(RevokeSourceError::Missing)?;
    let path =
        contained_source_record_path(root, &snapshot).map_err(|_| RevokeSourceError::Persist)?;
    let _lock = hold_lock(
        &path,
        LockOptions {
            mode: Some(0o600),
            ..LockOptions::default()
        },
    )
    .map_err(|_| RevokeSourceError::Persist)?;
    let mut record =
        serde_json::from_slice::<Value>(&fs::read(&path).map_err(|_| RevokeSourceError::Persist)?)
            .map_err(|_| RevokeSourceError::Persist)?
            .as_object()
            .cloned()
            .ok_or(RevokeSourceError::Persist)?;
    run_mutation_hook();
    if record.get("revoked") == Some(&Value::Bool(true)) {
        return Err(RevokeSourceError::AlreadyRevoked);
    }
    let key_prefix = state_prefix(&record).ok_or(RevokeSourceError::Missing)?;
    record.insert("revoked".to_owned(), json!(true));
    record.insert("revoked_at".to_owned(), json!(now_ms()));
    save_journal_source(root, &record).map_err(|_| RevokeSourceError::Persist)?;
    Ok(key_prefix)
}

pub(crate) async fn create(State(state): State<AppState>, Json(data): Json<Value>) -> Response {
    let name = data
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    if name.is_empty() {
        return error(
            StatusCode::BAD_REQUEST,
            "I couldn't find a required field.",
            "missing_required_field",
            "Name is required".to_owned(),
        );
    }
    if !valid_name(name) {
        return error(
            StatusCode::BAD_REQUEST,
            "I couldn't use that journal source.",
            "journal_source_problem",
            "Invalid journal source name".to_owned(),
        );
    }
    if source(&state.root, name).is_some() {
        return error(
            StatusCode::CONFLICT,
            "I couldn't use that journal source.",
            "journal_source_problem",
            format!("Journal source '{name}' already exists"),
        );
    }
    let Ok(key) = generated_key() else {
        return error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "I couldn't use that journal source.",
            "journal_source_problem",
            "Failed to save journal source".to_owned(),
        );
    };
    let key_prefix = key[..8].to_owned();
    let record = Map::from_iter([
        ("key".to_owned(), json!(key)),
        ("name".to_owned(), json!(name)),
        ("created_at".to_owned(), json!(now_ms())),
        ("enabled".to_owned(), json!(true)),
        ("revoked".to_owned(), json!(false)),
        ("revoked_at".to_owned(), Value::Null),
        (
            "stats".to_owned(),
            json!({"segments_received":0,"entities_received":0,"facets_received":0,"imports_received":0,"config_received":0}),
        ),
    ]);
    match create_source(&state.root, &record) {
        Ok(()) => {}
        Err(CreateSourceError::Exists) => {
            return error(
                StatusCode::CONFLICT,
                "I couldn't use that journal source.",
                "journal_source_problem",
                format!("Journal source '{name}' already exists"),
            );
        }
        Err(CreateSourceError::Persist) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "I couldn't use that journal source.",
                "journal_source_problem",
                "Failed to save journal source".to_owned(),
            );
        }
    }
    if create_state_directory(&state.root, &key_prefix).is_err()
        || append_action(
            &state.root,
            "journal_source_create",
            json!({"name": name, "key_prefix": key_prefix}),
        )
        .is_err()
    {
        return error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "I couldn't use that journal source.",
            "journal_source_problem",
            "Failed to save journal source".to_owned(),
        );
    }
    json_response(
        StatusCode::OK,
        json!({"key": key, "key_prefix": key_prefix, "name": name}),
    )
}

pub(crate) async fn revoke(
    State(state): State<AppState>,
    AxumPath(name): AxumPath<String>,
) -> Response {
    let key_prefix = match revoke_source(&state.root, &name) {
        Ok(prefix) => prefix,
        Err(RevokeSourceError::Missing) => return problem_missing(&name),
        Err(RevokeSourceError::AlreadyRevoked) => {
            return error(
                StatusCode::CONFLICT,
                "I couldn't use that journal source.",
                "journal_source_problem",
                format!("Journal source '{name}' is already revoked"),
            );
        }
        Err(RevokeSourceError::Persist) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "I couldn't use that journal source.",
                "journal_source_problem",
                "Failed to save journal source".to_owned(),
            );
        }
    };
    if append_action(
        &state.root,
        "journal_source_revoke",
        json!({"name": name, "key_prefix": key_prefix}),
    )
    .is_err()
    {
        return error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "I couldn't use that journal source.",
            "journal_source_problem",
            "Failed to save journal source".to_owned(),
        );
    }
    json_response(
        StatusCode::OK,
        json!({"name": name, "prefix": key_prefix, "revoked": true}),
    )
}

pub(crate) async fn manifest(
    State(state): State<AppState>,
    AxumPath((key_prefix, area)): AxumPath<(String, String)>,
    identity: JournalSourceIdentity,
) -> Response {
    debug_assert_eq!(identity.derived_prefix, key_prefix);
    if !STATE_AREAS.contains(&area.as_str()) {
        return error(
            StatusCode::NOT_FOUND,
            "I couldn't use one of those values.",
            "invalid_request_value",
            "Unknown manifest area".to_owned(),
        );
    }
    let state_file = state
        .root
        .join("imports")
        .join(identity.derived_prefix)
        .join(area)
        .join("state.json");
    let value = fs::read_to_string(state_file)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .unwrap_or_else(|| json!({}));
    json_response(StatusCode::OK, value)
}

pub(crate) async fn list(State(state): State<AppState>) -> Response {
    let items: Vec<Value> = records(&state.root).into_iter().filter(|record| record.get("pair_mode").and_then(Value::as_str) != Some("pl")).filter_map(|record| Some(json!({
        "name": record.get("name")?, "prefix": prefix(&record)?,
        "status": if record.get("revoked") == Some(&Value::Bool(true)) { "revoked" } else { "active" },
        "created_at": record.get("created_at")?,
    }))).collect();
    json_response(
        StatusCode::OK,
        json!({"items": items, "total": items.len()}),
    )
}

pub(crate) async fn status(
    State(state): State<AppState>,
    AxumPath(name): AxumPath<String>,
) -> Response {
    let Some(record) = source(&state.root, &name) else {
        return problem_missing(&name);
    };
    let Some(prefix) = prefix(&record) else {
        return problem_missing(&name);
    };
    json_response(
        StatusCode::OK,
        json!({
            "name": record.get("name").cloned().unwrap_or_else(|| json!("")), "prefix": prefix,
            "status": if record.get("revoked") == Some(&Value::Bool(true)) { "revoked" } else { "active" },
            "created_at": record.get("created_at").cloned().unwrap_or(Value::Null),
            "revoked": record.get("revoked").cloned().unwrap_or(Value::Bool(false)),
            "revoked_at": record.get("revoked_at").cloned().unwrap_or(Value::Null),
            "stats": record.get("stats").cloned().unwrap_or_else(|| json!({})),
        }),
    )
}

pub(crate) async fn staged(
    State(state): State<AppState>,
    AxumPath(name): AxumPath<String>,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    let Some(record) = source(&state.root, &name) else {
        return problem_missing(&name);
    };
    let Some(prefix) = prefix(&record) else {
        return problem_missing(&name);
    };
    let area = query.get("area").map(String::as_str);
    if area.is_some_and(|area| !matches!(area, "entities" | "facets" | "config")) {
        return error(
            StatusCode::BAD_REQUEST,
            "I couldn't use one of those values.",
            "invalid_request_value",
            "Area must be one of: entities, facets, config".to_owned(),
        );
    }
    let root = state.root.join("imports").join(prefix);
    let mut items = Vec::new();
    let entities = root.join("entities/staged");
    if area.is_none_or(|area| area == "entities")
        && let Ok(entries) = fs::read_dir(entities)
    {
        for entry in entries.flatten() {
            if let Ok(text) = fs::read_to_string(entry.path())
                && let Ok(value) = serde_json::from_str::<Value>(&text)
                && let Some(payload) = value.as_object()
            {
                items.push(json!({"area":"entities", "source_id": entry.path().file_stem().and_then(|x| x.to_str()).unwrap_or(""), "reason": payload.get("reason"), "source_entity": payload.get("source_entity"), "match_candidates": payload.get("match_candidates"), "staged_at": payload.get("staged_at")}));
            }
        }
    }
    let facets = root.join("facets/staged");
    if area.is_none_or(|area| area == "facets") && facets.exists() {
        let mut staged = Vec::new();
        collect_staged_facets(&facets, &facets, &mut staged);
        items.extend(staged);
    }
    if area.is_none_or(|area| area == "config")
        && let Ok(text) = fs::read_to_string(root.join("config/diff.json"))
        && let Ok(diff) = serde_json::from_str::<Value>(&text)
        && diff.is_object()
    {
        items.push(json!({"area":"config", "diff": diff}));
    }
    json_response(
        StatusCode::OK,
        json!({"items": items, "total": items.len()}),
    )
}

fn collect_staged_facets(root: &Path, directory: &Path, items: &mut Vec<Value>) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_staged_facets(root, &path, items);
            continue;
        }
        if !path.to_string_lossy().ends_with(".staged.json") {
            continue;
        }
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };
        let Ok(Value::Object(payload)) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        let Ok(relative) = path.strip_prefix(root) else {
            continue;
        };
        let parts: Vec<_> = relative.components().collect();
        if parts.len() < 3 {
            continue;
        }
        let facet = parts[0].as_os_str().to_string_lossy();
        let file_type = parts[1].as_os_str().to_string_lossy();
        let mut item = Map::new();
        item.insert("area".into(), json!("facets"));
        item.insert("staged_file".into(), json!(relative.to_string_lossy()));
        item.insert("facet".into(), json!(facet));
        item.insert("file_type".into(), json!(file_type));
        item.extend(payload);
        items.push(Value::Object(item));
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        os::unix::fs::PermissionsExt,
        rc::Rc,
        sync::{
            Arc, Barrier,
            atomic::{AtomicBool, Ordering},
            mpsc,
        },
        thread,
    };

    use axum::{
        Extension,
        body::{Body, to_bytes},
        http::{Request, StatusCode},
    };
    use serde_json::json;
    use solstone_core_convey_http::identity::{AccessBasis, Carrier, LinkedDeviceCid};
    use tempfile::TempDir;
    use tower::ServiceExt;

    use super::{
        DoorIdentity, IngestIdentityCase, JournalSourceIdentity, authorize, bearer_identity,
        create_state_directory, record_received, revoke_source,
    };

    fn source(key: &str, revoked: bool) -> serde_json::Map<String, serde_json::Value> {
        serde_json::Map::from_iter([
            ("key".to_owned(), json!(key)),
            ("name".to_owned(), json!("source")),
            ("enabled".to_owned(), json!(true)),
            ("revoked".to_owned(), json!(revoked)),
        ])
    }

    fn write_source(root: &std::path::Path, name: &str, record: serde_json::Value) {
        let directory = root.join("apps/import/journal_sources");
        fs::create_dir_all(&directory).unwrap();
        fs::write(
            directory.join(format!("{name}.json")),
            serde_json::to_vec(&record).unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn criterion_14_identity_cases_keep_their_distinct_status_and_description() {
        let root = TempDir::new().unwrap();
        write_source(
            root.path(),
            "dl",
            serde_json::Value::Object(source("valid-key-123456789", false)),
        );
        write_source(
            root.path(),
            "revoked-dl",
            serde_json::Value::Object(source("revoked-key-123456", true)),
        );
        write_source(
            root.path(),
            "pl-revoked",
            json!({"pair_mode":"pl","fingerprint":"sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","revoked":true,"enabled":true}),
        );
        write_source(
            root.path(),
            "pl-disabled",
            json!({"pair_mode":"pl","fingerprint":"sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","revoked":false,"enabled":false}),
        );

        let cases = [
            (
                IngestIdentityCase::MissingAuth,
                StatusCode::UNAUTHORIZED,
                "Missing or invalid authentication",
            ),
            (
                IngestIdentityCase::InvalidPlIdentity,
                StatusCode::UNAUTHORIZED,
                "Invalid PL identity",
            ),
            (
                IngestIdentityCase::PlRevoked,
                StatusCode::FORBIDDEN,
                "Journal source has been revoked",
            ),
            (
                IngestIdentityCase::PlDisabled,
                StatusCode::FORBIDDEN,
                "Journal source is disabled",
            ),
            (
                IngestIdentityCase::InvalidApiKey,
                StatusCode::UNAUTHORIZED,
                "Invalid API key",
            ),
            (
                IngestIdentityCase::DlRevoked,
                StatusCode::FORBIDDEN,
                "API key has been revoked",
            ),
            (
                IngestIdentityCase::PrefixMismatch,
                StatusCode::FORBIDDEN,
                "Key prefix mismatch",
            ),
        ];
        for (case, status, description) in cases {
            assert_eq!(case.response(), (status, description));
        }
        assert_eq!(
            bearer_identity(&axum::http::HeaderMap::new()),
            Err(IngestIdentityCase::MissingAuth)
        );
        assert_eq!(
            authorize(
                root.path(),
                "anything",
                DoorIdentity::PrivateLink(
                    "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"
                )
            ),
            Err(IngestIdentityCase::InvalidPlIdentity)
        );
        assert_eq!(
            authorize(
                root.path(),
                "anything",
                DoorIdentity::PrivateLink(
                    "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                )
            ),
            Err(IngestIdentityCase::PlRevoked)
        );
        assert_eq!(
            authorize(
                root.path(),
                "anything",
                DoorIdentity::PrivateLink(
                    "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
                )
            ),
            Err(IngestIdentityCase::PlDisabled)
        );
        assert_eq!(
            authorize(root.path(), "anything", DoorIdentity::Bearer("missing-key")),
            Err(IngestIdentityCase::InvalidApiKey)
        );
        assert_eq!(
            authorize(
                root.path(),
                "anything",
                DoorIdentity::Bearer("revoked-key-123456")
            ),
            Err(IngestIdentityCase::DlRevoked)
        );
        assert_eq!(
            authorize(
                root.path(),
                "other-prefix",
                DoorIdentity::Bearer("valid-key-123456789")
            ),
            Err(IngestIdentityCase::PrefixMismatch)
        );
    }

    #[test]
    fn state_directory_fills_only_absent_state_files() {
        let root = TempDir::new().unwrap();
        create_state_directory(root.path(), "prefix01").unwrap();
        let state = root.path().join("imports/prefix01/segments/state.json");
        fs::write(&state, b"{\"received\":7}").unwrap();
        create_state_directory(root.path(), "prefix01").unwrap();
        assert_eq!(fs::read(&state).unwrap(), b"{\"received\":7}");
        assert_eq!(
            fs::read(root.path().join("imports/prefix01/source.json")).unwrap(),
            b"{}"
        );
        for area in ["entities", "facets", "imports", "config"] {
            assert_eq!(
                fs::read(
                    root.path()
                        .join(format!("imports/prefix01/{area}/state.json"))
                )
                .unwrap(),
                b"{}"
            );
        }
    }

    #[test]
    fn concurrent_received_counters_reload_under_one_source_lock() {
        let root = TempDir::new().unwrap();
        let record = source("prefix01-key-material", false);
        write_source(
            root.path(),
            "source",
            serde_json::Value::Object(record.clone()),
        );
        let path = root.path().join("apps/import/journal_sources/source.json");
        let root = Arc::new(root.path().to_owned());
        let identity = Arc::new(JournalSourceIdentity {
            source: record,
            derived_prefix: "prefix01".to_owned(),
        });
        let start = Arc::new(Barrier::new(3));
        let release = Arc::new(Barrier::new(2));
        let first = Arc::new(AtomicBool::new(true));
        let (entered_tx, entered_rx) = mpsc::channel();
        let (parked_tx, parked_rx) = mpsc::channel();
        let segments = {
            let root = Arc::clone(&root);
            let identity = Arc::clone(&identity);
            let start = Arc::clone(&start);
            let release = Arc::clone(&release);
            let first = Arc::clone(&first);
            let entered_tx = entered_tx.clone();
            let parked_tx = parked_tx.clone();
            let path = path.clone();
            thread::spawn(move || {
                let _guard = super::install_mutation_hook(Rc::new(move || {
                    if first.swap(false, Ordering::SeqCst) {
                        parked_tx.send("segments_received").unwrap();
                        release.wait();
                    } else {
                        let saved: serde_json::Value =
                            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
                        assert_eq!(saved["stats"]["entities_received"], 1);
                    }
                }));
                start.wait();
                entered_tx.send("segments_received").unwrap();
                record_received(&root, &identity, "segments_received", 1).unwrap();
            })
        };
        let entities = {
            let root = Arc::clone(&root);
            let identity = Arc::clone(&identity);
            let start = Arc::clone(&start);
            let release = Arc::clone(&release);
            let first = Arc::clone(&first);
            thread::spawn(move || {
                let _guard = super::install_mutation_hook(Rc::new(move || {
                    if first.swap(false, Ordering::SeqCst) {
                        parked_tx.send("entities_received").unwrap();
                        release.wait();
                    } else {
                        let saved: serde_json::Value =
                            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
                        assert_eq!(saved["stats"]["segments_received"], 1);
                    }
                }));
                start.wait();
                entered_tx.send("entities_received").unwrap();
                record_received(&root, &identity, "entities_received", 1).unwrap();
            })
        };
        start.wait();
        parked_rx.recv().unwrap();
        entered_rx.recv().unwrap();
        entered_rx.recv().unwrap();
        release.wait();
        segments.join().unwrap();
        entities.join().unwrap();

        let saved: serde_json::Value = serde_json::from_slice(
            &fs::read(root.join("apps/import/journal_sources/source.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(saved["key"], "prefix01-key-material");
        assert_eq!(saved["name"], "source");
        assert_eq!(saved["enabled"], true);
        assert_eq!(saved["revoked"], false);
        assert_eq!(saved["stats"]["segments_received"], 1);
        assert_eq!(saved["stats"]["entities_received"], 1);
    }

    #[test]
    fn concurrent_revoke_and_counter_update_preserve_both_changes() {
        let root = TempDir::new().unwrap();
        let record = source("prefix01-key-material", false);
        write_source(
            root.path(),
            "source",
            serde_json::Value::Object(record.clone()),
        );
        let path = root.path().join("apps/import/journal_sources/source.json");
        let root = Arc::new(root.path().to_owned());
        let identity = Arc::new(JournalSourceIdentity {
            source: record,
            derived_prefix: "prefix01".to_owned(),
        });
        let start = Arc::new(Barrier::new(3));
        let release = Arc::new(Barrier::new(2));
        let first = Arc::new(AtomicBool::new(true));
        let (entered_tx, entered_rx) = mpsc::channel();
        let (parked_tx, parked_rx) = mpsc::channel();
        let increment = {
            let root = Arc::clone(&root);
            let identity = Arc::clone(&identity);
            let start = Arc::clone(&start);
            let release = Arc::clone(&release);
            let first = Arc::clone(&first);
            let entered_tx = entered_tx.clone();
            let parked_tx = parked_tx.clone();
            let path = path.clone();
            thread::spawn(move || {
                let _guard = super::install_mutation_hook(Rc::new(move || {
                    if first.swap(false, Ordering::SeqCst) {
                        parked_tx.send("segments_received").unwrap();
                        release.wait();
                    } else {
                        let saved: serde_json::Value =
                            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
                        assert_eq!(saved["revoked"], true);
                    }
                }));
                start.wait();
                entered_tx.send("segments_received").unwrap();
                record_received(&root, &identity, "segments_received", 1).unwrap();
            })
        };
        let revoke = {
            let root = Arc::clone(&root);
            let start = Arc::clone(&start);
            let release = Arc::clone(&release);
            let first = Arc::clone(&first);
            thread::spawn(move || {
                let _guard = super::install_mutation_hook(Rc::new(move || {
                    if first.swap(false, Ordering::SeqCst) {
                        parked_tx.send("revoked").unwrap();
                        release.wait();
                    } else {
                        let saved: serde_json::Value =
                            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
                        assert_eq!(saved["stats"]["segments_received"], 1);
                    }
                }));
                start.wait();
                entered_tx.send("revoked").unwrap();
                revoke_source(&root, "source").unwrap();
            })
        };
        start.wait();
        parked_rx.recv().unwrap();
        entered_rx.recv().unwrap();
        entered_rx.recv().unwrap();
        release.wait();
        increment.join().unwrap();
        revoke.join().unwrap();

        let saved: serde_json::Value = serde_json::from_slice(
            &fs::read(root.join("apps/import/journal_sources/source.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(saved["key"], "prefix01-key-material");
        assert_eq!(saved["name"], "source");
        assert_eq!(saved["enabled"], true);
        assert_eq!(saved["revoked"], true);
        assert!(saved["revoked_at"].as_i64().is_some());
        assert_eq!(saved["stats"]["segments_received"], 1);
    }

    #[tokio::test]
    async fn criterion_12_unknown_prefix_refuses_like_known_prefix() {
        let root = TempDir::new().unwrap();
        super::create_state_directory(root.path(), "prefix01").unwrap();
        let router = crate::routes(root.path().to_path_buf());
        let mut responses = Vec::new();
        for path in [
            "/app/import/journal/prefix01/manifest/entities",
            "/app/import/journal/unknown0/manifest/entities",
        ] {
            let response = router
                .clone()
                .oneshot(Request::get(path).body(Body::empty()).unwrap())
                .await
                .unwrap();
            let status = response.status();
            let content_type = response.headers()["content-type"].clone();
            let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
            responses.push((status, content_type, body));
        }
        assert_eq!(responses[0], responses[1]);
        assert_eq!(responses[0].0, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn private_link_transport_identity_authenticates_the_manifest_door() {
        const FINGERPRINT: &str =
            "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let root = TempDir::new().unwrap();
        write_source(
            root.path(),
            "pl-peer",
            json!({
                "name":"pl-peer",
                "pair_mode":"pl",
                "fingerprint":FINGERPRINT,
                "enabled":true,
                "revoked":false,
            }),
        );
        let prefix = "aaaaaaaaaaaaaaaa";
        create_state_directory(root.path(), prefix).unwrap();
        let router =
            crate::routes(root.path().to_path_buf()).layer(Extension(AccessBasis::LinkedDevice {
                carrier: Carrier::Direct,
                cid: LinkedDeviceCid::try_from(FINGERPRINT).unwrap(),
            }));

        let response = router
            .oneshot(
                Request::get(format!("/app/import/journal/{prefix}/manifest/entities"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn create_persists_private_source_state_and_action() {
        let root = TempDir::new().unwrap();
        let response = crate::routes(root.path().to_path_buf())
            .oneshot(
                Request::post("/app/import/api/journal-sources/create")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"name":"new_source"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let response: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let prefix = response["key_prefix"].as_str().unwrap();
        let source = root
            .path()
            .join("apps/import/journal_sources/new_source.json");
        assert_eq!(
            fs::metadata(source).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert!(
            root.path()
                .join("imports")
                .join(prefix)
                .join("config/state.json")
                .exists()
        );
        assert!(root.path().join("config/actions").exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn symlinked_journal_source_directory_is_refused_without_an_external_record() {
        use std::os::unix::fs::symlink;

        let root = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        fs::create_dir_all(root.path().join("apps/import")).unwrap();
        symlink(
            outside.path(),
            root.path().join("apps/import/journal_sources"),
        )
        .unwrap();
        let response = crate::routes(root.path().to_path_buf())
            .oneshot(
                Request::post("/app/import/api/journal-sources/create")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"name":"peer"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert!(fs::read_dir(outside.path()).unwrap().next().is_none());
    }

    #[tokio::test]
    async fn revoke_marks_the_source_and_appends_its_action() {
        let root = TempDir::new().unwrap();
        write_source(
            root.path(),
            "source",
            serde_json::Value::Object(source("source-key-123456789", false)),
        );
        let response = crate::routes(root.path().to_path_buf())
            .oneshot(
                Request::post("/app/import/api/journal-sources/source/revoke")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let record: serde_json::Value = serde_json::from_slice(
            &fs::read(root.path().join("apps/import/journal_sources/source.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(record["revoked"], true);
        assert!(record["revoked_at"].is_i64());
        assert!(root.path().join("config/actions").exists());
    }
}
