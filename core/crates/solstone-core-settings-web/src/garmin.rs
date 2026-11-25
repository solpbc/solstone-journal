// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Settings routes for Garmin Connect Personal API key connection, status,
//! manual sync / range backfill, per-run recoverable Trash, and first-connect
//! Body facet selection.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::{
    body::Bytes,
    extract::Path as AxumPath,
    http::{HeaderMap, StatusCode, header::USER_AGENT},
    response::{IntoResponse, Response},
};
use jiff::Timestamp;
use jiff::civil::Date;
use serde_json::{Map, Value, json};
use solstone_core_callosum::{
    CallosumConnectionPhase, CallosumReceiveEvent, CallosumSocketConnection,
};
use solstone_core_convey_http::window_catalog::{strings, text};
use solstone_core_facets::BodyFacetAsk;
use solstone_core_import::garmin::{self, Error};
use solstone_core_import::import_trash::{
    ImportThings, Imports, deletes_here, failure_key, import_things, is_import_id,
};
use solstone_core_indexer_store::RetentionIndex;
use solstone_core_journal_config::PREFERRED_BODY_FACET;
use solstone_core_removals::{Action, Page, Surface};
use solstone_core_trash::TrashError;

use crate::{
    http::json_response,
    request_body::{JsonBody, json_body},
};

/// Garmin Connect sync runs on desktop journals only; mobile journals hide the card.
pub(crate) const GARMIN_AVAILABLE: bool = !cfg!(any(target_os = "android", target_os = "ios"));

/// A Garmin sync this server requested via Callosum is still running.
static SYNCING: AtomicBool = AtomicBool::new(false);

const BUS_CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
const SYNC_WATCH_MAX_DURATION: Duration = Duration::from_secs(35 * 60);

const BODY_FACET_COPY: &[(&str, &str)] = &[
    ("heading", "journal.window.import.body.facet.shared.heading"),
    (
        "ask_existing",
        "journal.window.import.body.facet.apple.ask.named",
    ),
    (
        "ask_make_body",
        "journal.window.import.body.facet.apple.ask.make.body",
    ),
    (
        "ask_unmute",
        "journal.window.import.body.facet.apple.ask.unmute",
    ),
    (
        "no_means",
        "journal.window.import.body.facet.apple.no.means",
    ),
    ("yes", "journal.window.import.body.facet.shared.yes"),
    ("no", "journal.window.import.body.facet.shared.no"),
    (
        "this_import",
        "journal.window.import.body.facet.shared.this.import",
    ),
    (
        "none_option",
        "journal.window.import.body.facet.shared.none.option",
    ),
    (
        "files_earlier",
        "journal.window.import.body.facet.apple.files.named",
    ),
    (
        "none_hint",
        "journal.window.import.body.facet.apple.none.hint",
    ),
    (
        "unanswered",
        "journal.window.import.body.facet.shared.unanswered",
    ),
    (
        "answer_failed",
        "journal.window.import.body.facet.shared.answer.failed",
    ),
    (
        "offer_changed",
        "journal.window.import.body.facet.shared.offer.changed",
    ),
];

fn response_text(locale: &str, error: Error) -> String {
    let id = match error {
        Error::InvalidKey => "journal.window.settings.garmin.response.invalidkey",
        Error::InvalidExpiry => "journal.window.settings.garmin.response.invalidexpiry",
        Error::InvalidRange => "journal.window.settings.garmin.response.invalidrange",
        Error::Key => "journal.window.settings.garmin.response.key",
        Error::Access => "journal.window.settings.garmin.response.access",
        Error::Limited => "journal.window.settings.garmin.response.limited",
        Error::Unavailable => "journal.window.settings.garmin.response.unavailable",
        Error::Response => "journal.window.settings.garmin.response.response",
        Error::Storage => "journal.window.settings.garmin.response.storage",
        Error::Busy => "journal.window.settings.garmin.response.busy",
        Error::Stopped => "journal.window.settings.garmin.response.stopped",
        Error::NotConnected => "journal.window.settings.garmin.response.notconnected",
        Error::UnavailableOnMobile => "journal.window.settings.garmin.response.mobileunavailable",
    };
    text(locale, id)
}

