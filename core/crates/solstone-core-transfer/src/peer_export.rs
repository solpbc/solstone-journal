// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Mechanical paired-peer journal export.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use solstone_core_entity::load_all_journal_entities;
use solstone_core_journal_config::read_journal_config;
use solstone_core_journal_io::{PathOrDay, iter_segments};
use solstone_core_transfer_manifest::{ManifestFile, SegmentManifest};

use crate::TransferError;
use crate::peer::resolve_peer;
use crate::peer::{
    MultipartFile, PeerHttpResponse, PeerLoopbackClient, multipart_body, with_peer_bridge,
};
use crate::send::{RESERVED_SEGMENT_FILENAMES, parse_day_spec};

const EXPORT_AREAS: [&str; 5] = ["segments", "imports", "entities", "facets", "config"];
const RETRY_BACKOFF: [Duration; 3] = [
    Duration::from_secs(1),
    Duration::from_secs(5),
    Duration::from_secs(15),
];
const SYNC_STATE_NAMES: [&str; 3] = ["plaud.json", "obsidian.json", "audio.json"];

/// Input to [`peer_export`].
#[derive(Debug, Clone)]
pub struct PeerExportRequest {
    pub to: String,
    pub only: Option<String>,
    pub day: Option<String>,
    pub dry_run: bool,
}

/// Outcome of one independently exported journal area.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerExportAreaResult {
    pub area: String,
    pub sent: u64,
    pub skipped: u64,
    pub staged: u64,
    pub failed: u64,
    pub errors: Vec<String>,
    pub error: Option<String>,
}

impl PeerExportAreaResult {
    fn new(area: &str) -> Self {
        Self {
            area: area.to_string(),
            sent: 0,
            skipped: 0,
            staged: 0,
            failed: 0,
            errors: Vec::new(),
            error: None,
        }
    }
}

/// Complete peer-export outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerExportReport {
    pub results: Vec<PeerExportAreaResult>,
    pub any_failed: bool,
}

/// Export the selected journal areas through one paired-link bridge session.
pub fn peer_export(
    journal: &Path,
    request: PeerExportRequest,
) -> Result<PeerExportReport, TransferError> {
    let areas = parse_only(request.only.as_deref())?;
    let days = parse_day_spec(request.day.as_deref(), journal)?;
    let peer = resolve_peer(journal, &request.to)?;
    let key_prefix: String = peer.instance_id.chars().take(8).collect();
    with_peer_bridge(journal, &peer, |loopback| {
        let mut results = Vec::new();
        for area in EXPORT_AREAS {
            if !areas.contains(area) {
                continue;
            }
            let result = match area {
                "segments" => {
                    export_segments(journal, loopback, &key_prefix, &days, request.dry_run)
                }
                "imports" => export_imports(journal, loopback, &key_prefix, request.dry_run),
                "entities" => export_entities(journal, loopback, &key_prefix, request.dry_run),
                "facets" => export_facets(journal, loopback, &key_prefix, request.dry_run),
                "config" => export_config(journal, loopback, &key_prefix, request.dry_run),
                _ => unreachable!(),
            };
            results.push(area_result(area, result));
        }
        let any_failed = results
            .iter()
            .any(|result| result.error.is_some() || result.failed > 0);
        Ok(PeerExportReport {
            results,
            any_failed,
        })
    })
}

pub(crate) fn area_result(
    area: &str,
    result: Result<PeerExportAreaResult, TransferError>,
) -> PeerExportAreaResult {
    match result {
        Ok(result) => result,
        Err(error) => PeerExportAreaResult {
            area: area.to_string(),
            error: Some(sanitize_diagnostic(&error.to_string())),
            ..PeerExportAreaResult::new(area)
        },
    }
}

fn sanitize_diagnostic(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    for character in input.chars() {
        match character {
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            '\u{1b}' => output.push_str("\\x1b"),
            character if character.is_control() => {
                use std::fmt::Write;
                let _ = write!(output, "\\u{{{:x}}}", character as u32);
            }
            character => output.push(character),
        }
    }
    output
}

fn parse_only(raw: Option<&str>) -> Result<BTreeSet<&'static str>, TransferError> {
    let Some(raw) = raw else {
        return Ok(EXPORT_AREAS.into_iter().collect());
    };
    let areas = raw
        .split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect::<BTreeSet<_>>();
    if areas.is_empty() || areas.iter().any(|area| !EXPORT_AREAS.contains(area)) {
        return Err(TransferError::InvalidExportAreas);
    }
    Ok(areas
        .into_iter()
        .map(|area| {
            EXPORT_AREAS
                .iter()
                .copied()
                .find(|candidate| *candidate == area)
                .expect("validated export area")
        })
        .collect())
}

