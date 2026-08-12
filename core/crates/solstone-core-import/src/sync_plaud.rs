// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Plaud catalogue sync with a production HTTPS boundary.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use serde_json::{Map, Value, json};

use crate::ImportError;
use crate::sync_state::{
    FileSyncBackend, FileSyncState, SyncActionSeams, SyncReport, load_sync_state, run_actions,
    write_sync_state,
};

const API_BASE: &str = "https://api.plaud.ai";
const MIN_DURATION_MS: u64 = 30_000;

/// Plaud options preserve the reference's dry-run-by-default shape at the caller boundary.
pub struct PlaudSyncOptions<'a> {
    pub journal: &'a std::path::Path,
    pub save: bool,
    pub access_token: &'a str,
}

pub type PlaudSyncState = FileSyncState;
pub type PlaudFileState = Map<String, Value>;

/// One catalog record returned by Plaud. The access token is intentionally absent.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlaudRemoteFile {
    pub id: String,
    pub filename: String,
    pub fullname: String,
    pub filesize: u64,
    pub start_time: u64,
    pub duration: u64,
    pub is_trash: bool,
}

/// The only Plaud network capability required by catalogue sync.
pub trait PlaudHttp {
    fn list_files(&mut self, access_token: &str) -> Result<Vec<PlaudRemoteFile>, ImportError>;
}

/// Production HTTPS client. Tests inject a [`PlaudHttp`] double instead.
pub struct LivePlaudHttp {
    agent: ureq::Agent,
}

impl LivePlaudHttp {
    #[must_use]
    pub fn new() -> Self {
        let config = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_connect(Some(Duration::from_secs(30)))
            .timeout_recv_response(Some(Duration::from_secs(30)))
            .timeout_recv_body(Some(Duration::from_secs(30)))
            .timeout_global(Some(Duration::from_secs(90)))
            .build();
        Self {
            agent: ureq::Agent::new_with_config(config),
        }
    }
}

impl Default for LivePlaudHttp {
    fn default() -> Self {
        Self::new()
    }
}

impl PlaudHttp for LivePlaudHttp {
    fn list_files(&mut self, access_token: &str) -> Result<Vec<PlaudRemoteFile>, ImportError> {
        let response = self
            .agent
            .get(&format!("{API_BASE}/file/simple/web?skip=0&limit=99999&is_trash=2&sort_by=start_time&is_desc=true"))
            .header("accept", "application/json, text/plain, */*")
            .header("authorization", &format!("bearer {access_token}"))
            .header("app-platform", "web")
            .call()
            .map_err(|_| catalog_error("Plaud catalog request failed"))?;
        if response.status().as_u16() != 200 {
            return Err(catalog_error("Plaud catalog returned a non-success status"));
        }
        let body = response
            .into_body()
            .read_to_string()
            .map_err(|_| catalog_error("Plaud catalog response could not be read"))?;
        let root: Value = serde_json::from_str(&body)
            .map_err(|_| catalog_error("Plaud catalog returned invalid JSON"))?;
        if root.get("status").and_then(Value::as_i64) != Some(0) {
            return Err(catalog_error("Plaud catalog returned an API error"));
        }
        root.get("data_file_list")
            .and_then(Value::as_array)
            .ok_or_else(|| catalog_error("Plaud catalog has no file list"))?
            .iter()
            .map(parse_remote_file)
            .collect()
    }
}

pub fn sync_plaud_with_http<A>(
    options: &PlaudSyncOptions<'_>,
    http: &mut dyn PlaudHttp,
    seams: &mut SyncActionSeams<A>,
) -> Result<SyncReport, ImportError>
where
    A: for<'a> FnMut(
        crate::sync_state::SyncActionRequest<'a>,
    ) -> Result<(), crate::sync_state::SyncActionFailure>,
{
    let remote = http.list_files(options.access_token)?;
    let mut state = load_sync_state(options.journal, FileSyncBackend::Plaud)?
        .unwrap_or_else(|| FileSyncState::empty(FileSyncBackend::Plaud, None));
    let remote_ids: BTreeSet<String> = remote.iter().map(|file| file.id.clone()).collect();
    for file in remote {
        let existing = state.files.get(&file.id).cloned();
        let mut entry = existing
            .and_then(|entry| entry.as_object().cloned())
            .unwrap_or_default();
        entry.insert("filename".to_owned(), Value::String(file.filename));
        entry.insert("fullname".to_owned(), Value::String(file.fullname));
        entry.insert("filesize".to_owned(), json!(file.filesize));
        entry.insert("start_time".to_owned(), json!(file.start_time));
        entry.insert("duration".to_owned(), json!(file.duration));
        entry.insert("is_trash".to_owned(), Value::Bool(file.is_trash));
        if !entry.contains_key("status") {
            if file.is_trash {
                entry.insert("status".to_owned(), Value::String("skipped".to_owned()));
                entry.insert(
                    "skip_reason".to_owned(),
                    Value::String("trashed".to_owned()),
                );
            } else if file.duration > 0 && file.duration < MIN_DURATION_MS {
                entry.insert("status".to_owned(), Value::String("skipped".to_owned()));
                entry.insert(
                    "skip_reason".to_owned(),
                    Value::String("too_short".to_owned()),
                );
            } else {
                entry.insert("status".to_owned(), Value::String("available".to_owned()));
            }
        }
        state.files.insert(file.id, Value::Object(entry));
    }
    state.files.retain(|key, _| remote_ids.contains(key));
    state.stamp();
    write_sync_state(options.journal, &state)?;
    run_actions(
        &mut state,
        options.save,
        options.journal,
        seams,
        &BTreeMap::new(),
    )
}

fn parse_remote_file(value: &Value) -> Result<PlaudRemoteFile, ImportError> {
    let object = value
        .as_object()
        .ok_or_else(|| catalog_error("Plaud catalog contains a non-object file"))?;
    let string = |name: &str| {
        object
            .get(name)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
            .ok_or_else(|| catalog_error("Plaud catalog file is missing an identifier"))
    };
    Ok(PlaudRemoteFile {
        id: string("id")?,
        filename: object
            .get("filename")
            .and_then(Value::as_str)
            .unwrap_or("unnamed")
            .to_owned(),
        fullname: object
            .get("fullname")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned(),
        filesize: object.get("filesize").and_then(Value::as_u64).unwrap_or(0),
        start_time: object
            .get("start_time")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        duration: object.get("duration").and_then(Value::as_u64).unwrap_or(0),
        is_trash: object
            .get("is_trash")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    })
}

fn catalog_error(message: &str) -> ImportError {
    ImportError::SyncCatalog {
        backend: "plaud",
        message: message.to_owned(),
    }
}