fn attention_text(locale: &str, code: &str) -> String {
    let id = match code {
        "key" => "journal.window.settings.garmin.response.key",
        "key_expiring" => "journal.window.settings.garmin.response.keyexpiring",
        "access" => "journal.window.settings.garmin.response.access",
        "limited" => "journal.window.settings.garmin.response.limited",
        "response" => "journal.window.settings.garmin.response.response",
        "storage" => "journal.window.settings.garmin.response.storage",
        "busy" => "journal.window.settings.garmin.response.busy",
        "stopped" => "journal.window.settings.garmin.response.stopped",
        _ => "journal.window.settings.garmin.response.unavailable",
    };
    text(locale, id)
}

fn pasted_text(locale: &str, error: Error) -> String {
    match error {
        Error::Key => text(locale, "journal.window.settings.garmin.paste.key"),
        Error::Access => text(locale, "journal.window.settings.garmin.paste.access"),
        other => response_text(locale, other),
    }
}

fn error_response(status: StatusCode, code: &str, message: String) -> Response {
    (
        status,
        axum::Json(json!({ "error": message, "code": code })),
    )
        .into_response()
}

fn failed(locale: &str, error: Error) -> Response {
    let status = match error {
        Error::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
        _ => StatusCode::BAD_REQUEST,
    };
    error_response(status, error.code(), response_text(locale, error))
}

fn refused_paste(locale: &str, error: Error) -> Response {
    error_response(
        StatusCode::BAD_REQUEST,
        error.code(),
        pasted_text(locale, error),
    )
}

pub async fn get(locale: &str, root: PathBuf) -> Response {
    let syncing = SYNCING.load(Ordering::SeqCst);
    match tokio::task::spawn_blocking(move || garmin::status(&root, syncing, GARMIN_AVAILABLE))
        .await
    {
        Ok(Ok(status)) => {
            let attention = status
                .attention_code
                .as_deref()
                .map(|code| attention_text(locale, code));
            json_response(garmin::status_json(&status, attention))
        }
        Ok(Err(error)) => failed(locale, error),
        Err(_) => failed(locale, Error::Storage),
    }
}

pub async fn connect(locale: &str, root: PathBuf, body: Bytes) -> Response {
    if !GARMIN_AVAILABLE {
        return failed(locale, Error::UnavailableOnMobile);
    }
    let JsonBody::Value(value) = json_body(body) else {
        return failed(locale, Error::InvalidKey);
    };
    let Some(key) = value
        .get("api_key")
        .and_then(Value::as_str)
        .map(|raw| raw.trim().to_owned())
    else {
        return failed(locale, Error::InvalidKey);
    };
    if let Err(error) = garmin::validate_key(&key) {
        return failed(locale, error);
    }
    let expires_on = match value.get("key_expires_on") {
        None | Some(Value::Null) => None,
        Some(Value::String(raw)) if raw.trim().is_empty() => None,
        Some(Value::String(raw)) => match garmin::validate_expiry(raw) {
            Ok(normalized) => Some(normalized),
            Err(error) => return failed(locale, error),
        },
        Some(_) => return failed(locale, Error::InvalidExpiry),
    };

    // Validate-before-replace: a key Garmin rejects with 401/403 never overwrites
    // the saved key. If Garmin is transiently unreachable, save the key and let
    // the sync report what it finds.
    let probe_key = key.clone();
    let probed_cache =
        match tokio::task::spawn_blocking(move || garmin::check_key(&probe_key)).await {
            Ok(Ok(cache)) => Some(cache),
            Ok(Err(error @ (Error::Key | Error::Access))) => return refused_paste(locale, error),
            Ok(Err(_)) => None,
            Err(_) => return failed(locale, Error::Storage),
        };

    let sync_root = root.clone();
    let result = tokio::task::spawn_blocking(move || {
        change(
            &root,
            Some(&key),
            expires_on.as_deref(),
            probed_cache.as_ref(),
        )
    })
    .await;
    match result {
        Ok(Ok(())) => {
            let cmd = vec![
                "solstone".to_owned(),
                "journal".to_owned(),
                "importer".to_owned(),
                "--sync".to_owned(),
                "garmin".to_owned(),
                "--save".to_owned(),
            ];
            let _ = try_dispatch_desktop_sync(&sync_root, cmd).await;
            json_response(json!({ "success": true }))
        }
        Ok(Err(error)) => failed(locale, error),
        Err(_) => failed(locale, Error::Storage),
    }
}