pub(crate) fn export_segments(
    journal: &Path,
    loopback: &PeerLoopbackClient,
    key_prefix: &str,
    days: &[String],
    dry_run: bool,
) -> Result<PeerExportAreaResult, TransferError> {
    let mut result = PeerExportAreaResult::new("segments");
    let manifest = match query_manifest(loopback, key_prefix, "segments") {
        Ok(manifest) => manifest,
        Err(error) => {
            result.error = Some(error.to_string());
            return Ok(result);
        }
    };
    let by_day = listed_segments_by_day(journal, days)?;
    for (day, listed) in &by_day {
        for segment in listed {
            let identity =
                segment
                    .record_identity()
                    .map_err(|error| TransferError::Unrepresentable {
                        reason: error.to_string(),
                    })?;
            let files = segment_files(segment.path())?;
            if files.is_empty() {
                result.skipped += 1;
                continue;
            }
            let local = SegmentManifest {
                files: files
                    .iter()
                    .map(|file| {
                        let name = file.file_name().and_then(|name| name.to_str()).ok_or(
                            TransferError::Unrepresentable {
                                reason: format!(
                                    "non-UTF-8 file name under {}",
                                    segment.path().display()
                                ),
                            },
                        )?;
                        Ok(ManifestFile {
                            name: name.to_owned(),
                            sha256: hash_file(file)?.0,
                            size: fs::metadata(file)?.len(),
                        })
                    })
                    .collect::<Result<Vec<_>, TransferError>>()?,
            };
            let route = format!("{}/{}", identity.stream, identity.key);
            let remote = manifest
                .get(day)
                .and_then(Value::as_object)
                .and_then(|day_manifest| day_manifest.get(&route))
                .and_then(|value| serde_json::from_value::<SegmentManifest>(value.clone()).ok());
            if segment_manifest_matches(&local, remote.as_ref()) {
                result.skipped += 1;
                continue;
            }
            if dry_run {
                result.sent += 1;
                continue;
            }
            let file_names = files
                .iter()
                .map(|file| {
                    file.file_name()
                        .and_then(|name| name.to_str())
                        .map(str::to_owned)
                        .ok_or_else(|| TransferError::Unrepresentable {
                            reason: format!(
                                "non-UTF-8 file name under {}",
                                segment.path().display()
                            ),
                        })
                })
                .collect::<Result<Vec<_>, TransferError>>()?;
            let metadata = python_compatible_wire_json(
                &json!({"segments": [{"day": day, "stream": identity.stream, "segment_key": identity.key, "files": file_names }]}),
            );
            let multipart_files = read_multipart_files(&files, "files_0")?;
            let (body, boundary) = multipart_body(&metadata, &multipart_files);
            let path = crate::manifest::segment_ingest_path(key_prefix);
            match post_with_retry(
                loopback,
                &path,
                &format!("multipart/form-data; boundary={boundary}"),
                body,
            ) {
                Ok(Some(response)) if response.status == 200 => result.sent += 1,
                Ok(Some(response)) if response.status == 401 => {
                    result.error =
                        Some("Authentication failed: invalid or missing API key".to_string());
                    return Ok(result);
                }
                Ok(Some(response)) if response.status == 403 => {
                    result.error = Some(
                        "Authentication failed: journal source revoked or disabled".to_string(),
                    );
                    return Ok(result);
                }
                _ => result.failed += 1,
            }
        }
    }
    Ok(result)
}

fn listed_segments_by_day(
    journal: &Path,
    days: &[String],
) -> Result<Vec<(String, Vec<solstone_core_journal_io::Segment>)>, TransferError> {
    let mut by_day = Vec::new();
    for day in days {
        by_day.push((day.clone(), iter_segments(journal, PathOrDay::Day(day))?));
    }
    let listed = by_day
        .iter()
        .flat_map(|(_, segments)| segments.iter())
        .collect::<Vec<_>>();
    solstone_core_journal_io::check_record_identities(listed).map_err(|error| {
        TransferError::Unrepresentable {
            reason: error.to_string(),
        }
    })?;
    Ok(by_day)
}

fn export_entities(
    journal: &Path,
    loopback: &PeerLoopbackClient,
    key_prefix: &str,
    dry_run: bool,
) -> Result<PeerExportAreaResult, TransferError> {
    let mut result = PeerExportAreaResult::new("entities");
    let manifest = match query_manifest(loopback, key_prefix, "entities") {
        Ok(manifest) => manifest,
        Err(error) => {
            result.error = Some(error.to_string());
            return Ok(result);
        }
    };
    let received = manifest.get("received").and_then(Value::as_object);
    let (to_send, unchanged) = select_entities(journal, received)?;
    if dry_run {
        result.sent = to_send.len() as u64;
        result.skipped = unchanged;
        return Ok(result);
    }
    if to_send.is_empty() {
        result.skipped = unchanged;
        return Ok(result);
    }
    let path = crate::manifest::entities_ingest_path(key_prefix);
    let body = python_compatible_wire_json(&json!({"entities": to_send})).into_bytes();
    let Some(response) = post_with_retry(loopback, &path, "application/json", body)? else {
        result.error = Some("Entity upload failed after all retries".to_string());
        return Ok(result);
    };
    return_json_result(
        response,
        &mut result,
        "Entity",
        "created",
        Some("auto_merged"),
    )
}