pub async fn disconnect(locale: &str, root: PathBuf) -> Response {
    match tokio::task::spawn_blocking(move || change(&root, None, None, None)).await {
        Ok(Ok(())) => json_response(json!({ "success": true })),
        Ok(Err(error)) => failed(locale, error),
        Err(_) => failed(locale, Error::Storage),
    }
}

pub async fn sync_now(locale: &str, root: PathBuf) -> Response {
    if !GARMIN_AVAILABLE {
        return failed(locale, Error::UnavailableOnMobile);
    }
    let check_root = root.clone();
    let Ok((connected, locked)) = tokio::task::spawn_blocking(move || {
        (
            garmin::key(&check_root).ok().flatten().is_some(),
            garmin::is_sync_locked(&check_root),
        )
    })
    .await
    else {
        return failed(locale, Error::Storage);
    };
    if !connected {
        return failed(locale, Error::NotConnected);
    }
    if SYNCING.load(Ordering::SeqCst) || locked {
        return failed(locale, Error::Busy);
    }
    let cmd = vec![
        "solstone".to_owned(),
        "journal".to_owned(),
        "importer".to_owned(),
        "--sync".to_owned(),
        "garmin".to_owned(),
        "--save".to_owned(),
    ];
    dispatch_desktop_sync_response(locale, &root, cmd).await
}

pub async fn backfill(locale: &str, root: PathBuf, body: Bytes) -> Response {
    if !GARMIN_AVAILABLE {
        return failed(locale, Error::UnavailableOnMobile);
    }
    let JsonBody::Value(Value::Object(map)) = json_body(body) else {
        return failed(locale, Error::InvalidRange);
    };
    let Some(from_raw) = map.get("date_from").and_then(Value::as_str).map(str::trim) else {
        return failed(locale, Error::InvalidRange);
    };
    let Ok(from_date) = Date::strptime("%Y-%m-%d", from_raw) else {
        return failed(locale, Error::InvalidRange);
    };

    let check_root = root.clone();
    let to_raw = map
        .get("date_to")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    let check = tokio::task::spawn_blocking(move || {
        let zone = solstone_core_journal_config::owner_zone(&check_root);
        let today = Timestamp::now().to_zoned_datetime(zone).date();
        let connected = garmin::key(&check_root).ok().flatten().is_some();
        let locked = garmin::is_sync_locked(&check_root);
        (today, connected, locked)
    })
    .await;
    let Ok((today, connected, locked)) = check else {
        return failed(locale, Error::Storage);
    };
    let to_date = match to_raw.as_deref() {
        Some(raw) => match Date::strptime("%Y-%m-%d", raw) {
            Ok(date) => date,
            Err(_) => return failed(locale, Error::InvalidRange),
        },
        None => today,
    };
    if from_date > to_date || to_date > today {
        return failed(locale, Error::InvalidRange);
    }
    if !connected {
        return failed(locale, Error::NotConnected);
    }
    if SYNCING.load(Ordering::SeqCst) || locked {
        return failed(locale, Error::Busy);
    }

    let cmd = vec![
        "solstone".to_owned(),
        "journal".to_owned(),
        "importer".to_owned(),
        "--sync".to_owned(),
        "garmin".to_owned(),
        "--save".to_owned(),
        "--date-from".to_owned(),
        from_date.strftime("%Y-%m-%d").to_string(),
        "--date-to".to_owned(),
        to_date.strftime("%Y-%m-%d").to_string(),
    ];
    dispatch_desktop_sync_response(locale, &root, cmd).await
}

fn change(
    root: &Path,
    key: Option<&str>,
    expires_on: Option<&str>,
    token_cache: Option<&garmin::TokenCache>,
) -> Result<(), Error> {
    if let Some(key) = key {
        garmin::validate_key(key)?;
    }
    if let Some(expiry) = expires_on {
        garmin::validate_expiry(expiry)?;
    }
    // Removing the key first stops any active sync at its next request before taking the hold.
    if key.is_none() {
        garmin::set_key(root, None, None)?;
    }
    let _hold = garmin::hold(root)?;
    if key.is_none() {
        garmin::set_key(root, None, None)?;
    }
    solstone_core_system::schedule::mutate_schedule_entries(
        &root.join("config/schedules.json"),
        |entries| {
            if key.is_some() {
                entries.insert(
                    "sync:garmin".into(),
                    json!({
                        "cmd": [
                            "solstone",
                            "journal",
                            "importer",
                            "--sync",
                            "garmin",
                            "--save",
                            "--scheduled"
                        ],
                        "every": "hourly"
                    }),
                );
            } else {
                entries.remove("sync:garmin");
            }
            solstone_core_system::schedule::ScheduleMutation {
                changed: true,
                value: (),
            }
        },
    )
    .map_err(|_| Error::Storage)?;
    if let Some(key) = key {
        garmin::set_key(root, Some(key), expires_on)?;
        if let Some(cache) = token_cache {
            let _ = garmin::save_token_cache(root, cache);
        }
    }
    Ok(())
}

struct SyncingGuard;

impl Drop for SyncingGuard {
    fn drop(&mut self) {
        SYNCING.store(false, Ordering::SeqCst);
    }
}

enum DispatchOutcome {
    Started,
    Busy,
    Unavailable,
}

async fn try_dispatch_desktop_sync(root: &Path, cmd: Vec<String>) -> DispatchOutcome {
    let socket = root.join("health").join("callosum.sock");
    let mut connection = CallosumSocketConnection::new(socket, Map::new());
    connection.start();
    let connected = tokio::time::timeout(BUS_CONNECT_TIMEOUT, async {
        loop {
            match connection.next_event().await {
                Some(CallosumReceiveEvent::Continuity {
                    phase: CallosumConnectionPhase::Connected,
                    ..
                }) => return true,
                Some(CallosumReceiveEvent::Continuity {
                    phase:
                        CallosumConnectionPhase::Unavailable { .. }
                        | CallosumConnectionPhase::Stopped { .. },
                    ..
                })
                | None => return false,
                Some(_) => {}
            }
        }
    })
    .await;

    if !matches!(connected, Ok(true)) {
        return DispatchOutcome::Unavailable;
    }
    if SYNCING.swap(true, Ordering::SeqCst) {
        return DispatchOutcome::Busy;
    }
    let guard = SyncingGuard;

    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    let task_ref = format!("garmin-sync-{stamp}");
    let mut fields = Map::new();
    fields.insert("ref".to_owned(), Value::String(task_ref.clone()));
    fields.insert("cmd".to_owned(), json!(cmd));
    fields.insert("queue_if_active_cmd_differs".to_owned(), Value::Bool(true));

    if !connection.emit("supervisor", "request", fields) {
        drop(guard);
        return DispatchOutcome::Unavailable;
    }

    tokio::spawn(async move {
        let _guard = guard;
        let _ = tokio::time::timeout(SYNC_WATCH_MAX_DURATION, async move {
            loop {
                match connection.next_event().await {
                    Some(CallosumReceiveEvent::Envelope { envelope, .. })
                        if envelope.tract == "supervisor"
                            && envelope.event == "stopped"
                            && envelope.extra.get("ref").and_then(Value::as_str)
                                == Some(task_ref.as_str()) =>
                    {
                        break;
                    }
                    Some(CallosumReceiveEvent::Continuity {
                        phase: CallosumConnectionPhase::Stopped { .. },
                        ..
                    })
                    | None => break,
                    _ => {}
                }
            }
        })
        .await;
    });

    DispatchOutcome::Started
}

async fn dispatch_desktop_sync_response(locale: &str, root: &Path, cmd: Vec<String>) -> Response {
    match try_dispatch_desktop_sync(root, cmd).await {
        DispatchOutcome::Started => json_response(json!({ "success": true })),
        DispatchOutcome::Busy => failed(locale, Error::Busy),
        DispatchOutcome::Unavailable => failed(locale, Error::Unavailable),
    }
}

// ---------------------------------------------------------------------------
// Per-run recoverable Trash (/app/settings/api/garmin/runs/{id}*)
// ---------------------------------------------------------------------------

#[derive(Debug, Default, serde::Deserialize)]
struct DeleteRunBody {
    #[serde(default)]
    erase: bool,
    #[serde(default)]
    backups: bool,
}

fn run_not_found(locale: &str) -> Response {
    error_response(
        StatusCode::NOT_FOUND,
        "not_found",
        text(locale, "journal.window.settings.garmin.run.notfound"),
    )
}

fn tree_bytes(path: &Path) -> u64 {
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return 0;
    };
    if !meta.is_dir() {
        return meta.len();
    }
    std::fs::read_dir(path)
        .map(|listing| {
            listing
                .flatten()
                .map(|entry| tree_bytes(&entry.path()))
                .sum()
        })
        .unwrap_or(0)
}