fn export_imports(
    journal: &Path,
    loopback: &PeerLoopbackClient,
    key_prefix: &str,
    dry_run: bool,
) -> Result<PeerExportAreaResult, TransferError> {
    let mut result = PeerExportAreaResult::new("imports");
    let manifest = match query_manifest(loopback, key_prefix, "imports") {
        Ok(manifest) => manifest,
        Err(error) => {
            result.error = Some(error.to_string());
            return Ok(result);
        }
    };
    let received = manifest.get("received").and_then(Value::as_object);
    let mut entries = match fs::read_dir(journal.join("imports")) {
        Ok(entries) => entries.collect::<Result<Vec<_>, _>>()?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(result),
        Err(error) => return Err(error.into()),
    };
    entries.sort_by_key(|entry| entry.file_name());
    let mut to_send = Vec::new();
    let mut unchanged = 0;
    for entry in entries {
        if entry.file_type()?.is_file()
            && SYNC_STATE_NAMES.contains(&entry.file_name().to_string_lossy().as_ref())
        {
            continue;
        }
        let path = entry.path();
        let id = entry.file_name().to_string_lossy().into_owned();
        if !path.is_dir() || !is_import_id(&id) {
            continue;
        }
        let Ok(import_json) = read_json(&path.join("import.json")) else {
            continue;
        };
        let Ok(imported_json) = read_json(&path.join("imported.json")) else {
            continue;
        };
        let content_manifest = match read_jsonl(&path.join("content_manifest.jsonl")) {
            Ok(value) => value,
            Err(_) => continue,
        };
        let hash = sha256(&canonical_hash_json(
            &json!({"import_json": import_json, "imported_json": imported_json, "content_manifest": content_manifest}),
        ));
        if received
            .and_then(|values| values.get(&id))
            .and_then(Value::as_str)
            == Some(hash.as_str())
        {
            unchanged += 1;
        } else {
            to_send.push(json!({"id": id, "import_json": import_json, "imported_json": imported_json, "content_manifest": content_manifest}));
        }
    }
    if dry_run {
        result.sent = to_send.len() as u64;
        result.skipped = unchanged;
        return Ok(result);
    }
    if to_send.is_empty() {
        result.skipped = unchanged;
        return Ok(result);
    }
    let Some(response) = post_with_retry(
        loopback,
        &crate::manifest::imports_ingest_path(key_prefix),
        "application/json",
        python_compatible_wire_json(&json!({"imports": to_send})).into_bytes(),
    )?
    else {
        result.error = Some("Import upload failed after all retries".to_string());
        return Ok(result);
    };
    return_json_result(response, &mut result, "Import", "copied", None)
}

fn export_facets(
    journal: &Path,
    loopback: &PeerLoopbackClient,
    key_prefix: &str,
    dry_run: bool,
) -> Result<PeerExportAreaResult, TransferError> {
    let mut result = PeerExportAreaResult::new("facets");
    let manifest = match query_manifest(loopback, key_prefix, "facets") {
        Ok(manifest) => manifest,
        Err(error) => {
            result.error = Some(error.to_string());
            return Ok(result);
        }
    };
    let received = manifest.get("received").and_then(Value::as_object);
    let facets = journal.join("facets");
    let mut directories = match fs::read_dir(facets) {
        Ok(entries) => entries.collect::<Result<Vec<_>, _>>()?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(result),
        Err(error) => return Err(error.into()),
    };
    directories.sort_by_key(|entry| entry.file_name());
    for directory in directories {
        let facet = directory.file_name().to_string_lossy().into_owned();
        let root = directory.path();
        if !root.is_dir() || !is_facet_name(&facet) || !root.join("facet.json").is_file() {
            continue;
        }
        let mut files = Vec::new();
        collect_files(&root, &root, &mut files)?;
        files.sort_by_key(|(_, relative, _)| relative.clone());
        let changed = changed_facet_files(files, received, &facet)?;
        if changed.is_empty() {
            result.skipped += 1;
            continue;
        }
        if dry_run {
            result.sent += 1;
            continue;
        }
        let metadata = python_compatible_wire_json(
            &json!({"facets": [{"name": facet, "files": changed.iter().map(|(_, path, kind)| json!({"path": path, "type": kind})).collect::<Vec<_>>() }]}),
        );
        let multipart_files = changed
            .iter()
            .enumerate()
            .map(|(index, (path, _, _))| {
                Ok(MultipartFile {
                    field_name: format!("files_0_{index}"),
                    file_name: path
                        .file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or_default()
                        .to_string(),
                    contents: fs::read(path)?,
                })
            })
            .collect::<Result<Vec<_>, std::io::Error>>()?;
        let (body, boundary) = multipart_body(&metadata, &multipart_files);
        match post_with_retry(
            loopback,
            &crate::manifest::facets_ingest_path(key_prefix),
            &format!("multipart/form-data; boundary={boundary}"),
            body,
        ) {
            Ok(Some(response)) if response.status == 200 => {
                let value: Value = serde_json::from_slice(&response.body).unwrap_or(Value::Null);
                result.errors.extend(string_values(value.get("errors")));
                result.sent += 1;
            }
            Ok(Some(response)) if response.status == 401 => {
                result.error =
                    Some("Authentication failed: invalid or missing API key".to_string());
                return Ok(result);
            }
            Ok(Some(response)) if response.status == 403 => {
                result.error =
                    Some("Authentication failed: journal source revoked or disabled".to_string());
                return Ok(result);
            }
            _ => result.failed += 1,
        }
    }
    Ok(result)
}