fn imported_at(root: &Path, id: &str) -> Option<String> {
    let dir = root.join("imports").join(id);
    ["manifest.json", "import.json"].iter().find_map(|name| {
        let value: Value = serde_json::from_slice(&std::fs::read(dir.join(name)).ok()?).ok()?;
        ["imported_at", "created_at", "processed_timestamp"]
            .iter()
            .find_map(|key| value.get(*key).and_then(Value::as_str))
            .map(str::to_owned)
    })
}

fn run_preview_json(root: &Path, id: &str, things: &ImportThings) -> Value {
    let bytes: u64 = things
        .paths()
        .iter()
        .map(|path| tree_bytes(&root.join(path)))
        .sum();
    let days = things.days();
    json!({
        "id": id,
        "source": things.source,
        "imported_at": imported_at(root, id),
        "segments": things.segments.len(),
        "days": days.map(|(first, last)| json!([first, last])),
        "shared_days": things.shared_days,
        "bytes": bytes,
        "deletes_here": deletes_here(&things.source),
        "waiting": solstone_core_import::segment_trash::waiting_by_import(root)
            .get(id)
            .copied()
            .unwrap_or(0),
    })
}

pub async fn run_delete_preview(
    locale: &str,
    root: PathBuf,
    AxumPath(id): AxumPath<String>,
) -> Response {
    if !is_import_id(&id) {
        return run_not_found(locale);
    }
    match tokio::task::spawn_blocking(move || {
        import_things(&root, &id)
            .filter(|things| things.source == solstone_core_import::body_trash::GARMIN)
            .map(|things| run_preview_json(&root, &id, &things))
    })
    .await
    {
        Ok(Some(preview)) => json_response(preview),
        Ok(None) => run_not_found(locale),
        Err(_) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "storage",
            text(locale, "journal.window.settings.garmin.run.deletefailed"),
        ),
    }
}

pub async fn run_delete(
    locale: &str,
    root: PathBuf,
    AxumPath(id): AxumPath<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if !is_import_id(&id) {
        return run_not_found(locale);
    }
    let request: DeleteRunBody = if body.iter().all(u8::is_ascii_whitespace) {
        DeleteRunBody::default()
    } else {
        match serde_json::from_slice(&body) {
            Ok(parsed) => parsed,
            Err(_) => {
                return error_response(
                    StatusCode::BAD_REQUEST,
                    "invalid_request",
                    text(locale, "journal.window.settings.garmin.run.invalidrequest"),
                );
            }
        }
    };
    let surface = Surface::of_request(
        Page::Settings,
        headers.get(USER_AGENT).and_then(|v| v.to_str().ok()),
    );
    let outcome = tokio::task::spawn_blocking(move || {
        let Some(things) = import_things(&root, &id)
            .filter(|t| t.source == solstone_core_import::body_trash::GARMIN)
        else {
            return Err(TrashError::Missing("import".to_owned()));
        };
        let _ = things;
        let index = RetentionIndex::new(&root);
        let made = solstone_core_import::made_from::MadeFromIt::new();
        let item =
            match Imports::new(&index, &made).trash(&root, &id, Action::ImportDelete, surface) {
                Ok(item) => item,
                Err(TrashError::Unfinished(item_id)) => solstone_core_trash::item(&root, &item_id)?
                    .ok_or(TrashError::Unfinished(item_id))?,
                Err(error) => return Err(error),
            };
        let erased = request.erase
            && match solstone_core_import::trash_kinds::with_kinds(&root, |kinds| {
                solstone_core_trash::erase_with(
                    &root,
                    kinds,
                    &item.id,
                    Action::ImportDelete,
                    surface,
                    None,
                    request.backups,
                )
            }) {
                Ok(()) => true,
                Err(error) => {
                    log::warn!(
                        "garmin trash item {}: erase now was refused: {error}",
                        item.id
                    );
                    false
                }
            };
        Ok((item, erased, made.refresh_search(&root)))
    })
    .await;

    match outcome {
        Ok(Ok((item, erased, search))) => {
            let mut payload = match (request.erase, erased) {
                (true, true) => json!({ "success": true, "erased": true }),
                (true, false) => json!({
                    "success": true,
                    "erased": false,
                    "erase_failed": true,
                    "trash": { "id": item.id, "erase_at": item.erase_at },
                }),
                (false, _) => json!({
                    "success": true,
                    "trash": { "id": item.id, "erase_at": item.erase_at },
                }),
            };
            payload["search"] = json!(search);
            json_response(payload)
        }
        Ok(Err(TrashError::Missing(_) | TrashError::NotInTrash)) => run_not_found(locale),
        Ok(Err(
            error @ (TrashError::Held(_) | TrashError::InFlight | TrashError::Unfinished(_)),
        )) => {
            let key = failure_key(&error);
            (
                StatusCode::CONFLICT,
                axum::Json(json!({
                    "error": text(locale, "journal.window.settings.garmin.run.deleteheld"),
                    "code": key,
                    "key": key,
                })),
            )
                .into_response()
        }
        Ok(Err(_)) | Err(_) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "storage",
            text(locale, "journal.window.settings.garmin.run.deletefailed"),
        ),
    }
}

// ---------------------------------------------------------------------------
// First-connect Body facet choice (/app/settings/api/garmin/body-facet)
// ---------------------------------------------------------------------------

fn body_facet_copy(locale: &str) -> Value {
    let words: Map<String, Value> = BODY_FACET_COPY
        .iter()
        .map(|(key, id)| {
            let line = if matches!(*key, "ask_existing" | "files_earlier") {
                strings(locale, id, [("facet_name", "{facet}")])
            } else {
                text(locale, id)
            };
            ((*key).to_owned(), Value::String(line))
        })
        .collect();
    Value::Object(words)
}

fn body_facet_offer_value(offer: &BodyFacetAsk) -> Value {
    let (state, facet) = match offer {
        BodyFacetAsk::Chosen(facet) => ("chosen", Some(facet.as_str())),
        BodyFacetAsk::Declined => ("declined", None),
        BodyFacetAsk::AskExisting(facet) => ("ask_existing", Some(facet.as_str())),
        BodyFacetAsk::AskMakeBody => ("ask_make_body", Some(PREFERRED_BODY_FACET)),
        BodyFacetAsk::AskUnmute => ("ask_unmute", Some(PREFERRED_BODY_FACET)),
    };
    json!({ "state": state, "facet": facet })
}

fn body_facet_payload(locale: &str, root: &Path) -> Result<Value, String> {
    let (offer, facets) = solstone_core_facets::body_facet_ask(root)?;
    Ok(json!({
        "offer": body_facet_offer_value(&offer),
        "facets": facets
            .into_iter()
            .map(|(name, title)| json!({ "name": name, "title": title }))
            .collect::<Vec<_>>(),
        "copy": body_facet_copy(locale),
    }))
}

pub async fn body_facet_get(locale: &'static str, root: PathBuf) -> Response {
    match tokio::task::spawn_blocking(move || body_facet_payload(locale, &root)).await {
        Ok(Ok(payload)) => json_response(payload),
        Ok(Err(detail)) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            axum::Json(json!({
                "error": text(locale, "journal.window.import.body.facet.shared.answer.failed"),
                "reason_code": "body_facet_failed",
                "detail": detail,
            })),
        )
            .into_response(),
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            axum::Json(json!({
                "error": text(locale, "journal.window.import.body.facet.shared.answer.failed"),
                "reason_code": "body_facet_failed",
                "detail": error.to_string(),
            })),
        )
            .into_response(),
    }
}

enum AnswerRefused {
    OfferChanged,
    Failed(String),
}