fn export_config(
    journal: &Path,
    loopback: &PeerLoopbackClient,
    key_prefix: &str,
    dry_run: bool,
) -> Result<PeerExportAreaResult, TransferError> {
    let mut result = PeerExportAreaResult::new("config");
    let manifest = match query_manifest(loopback, key_prefix, "config") {
        Ok(manifest) => manifest,
        Err(error) => {
            result.error = Some(error.to_string());
            return Ok(result);
        }
    };
    let config = read_journal_config(journal)
        .map_err(|error| TransferError::Manifest(error.to_string()))?
        .config
        .unwrap_or_default();
    let mut config = Value::Object(config);
    strip_never_transfer(&mut config);
    if manifest.get("last_hash").and_then(Value::as_str)
        == Some(sha256(&canonical_hash_json(&config)).as_str())
    {
        result.skipped = 1;
        return Ok(result);
    }
    if dry_run {
        result.staged = 1;
        return Ok(result);
    }
    let Some(response) = post_with_retry(
        loopback,
        &crate::manifest::config_ingest_path(key_prefix),
        "application/json",
        python_compatible_wire_json(&json!({"config": config})).into_bytes(),
    )?
    else {
        result.error = Some("Config upload failed after all retries".to_string());
        return Ok(result);
    };
    if response.status == 401 {
        result.error = Some("Authentication failed: invalid or missing API key".to_string());
        return Ok(result);
    }
    if response.status == 403 {
        result.error =
            Some("Authentication failed: journal source revoked or disabled".to_string());
        return Ok(result);
    }
    if response.status != 200 {
        result.error = Some(format!(
            "Config upload failed: {} {}",
            response.status,
            String::from_utf8_lossy(&response.body)
        ));
        return Ok(result);
    }
    let value: Value = serde_json::from_slice(&response.body).unwrap_or(Value::Null);
    result.staged = u64::from(
        value
            .get("staged")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    );
    result.skipped = u64::from(
        value
            .get("skipped")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    );
    Ok(result)
}

fn query_manifest(
    loopback: &PeerLoopbackClient,
    key_prefix: &str,
    area: &str,
) -> Result<Value, TransferError> {
    let response = loopback.get(&crate::manifest::manifest_path(key_prefix, area))?;
    match response.status {
        200 => serde_json::from_slice(&response.body)
            .map_err(|error| TransferError::ManifestQuery(error.to_string())),
        401 => Err(TransferError::ManifestQuery(
            "Authentication failed: invalid or missing API key".to_string(),
        )),
        403 => Err(TransferError::ManifestQuery(
            "Authentication failed: journal source revoked or disabled".to_string(),
        )),
        _ => Err(TransferError::ManifestQuery(format!(
            "Manifest query failed: {} {}",
            response.status,
            String::from_utf8_lossy(&response.body)
        ))),
    }
}

fn post_with_retry(
    loopback: &PeerLoopbackClient,
    path: &str,
    content_type: &str,
    body: Vec<u8>,
) -> Result<Option<PeerHttpResponse>, TransferError> {
    for (attempt, delay) in RETRY_BACKOFF.iter().enumerate() {
        match loopback.post(path, content_type, body.clone()) {
            Ok(response) if !(500..=599).contains(&response.status) => {
                return Ok(Some(response));
            }
            Ok(_) | Err(_) if attempt + 1 < RETRY_BACKOFF.len() => {
                std::thread::sleep(*delay);
            }
            Ok(_) | Err(_) => {}
        }
    }
    Ok(None)
}

fn return_json_result(
    response: PeerHttpResponse,
    result: &mut PeerExportAreaResult,
    noun: &str,
    sent: &str,
    merged: Option<&str>,
) -> Result<PeerExportAreaResult, TransferError> {
    if response.status == 401 {
        result.error = Some("Authentication failed: invalid or missing API key".to_string());
        return Ok(result.clone());
    }
    if response.status == 403 {
        result.error =
            Some("Authentication failed: journal source revoked or disabled".to_string());
        return Ok(result.clone());
    }
    if response.status != 200 {
        result.error = Some(format!(
            "{noun} upload failed: {} {}",
            response.status,
            String::from_utf8_lossy(&response.body)
        ));
        return Ok(result.clone());
    }
    let value: Value = serde_json::from_slice(&response.body)
        .map_err(|error| TransferError::Manifest(error.to_string()))?;
    result.sent = value.get(sent).and_then(Value::as_u64).unwrap_or(0)
        + merged
            .and_then(|key| value.get(key))
            .and_then(Value::as_u64)
            .unwrap_or(0);
    result.staged = value.get("staged").and_then(Value::as_u64).unwrap_or(0);
    result.skipped = value.get("skipped").and_then(Value::as_u64).unwrap_or(0);
    result.errors = string_values(value.get("errors"));
    Ok(result.clone())
}