pub async fn body_facet_post(locale: &'static str, root: PathBuf, body: Bytes) -> Response {
    let JsonBody::Value(data) = json_body(body) else {
        return (
            StatusCode::BAD_REQUEST,
            axum::Json(json!({
                "error": text(locale, "journal.window.import.body.facet.shared.answer.failed"),
                "reason_code": "body_facet_failed",
                "detail": "answer must be yes or no",
            })),
        )
            .into_response();
    };
    let yes = match data.get("answer").and_then(Value::as_str) {
        Some("yes") => true,
        Some("no") => false,
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                axum::Json(json!({
                    "error": text(locale, "journal.window.import.body.facet.shared.answer.failed"),
                    "reason_code": "body_facet_failed",
                    "detail": "answer must be yes or no",
                })),
            )
                .into_response();
        }
    };
    let shown = data.get("offer").cloned().unwrap_or(Value::Null);
    let outcome = tokio::task::spawn_blocking(move || {
        let (offer, _) =
            solstone_core_facets::body_facet_ask(&root).map_err(AnswerRefused::Failed)?;
        if shown != body_facet_offer_value(&offer) {
            return Err(AnswerRefused::OfferChanged);
        }
        if let Some(choice) = solstone_core_facets::apply_body_facet_answer(&root, &offer, yes)
            .map_err(AnswerRefused::Failed)?
        {
            solstone_core_journal_config_write::record_body_facet(&root, &choice)
                .map_err(|error| AnswerRefused::Failed(error.to_string()))?;
        }
        body_facet_payload(locale, &root).map_err(AnswerRefused::Failed)
    })
    .await;

    match outcome {
        Ok(Ok(payload)) => json_response(payload),
        Ok(Err(AnswerRefused::OfferChanged)) => (
            StatusCode::CONFLICT,
            axum::Json(json!({
                "error": text(locale, "journal.window.import.body.facet.shared.offer.changed"),
                "reason_code": "body_facet_offer_changed",
                "detail": "the facet question changed since it was shown",
            })),
        )
            .into_response(),
        Ok(Err(AnswerRefused::Failed(detail))) => (
            StatusCode::CONFLICT,
            axum::Json(json!({
                "error": text(locale, "journal.window.import.body.facet.shared.answer.failed"),
                "reason_code": "body_facet_failed",
                "detail": detail,
            })),
        )
            .into_response(),
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            axum::Json(json!({
                "error": text(locale, "journal.window.import.body.facet.shared.answer.failed"),
                "reason_code": "body_facet_failed",
                "detail": error.to_string(),
            })),
        )
            .into_response(),
    }
}

#[cfg(all(test, feature = "full-tests"))]
mod tests {
    use super::*;
    use axum::{
        body::{Body, to_bytes},
        http::Request,
    };
    use tower::ServiceExt;

    #[tokio::test]
    async fn garmin_settings_routes_schedule_mask_key_handle_runs_trash_and_body_facet() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("config")).unwrap();
        std::fs::create_dir_all(root.path().join("imports")).unwrap();
        std::fs::write(root.path().join("config/journal.json"), "{}").unwrap();

        change(
            root.path(),
            Some("GCDC_secret_settings_test_key"),
            Some("2027-02-22"),
            None,
        )
        .unwrap();

        let schedules: Value = serde_json::from_slice(
            &std::fs::read(root.path().join("config/schedules.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            schedules["sync:garmin"]["cmd"],
            json!([
                "solstone",
                "journal",
                "importer",
                "--sync",
                "garmin",
                "--save",
                "--scheduled"
            ])
        );
        assert_eq!(schedules["sync:garmin"]["every"], "hourly");

        // Config projection strips garmin.api_key.
        let cfg = solstone_core_journal_config::read_journal_config(root.path())
            .unwrap()
            .config
            .unwrap();
        let projected = crate::config::project_public_config(cfg).unwrap();
        assert!(
            !serde_json::to_string(&projected)
                .unwrap()
                .contains("GCDC_secret_settings_test_key")
        );

        let router = crate::routes(root.path().to_path_buf());

        // GET /app/settings/api/garmin never exposes key.
        let resp = router
            .clone()
            .oneshot(
                Request::get("/app/settings/api/garmin")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        assert!(!String::from_utf8_lossy(&bytes).contains("GCDC_secret_settings_test_key"));
        let status: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(status["available"], GARMIN_AVAILABLE);
        assert_eq!(status["connected"], true);
        assert_eq!(status["key_expires_on"], "2027-02-22");

        // Invalid key format refused without overwriting existing key.
        let bad = router
            .clone()
            .oneshot(
                Request::post("/app/settings/api/garmin")
                    .body(Body::from(r#"{"api_key":"not-a-garmin-key"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(bad.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            garmin::key(root.path()).unwrap().as_deref(),
            Some("GCDC_secret_settings_test_key")
        );

        // Invalid backfill range refused.
        let bad_range = router
            .clone()
            .oneshot(
                Request::post("/app/settings/api/garmin/backfill")
                    .body(Body::from(
                        r#"{"date_from":"2099-01-01","date_to":"2099-01-02"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(bad_range.status(), StatusCode::BAD_REQUEST);

        // First-connect body-facet offer & answer round-trip + 409 stale offer guard.
        let bf_get = router
            .clone()
            .oneshot(
                Request::get("/app/settings/api/garmin/body-facet")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(bf_get.status(), StatusCode::OK);
        let bf_json: Value =
            serde_json::from_slice(&to_bytes(bf_get.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        assert_eq!(bf_json["offer"]["state"], "ask_make_body");

        // Stale offer -> 409 Conflict.
        let stale = router
            .clone()
            .oneshot(
                Request::post("/app/settings/api/garmin/body-facet")
                    .body(Body::from(
                        r#"{"answer":"yes","offer":{"state":"declined","facet":null}}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(stale.status(), StatusCode::CONFLICT);

        // Valid answer "no" -> declines and remembers.
        let answered = router
            .clone()
            .oneshot(
                Request::post("/app/settings/api/garmin/body-facet")
                    .body(Body::from(
                        json!({"answer": "no", "offer": bf_json["offer"]}).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(answered.status(), StatusCode::OK);
        let answered_json: Value =
            serde_json::from_slice(&to_bytes(answered.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        assert_eq!(answered_json["offer"]["state"], "declined");

        // Seed a Garmin sync run and verify preview + Trash delete scoped strictly to garmin.
        let run_id = "20261124_090000";
        let run_dir = root.path().join("imports").join(run_id);
        std::fs::create_dir_all(&run_dir).unwrap();
        std::fs::write(
            run_dir.join("manifest.json"),
            r#"{"source_type":"garmin","imported_at":"2026-11-24T09:00:00Z","days_affected":["20261124"],"entry_count":1}"#,
        )
        .unwrap();
        std::fs::write(
            run_dir.join("body-touched.jsonl"),
            "{\"day\":\"20261124\",\"windows\":[[\"000000_86400\",1,0,0]]}\n",
        )
        .unwrap();
        let win_dir = root
            .path()
            .join("chronicle/20261124/import.garmin/000000_86400");
        std::fs::create_dir_all(&win_dir).unwrap();
        std::fs::write(
            win_dir.join("body.jsonl"),
            format!(
                "{}\n",
                json!({
                    "id": "g_row_1",
                    "kind": "dated",
                    "date": "2026-11-24",
                    "by": "Garmin",
                    "type": "daily_summary",
                    "label": "daily summary",
                    "v": 1,
                    "source": "garmin",
                    "run": run_id
                })
            ),
        )
        .unwrap();

        // Non-garmin import must return 404 on Garmin run endpoints.
        let other_dir = root.path().join("imports/20261124_080000");
        std::fs::create_dir_all(&other_dir).unwrap();
        std::fs::write(other_dir.join("manifest.json"), r#"{"source_type":"oura"}"#).unwrap();
        let wrong_src = router
            .clone()
            .oneshot(
                Request::get("/app/settings/api/garmin/runs/20261124_080000/delete")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(wrong_src.status(), StatusCode::NOT_FOUND);

        // Preview Garmin run.
        let preview = router
            .clone()
            .oneshot(
                Request::get(format!("/app/settings/api/garmin/runs/{run_id}/delete"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(preview.status(), StatusCode::OK);
        let preview_json: Value =
            serde_json::from_slice(&to_bytes(preview.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        assert_eq!(preview_json["source"], "garmin");
        assert_eq!(preview_json["segments"], 1);
        assert_eq!(preview_json["deletes_here"], true);

        // Trash Garmin run.
        let del = router
            .clone()
            .oneshot(
                Request::delete(format!("/app/settings/api/garmin/runs/{run_id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(del.status(), StatusCode::OK);
        let del_json: Value =
            serde_json::from_slice(&to_bytes(del.into_body(), usize::MAX).await.unwrap()).unwrap();
        assert_eq!(del_json["success"], true);
        assert!(del_json["trash"]["id"].is_string());
        assert!(!run_dir.exists());

        // Disconnect removes key & schedule cleanly.
        let disc = router
            .clone()
            .oneshot(
                Request::delete("/app/settings/api/garmin")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(disc.status(), StatusCode::OK);
        assert_eq!(garmin::key(root.path()).unwrap(), None);
        let schedules_after: Value = serde_json::from_slice(
            &std::fs::read(root.path().join("config/schedules.json")).unwrap(),
        )
        .unwrap();
        assert!(schedules_after.get("sync:garmin").is_none());

        // Manual sync refused when disconnected.
        let sync_refused = router
            .oneshot(
                Request::post("/app/settings/api/garmin/sync")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(sync_refused.status(), StatusCode::BAD_REQUEST);
    }
}