fn read_multipart_files(
    paths: &[PathBuf],
    field_name: &str,
) -> Result<Vec<MultipartFile>, TransferError> {
    paths
        .iter()
        .map(|path| {
            Ok(MultipartFile {
                field_name: field_name.to_string(),
                file_name: path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or_default()
                    .to_string(),
                contents: fs::read(path)?,
            })
        })
        .collect()
}
fn segment_files(segment: &Path) -> Result<Vec<PathBuf>, TransferError> {
    let mut files = fs::read_dir(segment)?
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .filter_map(|entry| {
            let path = entry.path();
            (path.is_file()
                && !RESERVED_SEGMENT_FILENAMES
                    .contains(&entry.file_name().to_string_lossy().as_ref()))
            .then_some(path)
        })
        .collect::<Vec<_>>();
    files.sort();
    Ok(files)
}

fn hash_file(path: &Path) -> Result<(String, u64), TransferError> {
    let mut source = fs::File::open(path)?;
    let mut digest = Sha256::new();
    let mut size = 0_u64;
    let mut buffer = [0_u8; 65_536];
    loop {
        let read = source.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
        size += read as u64;
    }
    Ok((format!("{:x}", digest.finalize()), size))
}

fn segment_manifest_matches(local: &SegmentManifest, remote: Option<&SegmentManifest>) -> bool {
    local
        .files
        .iter()
        .map(|file| (&file.name, &file.sha256))
        .collect::<BTreeMap<_, _>>()
        == remote
            .map(|manifest| {
                manifest
                    .files
                    .iter()
                    .map(|file| (&file.name, &file.sha256))
                    .collect()
            })
            .unwrap_or_default()
}
fn is_facet_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    matches!(bytes.next(), Some(byte) if byte.is_ascii_lowercase() || byte.is_ascii_digit())
        && bytes.all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'-')
        })
}
fn is_import_id(name: &str) -> bool {
    let bytes = name.as_bytes();
    bytes.len() == 15
        && bytes[8] == b'_'
        && bytes
            .iter()
            .enumerate()
            .all(|(index, byte)| index == 8 || byte.is_ascii_digit())
}
fn classify_facet_file(relative: &str) -> Option<&'static str> {
    let parts = relative.split('/').collect::<Vec<_>>();
    match parts.as_slice() {
        ["facet.json"] => Some("facet_json"),
        ["entities", _, "entity.json"] => Some("entity_relationship"),
        ["entities", _, "observations.jsonl"] => Some("entity_observations"),
        ["entities", day] if is_day_jsonl(day) => Some("detected_entities"),
        ["activities", "activities.jsonl"] => Some("activity_config"),
        ["activities", day] if is_day_jsonl(day) => Some("activity_records"),
        ["activities", day, _, _, ..] if is_day(day) => Some("activity_output"),
        ["news", day] if is_day_md(day) => Some("news"),
        ["logs", day] if is_day_jsonl(day) => Some("logs"),
        _ => None,
    }
}
fn collect_files(
    root: &Path,
    directory: &Path,
    out: &mut Vec<(PathBuf, String, String)>,
) -> Result<(), TransferError> {
    let mut entries = fs::read_dir(directory)?.collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(|entry| entry.path());
    for entry in entries {
        let path = entry.path();
        if path.is_dir() {
            collect_files(root, &path, out)?;
        } else if path.is_file() {
            let relative = path
                .strip_prefix(root)
                .expect("facet descendant")
                .to_string_lossy()
                .replace('\\', "/");
            if let Some(kind) = classify_facet_file(&relative) {
                out.push((path, relative, kind.to_string()));
            }
        }
    }
    Ok(())
}

fn changed_facet_files(
    files: Vec<(PathBuf, String, String)>,
    received: Option<&Map<String, Value>>,
    facet: &str,
) -> Result<Vec<(PathBuf, String, String)>, TransferError> {
    let mut new_files = Vec::new();
    let mut changed_files = Vec::new();
    for (path, relative, kind) in files {
        let hash = hash_file(&path)?.0;
        match received
            .and_then(|values| values.get(&format!("{facet}/{relative}")))
            .and_then(Value::as_str)
        {
            Some(remote_hash) if remote_hash == hash => {}
            Some(_) => changed_files.push((path, relative, kind)),
            None => new_files.push((path, relative, kind)),
        }
    }
    new_files.extend(changed_files);
    Ok(new_files)
}
fn is_day(value: &str) -> bool {
    value.len() == 8 && value.bytes().all(|byte| byte.is_ascii_digit())
}
fn is_day_jsonl(value: &str) -> bool {
    value.strip_suffix(".jsonl").is_some_and(is_day)
}
fn is_day_md(value: &str) -> bool {
    value.strip_suffix(".md").is_some_and(is_day)
}
fn read_json(path: &Path) -> Result<Value, TransferError> {
    serde_json::from_slice(&fs::read(path)?)
        .map_err(|error| TransferError::Manifest(error.to_string()))
}
fn read_jsonl(path: &Path) -> Result<Vec<Value>, TransferError> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    fs::read_to_string(path)?
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|line| {
            serde_json::from_str(line).map_err(|error| TransferError::Manifest(error.to_string()))
        })
        .collect()
}
fn select_entities(
    journal: &Path,
    received: Option<&Map<String, Value>>,
) -> Result<(Vec<Value>, u64), TransferError> {
    let mut to_send = Vec::new();
    let mut unchanged = 0;
    for entity in load_all_journal_entities(journal)
        .map_err(|error| TransferError::Manifest(error.to_string()))?
    {
        if entity.is_blocked() {
            continue;
        }
        let hash = sha256(&canonical_hash_json(&entity.value));
        if received
            .and_then(|values| values.get(&entity.id))
            .and_then(Value::as_str)
            == Some(hash.as_str())
        {
            unchanged += 1;
        } else {
            to_send.push(entity.value);
        }
    }
    Ok((to_send, unchanged))
}
fn string_values(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|value| {
            value
                .as_str()
                .map_or_else(|| value.to_string(), ToString::to_string)
        })
        .collect()
}
fn sha256(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))
}
fn strip_never_transfer(config: &mut Value) {
    for path in [
        "env.OPENAI_API_KEY",
        "env.ANTHROPIC_API_KEY",
        "env.GOOGLE_API_KEY",
        "env.PLAUD_ACCESS_TOKEN",
        "convey.password_hash",
        "convey.secret",
        "backup.destination.credentials",
        "backup.daily_key",
        "backup.recovery_key",
        "voice.openai_api_key",
        "pairing.home_address",
    ] {
        remove_path(config, &path.split('.').collect::<Vec<_>>());
    }
}

fn remove_path(value: &mut Value, parts: &[&str]) {
    let Some((first, rest)) = parts.split_first() else {
        return;
    };
    let Some(object) = value.as_object_mut() else {
        return;
    };
    if rest.is_empty() {
        object.remove(*first);
    } else if let Some(next) = object.get_mut(*first) {
        remove_path(next, rest);
    }
}

/// Match `json.dumps(value, sort_keys=True, ensure_ascii=False)` byte-for-byte.
fn canonical_hash_json(value: &Value) -> String {
    let mut output = String::new();
    write_json(value, true, false, &mut output);
    output
}
/// Match Requests' `json=` encoder: insertion order, spaces, and ASCII escaping.
fn python_compatible_wire_json(value: &Value) -> String {
    let mut output = String::new();
    write_json(value, false, true, &mut output);
    output
}
fn write_json(value: &Value, sort_keys: bool, ensure_ascii: bool, output: &mut String) {
    match value {
        Value::Null => output.push_str("null"),
        Value::Bool(value) => output.push_str(if *value { "true" } else { "false" }),
        Value::Number(value) => output.push_str(&value.to_string()),
        Value::String(value) => write_string(value, ensure_ascii, output),
        Value::Array(values) => {
            output.push('[');
            for (index, value) in values.iter().enumerate() {
                if index > 0 {
                    output.push_str(", ");
                }
                write_json(value, sort_keys, ensure_ascii, output);
            }
            output.push(']');
        }
        Value::Object(values) => {
            output.push('{');
            let mut entries = values.iter().collect::<Vec<_>>();
            if sort_keys {
                entries.sort_by_key(|(key, _)| *key);
            }
            for (index, (key, value)) in entries.into_iter().enumerate() {
                if index > 0 {
                    output.push_str(", ");
                }
                write_string(key, ensure_ascii, output);
                output.push_str(": ");
                write_json(value, sort_keys, ensure_ascii, output);
            }
            output.push('}');
        }
    }
}
fn write_string(value: &str, ensure_ascii: bool, output: &mut String) {
    output.push('"');
    for character in value.chars() {
        match character {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\u{08}' => output.push_str("\\b"),
            '\u{0c}' => output.push_str("\\f"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            character if character <= '\u{1f}' => {
                use std::fmt::Write;
                write!(output, "\\u{:04x}", character as u32).expect("string write");
            }
            character if ensure_ascii && !character.is_ascii() => {
                let mut units = [0; 2];
                for unit in character.encode_utf16(&mut units) {
                    use std::fmt::Write;
                    write!(output, "\\u{unit:04x}").expect("string write");
                }
            }
            character => output.push(character),
        }
    }
    output.push('"');
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "linux")]
    #[test]
    fn later_day_identity_failure_refuses_the_whole_days_list() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;

        let journal = tempfile::tempdir().unwrap();
        let early = journal.path().join("chronicle/20260101/120000_60");
        fs::create_dir_all(&early).unwrap();
        fs::write(early.join("audio.jsonl"), "{}\n").unwrap();
        let late = journal
            .path()
            .join("chronicle/20260102")
            .join(OsStr::from_bytes(b"s\xff"))
            .join("120000_60");
        fs::create_dir_all(&late).unwrap();
        fs::write(late.join("audio.jsonl"), "{}\n").unwrap();

        let error = listed_segments_by_day(
            journal.path(),
            &["20260101".to_owned(), "20260102".to_owned()],
        )
        .unwrap_err();
        match error {
            TransferError::Unrepresentable { reason } => {
                assert!(reason.contains("not UTF-8 representable"), "{reason}");
            }
            other => panic!("expected Unrepresentable, got {other:?}"),
        }
    }

    fn named_default_area_result(journal: &std::path::Path) -> PeerExportAreaResult {
        let error = listed_segments_by_day(journal, &["20260101".to_owned()])
            .expect_err("named _default must refuse the days list");
        area_result("segments", Err(error))
    }

    #[test]
    fn named_default_identity_failure_surfaces_through_area_result() {
        let journal = tempfile::tempdir().unwrap();
        let direct = journal.path().join("chronicle/20260101/080000_60");
        fs::create_dir_all(&direct).unwrap();
        fs::write(direct.join("audio.jsonl"), "{}\n").unwrap();
        let named = journal.path().join("chronicle/20260101/_default/090000_60");
        fs::create_dir_all(&named).unwrap();
        fs::write(named.join("audio.jsonl"), "{}\n").unwrap();

        let result = named_default_area_result(journal.path());
        let error = result.error.expect("area error");
        assert!(
            error.contains(
                "named stream directory \"_default\" cannot be spelled as a record identity"
            ),
            "{error}"
        );
        assert!(!error.contains("not UTF-8 representable"), "{error}");
        assert!(
            !error.contains("Exception during segments export"),
            "{error}"
        );
        assert_eq!(result.sent, 0);
        assert_eq!(result.failed, 0);
    }

    #[cfg(unix)]
    #[test]
    fn named_default_identity_failure_escapes_control_characters_via_listed_days() {
        let parent = tempfile::tempdir().unwrap();
        let journal = parent.path().join("journal\nroot");
        let named = journal.join("chronicle/20260101/_default/090000_60");
        fs::create_dir_all(&named).unwrap();
        fs::write(named.join("audio.jsonl"), "{}\n").unwrap();

        let result = named_default_area_result(&journal);
        let error = result.error.expect("area error");
        assert!(
            !error.contains('\n'),
            "raw newline leaked into transfer diagnostic: {error:?}"
        );
        assert!(error.contains("\\n"), "{error}");
        assert!(
            error.contains(
                "named stream directory \"_default\" cannot be spelled as a record identity"
            ),
            "{error}"
        );
        assert_eq!(result.sent, 0);
        assert_eq!(result.failed, 0);
    }

    #[test]
    fn serializers_match_python_json_shapes() {
        let value = json!({"z": "café😀", "a": [true, {"b": "\n", "a": 1}]});
        assert_eq!(
            canonical_hash_json(&value),
            "{\"a\": [true, {\"a\": 1, \"b\": \"\\n\"}], \"z\": \"café😀\"}"
        );
        assert_eq!(
            python_compatible_wire_json(&value),
            "{\"z\": \"caf\\u00e9\\ud83d\\ude00\", \"a\": [true, {\"b\": \"\\n\", \"a\": 1}]}"
        );
    }

    #[test]
    fn classifies_only_python_export_facet_paths() {
        assert_eq!(classify_facet_file("facet.json"), Some("facet_json"));
        assert_eq!(
            classify_facet_file("entities/20260203.jsonl"),
            Some("detected_entities")
        );
        assert_eq!(
            classify_facet_file("activities/20260203/output/a.json"),
            Some("activity_output")
        );
        assert_eq!(classify_facet_file("other/file.txt"), None);
        assert_eq!(classify_facet_file("todos/20260203.jsonl"), None);
    }

    #[test]
    fn facet_export_omits_retired_files_without_touching_source_bytes() {
        let facet = tempfile::tempdir().unwrap();
        let logs = facet.path().join("logs/20260203.jsonl");
        let retired = facet.path().join("todos/20260203.jsonl");
        fs::create_dir_all(logs.parent().unwrap()).unwrap();
        fs::create_dir_all(retired.parent().unwrap()).unwrap();
        fs::write(&logs, b"{\"message\":\"included\"}\n").unwrap();
        fs::write(&retired, b"{\"text\":\"untouched\"}\n").unwrap();
        let before = fs::read(&retired).unwrap();

        let mut files = Vec::new();
        collect_files(facet.path(), facet.path(), &mut files).unwrap();

        assert_eq!(files.len(), 1);
        assert_eq!(files[0].1, "logs/20260203.jsonl");
        assert_eq!(files[0].2, "logs");
        assert_eq!(fs::read(&retired).unwrap(), before);
    }

    #[test]
    fn classification_gates_reject_invalid_input_shapes() {
        assert!(!is_facet_name("Work"));
        assert!(!is_facet_name("work space"));

        assert_eq!(classify_facet_file("entities/2026020.jsonl"), None);
        assert_eq!(classify_facet_file("entities/20260203.txt"), None);
        assert!(!is_day_jsonl("2026020.jsonl"));
        assert!(!is_day_md("20260203.txt"));

        for import_id in [
            "20260203_12000",
            "20260203_1200000",
            "202602031120000",
            "2026020a_120000",
        ] {
            assert!(!is_import_id(import_id), "{import_id}");
        }

        assert_eq!(classify_facet_file("random/nested/file.bin"), None);
    }

    #[test]
    fn filters_secret_paths_structurally() {
        let mut value = json!({
            "env": {
                "OPENAI_API_KEY": "secret",
                "ANTHROPIC_API_KEY": "secret",
                "GOOGLE_API_KEY": "secret",
                "PLAUD_ACCESS_TOKEN": "secret",
                "other_field": "keep"
            },
            "convey": {
                "password_hash": "secret",
                "secret": "token",
                "bind": "127.0.0.1",
                "other_field": "keep"
            },
            "backup": {
                "destination": {"credentials": {"token": "secret"}, "url": "keep"},
                "daily_key": "secret",
                "recovery_key": "secret"
            },
            "voice": {"openai_api_key": "secret", "provider": "keep"},
            "pairing": {"home_address": "secret", "label": "keep"},
            "identity": {"name": "Keep"}
        });
        strip_never_transfer(&mut value);
        assert_eq!(
            value,
            json!({
                "env": {"other_field": "keep"},
                "convey": {"bind": "127.0.0.1", "other_field": "keep"},
                "backup": {"destination": {"url": "keep"}},
                "voice": {"provider": "keep"},
                "pairing": {"label": "keep"},
                "identity": {"name": "Keep"}
            })
        );
    }

    #[test]
    fn entity_selection_omits_blocked_identities() {
        let journal = tempfile::tempdir().expect("journal");
        for (id, value) in [
            ("keep", json!({"id": "keep", "name": "Kept"})),
            ("blocked", json!({"id": "blocked", "blocked": true})),
        ] {
            let directory = journal.path().join("entities").join(id);
            fs::create_dir_all(&directory).expect("entity directory");
            fs::write(directory.join("entity.json"), value.to_string()).expect("entity");
        }
        let (entities, unchanged) = select_entities(journal.path(), None).expect("select");
        assert_eq!(unchanged, 0);
        assert_eq!(entities, vec![json!({"id": "keep", "name": "Kept"})]);
    }

    #[test]
    fn facet_walker_keeps_only_classified_paths_in_lexical_order() {
        let root = tempfile::tempdir().expect("facet");
        for (relative, bytes) in [
            ("facet.json", b"{}".as_slice()),
            ("logs/20260203.jsonl", b"log".as_slice()),
            ("private.txt", b"private".as_slice()),
        ] {
            let path = root.path().join(relative);
            fs::create_dir_all(path.parent().expect("parent")).expect("parent directory");
            fs::write(path, bytes).expect("file");
        }
        let mut files = Vec::new();
        collect_files(root.path(), root.path(), &mut files).expect("collect");
        files.sort_by_key(|(_, relative, _)| relative.clone());
        assert_eq!(
            files
                .into_iter()
                .map(|(_, relative, kind)| (relative, kind))
                .collect::<Vec<_>>(),
            vec![
                ("facet.json".to_string(), "facet_json".to_string()),
                ("logs/20260203.jsonl".to_string(), "logs".to_string())
            ]
        );
    }

    #[test]
    fn facet_upload_orders_new_files_before_changed_files() {
        let root = tempfile::tempdir().expect("facet");
        let changed = root.path().join("a.json");
        let new = root.path().join("z.json");
        fs::write(&changed, b"changed").expect("changed file");
        fs::write(&new, b"new").expect("new file");
        let mut received = Map::new();
        received.insert(
            "work/a.json".to_string(),
            Value::String("old hash".to_string()),
        );
        let files = vec![
            (changed, "a.json".to_string(), "facet_json".to_string()),
            (new, "z.json".to_string(), "facet_json".to_string()),
        ];
        let ordered = changed_facet_files(files, Some(&received), "work").expect("order");
        assert_eq!(
            ordered
                .into_iter()
                .map(|(_, path, _)| path)
                .collect::<Vec<_>>(),
            ["z.json", "a.json"]
        );
    }

    #[test]
    fn entity_and_import_rejection_messages_match_python() {
        for (noun, expected) in [
            ("Entity", "Entity upload failed: 418 nope"),
            ("Import", "Import upload failed: 418 nope"),
        ] {
            let mut result = PeerExportAreaResult::new("area");
            let returned = return_json_result(
                PeerHttpResponse {
                    status: 418,
                    body: b"nope".to_vec(),
                },
                &mut result,
                noun,
                "created",
                None,
            )
            .expect("result");
            assert_eq!(returned.error.as_deref(), Some(expected));
        }
    }

    #[test]
    fn only_parser_requires_known_nonempty_area_set() {
        assert!(parse_only(Some("segments, config")).is_ok());
        assert!(matches!(
            parse_only(Some("")),
            Err(TransferError::InvalidExportAreas)
        ));
        assert!(matches!(
            parse_only(Some("nope")),
            Err(TransferError::InvalidExportAreas)
        ));
    }
    #[test]
    fn segment_export_keeps_artifacts_but_excludes_local_publication_records() {
        let root = tempfile::tempdir().unwrap();
        for name in [
            "screen.jsonl",
            "timeline.json",
            "timeline.state.json",
            "stream.json",
            "ingest.json",
            "ingest.json.lock",
        ] {
            std::fs::write(root.path().join(name), b"fixture").unwrap();
        }
        let files = segment_files(root.path()).unwrap();
        assert_eq!(
            files,
            [
                root.path().join("screen.jsonl"),
                root.path().join("timeline.json")
            ]
        );
        assert_eq!(
            std::fs::read(root.path().join("timeline.state.json")).unwrap(),
            b"fixture"
        );
    }
}
