// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Authenticated MCP `resources/*` templates, URI routing, and media reads.
//!
//! Every content-bearing `resources/read` traverses the same grant evaluation,
//! kind/option gating, facet scope checks, source-release barriers, post-read
//! permission-generation check, and fail-closed `solstone-core-mcp-audit`
//! admission + outcome publication as `tools/call`.

use std::collections::BTreeSet;
use std::path::Path;

use jiff::Timestamp;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use solstone_core_indexer_query::AdmittedCategory;
use solstone_core_journal_io::paths::{PathOrDay, iter_segments_skipping_unreadable_streams};
use solstone_core_mcp_audit::{Admission, AuditCoordinates, Outcome, ToolName, result_shape};
use solstone_core_observe_audio::{MAX_CLIP_DURATION_MS, MIN_CLIP_DURATION_MS};

use crate::audit::{self, Reason};
use crate::dispatch::{
    DispatchError, DispatchPrincipal, audit_permission_snapshot, boundary, bounded_photo, codec,
    content_digest, kind_reason, live_allows, owner_denial_reason,
    permission_generation_is_current, read_scoped_entities, record_outcome, reference_fingerprint,
};
use crate::jsonrpc::ResourceLink;
use crate::permissions::{ConnectionReadSnapshot, PermissionDecision, evaluate_connection_read};
use crate::photo_picture::PictureSize;
use crate::references::{ReferenceKind, ReferenceTarget, SegmentAddress};
use crate::tools::transcripts::{
    AudioClipRequest, AudioEncoding, DEFAULT_AUDIO_DURATION_MS, MAX_AUDIO_START_MS,
};
use crate::tools::{MAX_OPAQUE_REFERENCE_BYTES, ToolError, execute_after_audit};

const URI_PREFIX: &str = "solstone://media/";
pub(crate) const MAX_RESOURCE_URI_BYTES: usize = 4096;
const MAX_MEDIA_SELECTOR_BYTES: usize = 128;

/// Which kind of imported picture an entry reference names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EntryPictureKind {
    Photo,
    Screenshot,
}

/// Validated resource target parsed from a `solstone://media/...` URI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ParsedResourceUri {
    Entry {
        reference: String,
        size: PictureSize,
    },
    Entity {
        reference: String,
    },
    Screen {
        reference: String,
        offset_ms: u32,
        display: Option<String>,
    },
    Audio {
        reference: String,
        start_ms: u64,
        duration_ms: u64,
        track: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ParseResourceError {
    InvalidParams(String),
    UnknownRoute,
}

/// Approved binary resource bytes ready for MCP `resources/read` delivery.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResourceOutput {
    pub(crate) uri: String,
    pub(crate) mime_type: &'static str,
    pub(crate) bytes: Vec<u8>,
}

/// Failure modes for an authenticated `resources/read`.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ResourceError {
    InvalidParams(String),
    PermissionDenied(&'static str),
    KindDenied(AdmittedCategory),
    NotFound {
        reason: &'static str,
        message: String,
    },
    Failed {
        reason: &'static str,
        message: String,
    },
    AuditUnavailable,
}

struct LoadedResource {
    mime_type: &'static str,
    bytes: Vec<u8>,
    target: String,
}

/// Validate `resources/list` and `resources/templates/list` params.
///
/// Accepts omitted/null params, optional `_meta` object, and `cursor: null`.
/// Rejects any non-null cursor (this endpoint issues no resource list cursors)
/// and any unknown parameter key.
pub(crate) fn validate_list_params(params: Option<&Value>) -> Result<(), &'static str> {
    let Some(value) = params else {
        return Ok(());
    };
    if value.is_null() {
        return Ok(());
    }
    let Some(object) = value.as_object() else {
        return Err("invalid params");
    };
    for (key, val) in object {
        match key.as_str() {
            "_meta" => {
                if !val.is_null() && !val.is_object() {
                    return Err("invalid params");
                }
            }
            "cursor" => {
                if !val.is_null() {
                    return Err("invalid cursor");
                }
            }
            _ => return Err("invalid params"),
        }
    }
    Ok(())
}

/// Validate `resources/read` params and return the requested URI string.
pub(crate) fn extract_read_uri(params: Option<&Value>) -> Result<&str, &'static str> {
    let Some(object) = params.and_then(Value::as_object) else {
        return Err("invalid params");
    };
    let mut uri: Option<&str> = None;
    for (key, val) in object {
        match key.as_str() {
            "uri" => {
                let Some(text) = val.as_str() else {
                    return Err("invalid params");
                };
                if text.is_empty() || text.len() > MAX_RESOURCE_URI_BYTES {
                    return Err("invalid params");
                }
                uri = Some(text);
            }
            "_meta" => {
                if !val.is_null() && !val.is_object() {
                    return Err("invalid params");
                }
            }
            _ => return Err("invalid params"),
        }
    }
    uri.ok_or("invalid params")
}

/// Build the permission-filtered `resources/templates/list` payload for this
/// connection. Only templates whose required media grants are active are shown.
pub(crate) fn advertised_resource_templates(
    journal_root: &Path,
    decision: &PermissionDecision,
) -> Value {
    let PermissionDecision::Snapshot(snapshot) = decision else {
        return json!({ "resourceTemplates": [] });
    };
    let has_photos = snapshot.categories.contains(&AdmittedCategory::Photos);
    let has_screen_text = snapshot.categories.contains(&AdmittedCategory::ScreenText);
    let has_screenshots =
        has_screen_text && snapshot.categories.contains(&AdmittedCategory::Screenshots);
    let has_profile_pics = snapshot.categories.contains(&AdmittedCategory::Entities)
        && snapshot.categories.contains(&AdmittedCategory::ProfilePics)
        && solstone_core_entities::photos::enabled(journal_root).unwrap_or(false);
    let has_audio = snapshot.categories.contains(&AdmittedCategory::Transcripts)
        && snapshot.categories.contains(&AdmittedCategory::AudioMedia);

    let mut templates = Vec::new();
    if has_photos || has_screenshots {
        let (title, description) = match (has_photos, has_screenshots) {
            (true, true) => (
                "journal photo or screenshot",
                "read a bounded JPEG for a journal photo or imported screenshot using a reference from search (or the picture.resource_uri returned by fetch). optional size is full (default, up to 1024 px) or thumb (up to 320 px).",
            ),
            (true, false) => (
                "journal photo",
                "read a bounded JPEG for a journal photo using a reference from search (or the picture.resource_uri returned by fetch). optional size is full (default, up to 1024 px) or thumb (up to 320 px).",
            ),
            (false, true) => (
                "imported screenshot",
                "read a bounded JPEG for an imported screenshot using a reference from search (or the picture.resource_uri returned by fetch). optional size is full (default, up to 1024 px) or thumb (up to 320 px).",
            ),
            (false, false) => unreachable!(),
        };
        templates.push(json!({
            "uriTemplate": "solstone://media/entry/{reference}{?size}",
            "name": "journal_entry_picture",
            "title": title,
            "description": description,
            "mimeType": "image/jpeg",
        }));
    }
    if has_profile_pics {
        templates.push(json!({
            "uriTemplate": "solstone://media/entity/{reference}",
            "name": "entity_profile_pic",
            "title": "profile pic",
            "description": "read a bounded JPEG profile pic for a person or entity using a reference from list_entities or get_week (or the photo.resource_uri returned by get_entity).",
            "mimeType": "image/jpeg",
        }));
    }
    if has_screenshots {
        templates.push(json!({
            "uriTemplate": "solstone://media/screen/{+reference}{?offset_ms,display}",
            "name": "screen_picture",
            "title": "screen picture",
            "description": "read a bounded JPEG from a screen recording or screenshot segment using a reference from list_transcripts (or the picture.resource_uri returned by get_transcript). offset_ms defaults to 0; pass display when a segment has multiple screens.",
            "mimeType": "image/jpeg",
        }));
    }
    if has_audio {
        templates.push(json!({
            "uriTemplate": "solstone://media/audio/{+reference}{?start_ms,duration_ms,track}",
            "name": "transcript_audio_clip",
            "title": "transcript audio clip",
            "description": "read a 16 kHz mono WAV clip (up to 60 seconds; default 30000 ms from start_ms 0) for a transcript segment using a reference from list_transcripts (or the audio.resource_uri returned by get_transcript). pass track when a segment has multiple recordings.",
            "mimeType": "audio/wav",
        }));
    }
    json!({ "resourceTemplates": templates })
}

/// Build the `resource_link` for an imported photo or screenshot returned by `fetch`.
pub(crate) fn entry_picture_link(
    reference: &str,
    day: &str,
    kind: EntryPictureKind,
    size: PictureSize,
    jpeg: &[u8],
) -> ResourceLink {
    let uri = match size {
        PictureSize::Full => format!("{URI_PREFIX}entry/{reference}"),
        PictureSize::Thumb => format!("{URI_PREFIX}entry/{reference}?size=thumb"),
    };
    let hash8 = short_blob_hash(jpeg);
    let label = match kind {
        EntryPictureKind::Photo => "photo",
        EntryPictureKind::Screenshot => "screenshot",
    };
    let name = if day.is_empty() {
        format!("{label}_{hash8}_{}.jpg", size.as_str())
    } else {
        format!("{day}_{label}_{hash8}_{}.jpg", size.as_str())
    };
    let title = match (kind, day.is_empty()) {
        (EntryPictureKind::Photo, true) => "journal photo".to_owned(),
        (EntryPictureKind::Photo, false) => format!("journal photo from {day}"),
        (EntryPictureKind::Screenshot, true) => "imported screenshot".to_owned(),
        (EntryPictureKind::Screenshot, false) => format!("imported screenshot from {day}"),
    };
    ResourceLink {
        uri,
        name,
        title,
        mime_type: "image/jpeg",
        size: jpeg.len(),
    }
}

/// Build the `resource_link` for a confirmed entity profile pic returned by `get_entity`.
pub(crate) fn entity_profile_pic_link(
    reference: &str,
    entity_id: &str,
    jpeg: &[u8],
) -> ResourceLink {
    let uri = format!("{URI_PREFIX}entity/{reference}");
    let safe_id = sanitize_filename_component(entity_id, "entity", 40);
    let hash8 = short_blob_hash(jpeg);
    ResourceLink {
        uri,
        name: format!("profile_{safe_id}_{hash8}.jpg"),
        title: "profile pic".to_owned(),
        mime_type: "image/jpeg",
        size: jpeg.len(),
    }
}

/// Build the `resource_link` for a screen picture returned by `get_transcript`.
pub(crate) fn screen_moment_link(
    reference: &str,
    address: &SegmentAddress,
    offset_ms: u32,
    display: &str,
    jpeg: &[u8],
) -> ResourceLink {
    let encoded_display = percent_encode_query_value(display);
    let uri =
        format!("{URI_PREFIX}screen/{reference}?offset_ms={offset_ms}&display={encoded_display}");
    let clock = segment_clock_prefix(&address.segment);
    let safe_display = sanitize_filename_component(display, "screen", 40);
    let hash8 = short_blob_hash(jpeg);
    ResourceLink {
        uri,
        name: format!(
            "{}_{}_{}_{}ms_{hash8}.jpg",
            address.day, clock, safe_display, offset_ms
        ),
        title: format!("screen picture from {}", address.day),
        mime_type: "image/jpeg",
        size: jpeg.len(),
    }
}

/// Canonical `solstone://media/audio/...` URI for a transcript audio clip.
///
/// Uses the validated requested `duration_ms` (always within `1000..=60000`) so
/// even short trailing clips (< 1 s actual audio) round-trip identically through
/// `resources/read`.
pub(crate) fn audio_clip_uri(
    reference: &str,
    start_ms: u64,
    duration_ms: u64,
    track: &str,
) -> String {
    let encoded_track = percent_encode_query_value(track);
    format!(
        "{URI_PREFIX}audio/{reference}?start_ms={start_ms}&duration_ms={duration_ms}&track={encoded_track}"
    )
}

/// Build the `resource_link` for a WAV audio clip returned by `get_transcript`.
pub(crate) fn audio_clip_link(
    reference: &str,
    address: &SegmentAddress,
    start_ms: u64,
    duration_ms: u64,
    track: &str,
    filename: &str,
    wav: &[u8],
) -> ResourceLink {
    ResourceLink {
        uri: audio_clip_uri(reference, start_ms, duration_ms, track),
        name: filename.to_owned(),
        title: format!("audio clip from {}", address.day),
        mime_type: "audio/wav",
        size: wav.len(),
    }
}

/// Parse and strictly validate a `solstone://media/...` resource URI.
pub(crate) fn parse_resource_uri(uri: &str) -> Result<ParsedResourceUri, ParseResourceError> {
    if uri.is_empty()
        || uri.len() > MAX_RESOURCE_URI_BYTES
        || uri.contains('#')
        || uri.bytes().any(|b| b < 0x20 || b == 0x7F)
    {
        return Err(ParseResourceError::InvalidParams(
            "invalid resource URI".to_owned(),
        ));
    }
    let Some(rest) = uri.strip_prefix(URI_PREFIX) else {
        return Err(ParseResourceError::UnknownRoute);
    };
    let (path_part, query_part) = match rest.split_once('?') {
        Some((path, query)) => (path, Some(query)),
        None => (rest, None),
    };
    let Some((kind, raw_reference)) = path_part.split_once('/') else {
        return Err(ParseResourceError::UnknownRoute);
    };
    if !matches!(kind, "entry" | "entity" | "screen" | "audio") {
        return Err(ParseResourceError::UnknownRoute);
    }
    if raw_reference.is_empty() {
        return Err(ParseResourceError::InvalidParams(
            "resource URI is missing a reference".to_owned(),
        ));
    }
    let reference = percent_decode(raw_reference).ok_or_else(|| {
        ParseResourceError::InvalidParams("invalid resource reference".to_owned())
    })?;
    if reference.is_empty()
        || reference.len() > MAX_OPAQUE_REFERENCE_BYTES
        || reference.contains('\\')
        || reference.starts_with('/')
        || reference.ends_with('/')
        || reference.contains("//")
        || reference.split('/').any(|part| part == "..")
    {
        return Err(ParseResourceError::InvalidParams(
            "invalid resource reference".to_owned(),
        ));
    }
    if matches!(kind, "entry" | "entity") && reference.contains('/') {
        return Err(ParseResourceError::InvalidParams(
            "invalid resource reference".to_owned(),
        ));
    }

    let params = parse_query_params(query_part)?;
    match kind {
        "entry" => {
            let mut size = PictureSize::Full;
            for (key, value) in params {
                match key.as_str() {
                    "size" => {
                        size = match value.as_str() {
                            "full" => PictureSize::Full,
                            "thumb" => PictureSize::Thumb,
                            _ => {
                                return Err(ParseResourceError::InvalidParams(
                                    "size must be full or thumb".to_owned(),
                                ));
                            }
                        };
                    }
                    _ => {
                        return Err(ParseResourceError::InvalidParams(
                            "unsupported query parameter for entry resource".to_owned(),
                        ));
                    }
                }
            }
            Ok(ParsedResourceUri::Entry { reference, size })
        }
        "entity" => {
            if !params.is_empty() {
                return Err(ParseResourceError::InvalidParams(
                    "entity resource URIs do not accept query parameters".to_owned(),
                ));
            }
            Ok(ParsedResourceUri::Entity { reference })
        }
        "screen" => {
            let mut offset_ms: u32 = 0;
            let mut display: Option<String> = None;
            for (key, value) in params {
                match key.as_str() {
                    "offset_ms" => {
                        offset_ms = value.parse::<u32>().map_err(|_| {
                            ParseResourceError::InvalidParams(
                                "offset_ms must be a non-negative integer".to_owned(),
                            )
                        })?;
                    }
                    "display" => {
                        if value.is_empty() || value.len() > MAX_MEDIA_SELECTOR_BYTES {
                            return Err(ParseResourceError::InvalidParams(
                                "invalid display parameter".to_owned(),
                            ));
                        }
                        display = Some(value);
                    }
                    _ => {
                        return Err(ParseResourceError::InvalidParams(
                            "unsupported query parameter for screen resource".to_owned(),
                        ));
                    }
                }
            }
            Ok(ParsedResourceUri::Screen {
                reference,
                offset_ms,
                display,
            })
        }
        "audio" => {
            let mut start_ms: u64 = 0;
            let mut duration_ms: u64 = DEFAULT_AUDIO_DURATION_MS;
            let mut track: Option<String> = None;
            for (key, value) in params {
                match key.as_str() {
                    "start_ms" => {
                        let parsed = value.parse::<u64>().map_err(|_| {
                            ParseResourceError::InvalidParams(
                                "start_ms must be a non-negative integer".to_owned(),
                            )
                        })?;
                        if parsed > MAX_AUDIO_START_MS {
                            return Err(ParseResourceError::InvalidParams(
                                "start_ms exceeds maximum supported offset".to_owned(),
                            ));
                        }
                        start_ms = parsed;
                    }
                    "duration_ms" => {
                        let parsed = value.parse::<u64>().map_err(|_| {
                            ParseResourceError::InvalidParams(
                                "duration_ms must be an integer between 1000 and 60000".to_owned(),
                            )
                        })?;
                        if !(MIN_CLIP_DURATION_MS..=MAX_CLIP_DURATION_MS).contains(&parsed) {
                            return Err(ParseResourceError::InvalidParams(
                                "duration_ms must be between 1000 and 60000".to_owned(),
                            ));
                        }
                        duration_ms = parsed;
                    }
                    "track" => {
                        if value.is_empty() || value.len() > MAX_MEDIA_SELECTOR_BYTES {
                            return Err(ParseResourceError::InvalidParams(
                                "invalid track parameter".to_owned(),
                            ));
                        }
                        track = Some(value);
                    }
                    _ => {
                        return Err(ParseResourceError::InvalidParams(
                            "unsupported query parameter for audio resource".to_owned(),
                        ));
                    }
                }
            }
            Ok(ParsedResourceUri::Audio {
                reference,
                start_ms,
                duration_ms,
                track,
            })
        }
        _ => unreachable!(),
    }
}

/// Execute one authenticated `resources/read` with full grant enforcement,
/// source-release verification, generation re-check, and fail-closed auditing.
pub(crate) fn dispatch_authenticated_resource_read(
    journal_root: &Path,
    principal: DispatchPrincipal<'_>,
    uri: &str,
    now: Timestamp,
) -> Result<ResourceOutput, ResourceError> {
    let target = match parse_resource_uri(uri) {
        Ok(parsed) => parsed,
        Err(ParseResourceError::InvalidParams(message)) => {
            return Err(ResourceError::InvalidParams(message));
        }
        Err(ParseResourceError::UnknownRoute) => {
            let mut recorded = Map::new();
            recorded.insert("resource_kind".to_owned(), json!("unknown"));
            recorded.insert("uri".to_owned(), json!(reference_fingerprint(uri)));
            return match evaluate_connection_read(journal_root, principal.connection) {
                PermissionDecision::Denied { reason } => {
                    let coordinates =
                        audit_resource_call(journal_root, now, principal, recorded, None)?;
                    record_outcome(
                        journal_root,
                        &coordinates,
                        now,
                        Outcome::Refused,
                        owner_denial_reason(reason),
                    );
                    Err(ResourceError::PermissionDenied(reason))
                }
                PermissionDecision::Snapshot(snapshot) => {
                    let coordinates = audit_resource_call(
                        journal_root,
                        now,
                        principal,
                        recorded,
                        Some(&snapshot),
                    )?;
                    record_outcome(
                        journal_root,
                        &coordinates,
                        now,
                        Outcome::Error,
                        Reason::ReferenceMissing,
                    );
                    Err(ResourceError::NotFound {
                        reason: "not_found",
                        message: "resource not found".to_owned(),
                    })
                }
            };
        }
    };

    let recorded = recorded_resource_arguments(&target);

    let snapshot = match evaluate_connection_read(journal_root, principal.connection) {
        PermissionDecision::Snapshot(snapshot) => snapshot,
        PermissionDecision::Denied { reason } => {
            let coordinates = audit_resource_call(journal_root, now, principal, recorded, None)?;
            record_outcome(
                journal_root,
                &coordinates,
                now,
                Outcome::Refused,
                owner_denial_reason(reason),
            );
            return Err(ResourceError::PermissionDenied(reason));
        }
    };

    if let Some((refusal, missing_kind)) = resource_scope_refusal(principal, &snapshot, &target) {
        let coordinates =
            audit_resource_call(journal_root, now, principal, recorded, Some(&snapshot))?;
        record_outcome(journal_root, &coordinates, now, Outcome::Refused, refusal);
        return Err(missing_kind.map_or(
            ResourceError::PermissionDenied("no_permission"),
            ResourceError::KindDenied,
        ));
    }

    let (coordinates, executed) = execute_after_audit(
        || audit_resource_call(journal_root, now, principal, recorded, Some(&snapshot)),
        || read_resource_bytes(journal_root, principal, &snapshot, &target),
    )?;

    let loaded = match executed {
        Ok(loaded) => loaded,
        Err(error) => {
            let reason = match &error {
                ResourceError::InvalidParams(_) => Reason::InvalidArguments,
                ResourceError::NotFound { .. } => Reason::ReferenceMissing,
                ResourceError::Failed { .. } => Reason::FileUnreadable,
                ResourceError::PermissionDenied(_) | ResourceError::KindDenied(_) => {
                    Reason::OutsidePermission
                }
                ResourceError::AuditUnavailable => Reason::AuditUnavailable,
            };
            record_outcome(journal_root, &coordinates, now, Outcome::Error, reason);
            return Err(error);
        }
    };

    // A grant can be narrowed or revoked while a media read is in flight. Do
    // not release any prepared bytes after that change.
    if !permission_generation_is_current(journal_root, principal.connection, &snapshot) {
        record_outcome(
            journal_root,
            &coordinates,
            now,
            Outcome::Refused,
            Reason::PermissionChanged,
        );
        return Err(ResourceError::PermissionDenied("no_permission"));
    }

    let digest = content_digest(&json!({
        "mime_type": loaded.mime_type,
        "blob_sha256": format!("{:x}", Sha256::digest(&loaded.bytes)),
    }));
    let shape = result_shape(1, vec![loaded.target], digest);
    audit::write_outcome(
        journal_root,
        &coordinates,
        now,
        Outcome::Served,
        None,
        Some(shape),
    )
    .map_err(|_| ResourceError::AuditUnavailable)?;

    Ok(ResourceOutput {
        uri: uri.to_owned(),
        mime_type: loaded.mime_type,
        bytes: loaded.bytes,
    })
}

fn resource_scope_refusal(
    principal: DispatchPrincipal<'_>,
    snapshot: &ConnectionReadSnapshot,
    target: &ParsedResourceUri,
) -> Option<(Reason, Option<AdmittedCategory>)> {
    match target {
        ParsedResourceUri::Entry { reference, .. } => {
            let has_photos = snapshot.categories.contains(&AdmittedCategory::Photos);
            let has_screen_text = snapshot.categories.contains(&AdmittedCategory::ScreenText);
            let has_screenshots =
                has_screen_text && snapshot.categories.contains(&AdmittedCategory::Screenshots);
            if let Ok(ReferenceTarget::Entry(entry)) = codec().resolve_entry_or_activity(
                reference,
                principal.connection,
                snapshot.generation,
            ) {
                if crate::photo_picture::is_photo_description(&entry.path) {
                    if !has_photos {
                        return Some((
                            kind_reason(AdmittedCategory::Photos),
                            Some(AdmittedCategory::Photos),
                        ));
                    }
                    return None;
                }
                if crate::screen_picture::import_folder(&entry.path).is_some() {
                    if !has_screen_text {
                        return Some((
                            kind_reason(AdmittedCategory::ScreenText),
                            Some(AdmittedCategory::ScreenText),
                        ));
                    }
                    if !snapshot.categories.contains(&AdmittedCategory::Screenshots) {
                        return Some((
                            kind_reason(AdmittedCategory::Screenshots),
                            Some(AdmittedCategory::Screenshots),
                        ));
                    }
                    return None;
                }
            }
            if !has_photos && !has_screenshots {
                return Some((Reason::OutsidePermission, None));
            }
            None
        }
        ParsedResourceUri::Entity { .. } => {
            if !snapshot.categories.contains(&AdmittedCategory::Entities) {
                return Some((
                    kind_reason(AdmittedCategory::Entities),
                    Some(AdmittedCategory::Entities),
                ));
            }
            if !snapshot.categories.contains(&AdmittedCategory::ProfilePics) {
                return Some((
                    kind_reason(AdmittedCategory::ProfilePics),
                    Some(AdmittedCategory::ProfilePics),
                ));
            }
            None
        }
        ParsedResourceUri::Screen { .. } => {
            if !snapshot.categories.contains(&AdmittedCategory::ScreenText) {
                return Some((
                    kind_reason(AdmittedCategory::ScreenText),
                    Some(AdmittedCategory::ScreenText),
                ));
            }
            if !snapshot.categories.contains(&AdmittedCategory::Screenshots) {
                return Some((
                    kind_reason(AdmittedCategory::Screenshots),
                    Some(AdmittedCategory::Screenshots),
                ));
            }
            None
        }
        ParsedResourceUri::Audio { .. } => {
            if !snapshot.categories.contains(&AdmittedCategory::Transcripts) {
                return Some((
                    kind_reason(AdmittedCategory::Transcripts),
                    Some(AdmittedCategory::Transcripts),
                ));
            }
            if !snapshot.categories.contains(&AdmittedCategory::AudioMedia) {
                return Some((
                    kind_reason(AdmittedCategory::AudioMedia),
                    Some(AdmittedCategory::AudioMedia),
                ));
            }
            None
        }
    }
}

fn read_resource_bytes(
    journal_root: &Path,
    principal: DispatchPrincipal<'_>,
    snapshot: &ConnectionReadSnapshot,
    target: &ParsedResourceUri,
) -> Result<LoadedResource, ResourceError> {
    match target {
        ParsedResourceUri::Entry { reference, size } => {
            read_entry_resource(journal_root, principal, snapshot, reference, *size)
        }
        ParsedResourceUri::Entity { reference } => {
            read_entity_resource(journal_root, principal, snapshot, reference)
        }
        ParsedResourceUri::Screen {
            reference,
            offset_ms,
            display,
        } => read_screen_resource(
            journal_root,
            principal,
            snapshot,
            reference,
            *offset_ms,
            display.as_deref(),
        ),
        ParsedResourceUri::Audio {
            reference,
            start_ms,
            duration_ms,
            track,
        } => read_audio_resource(
            journal_root,
            principal,
            snapshot,
            reference,
            *start_ms,
            *duration_ms,
            track.as_deref(),
        ),
    }
}

fn read_entry_resource(
    journal_root: &Path,
    principal: DispatchPrincipal<'_>,
    snapshot: &ConnectionReadSnapshot,
    reference: &str,
    size: PictureSize,
) -> Result<LoadedResource, ResourceError> {
    let not_found = || ResourceError::NotFound {
        reason: "not_found",
        message: "resource not found".to_owned(),
    };
    let ReferenceTarget::Entry(entry) = codec()
        .resolve_entry_or_activity(reference, principal.connection, snapshot.generation)
        .map_err(|_| not_found())?
    else {
        return Err(not_found());
    };
    let is_photo = crate::photo_picture::is_photo_description(&entry.path);
    let import_folder = crate::screen_picture::import_folder(&entry.path);
    if !is_photo && import_folder.is_none() {
        return Err(not_found());
    }
    let query_boundary = boundary(snapshot, None, None);
    if !live_allows(
        journal_root,
        &query_boundary,
        &entry.path,
        Some(&entry.stream),
    ) {
        return Err(not_found());
    }
    #[cfg(feature = "test-hooks")]
    crate::source_freeze_barrier::checkpoint(crate::source_freeze_barrier::FreezePoint::Before);
    let pre_allowed =
        crate::source_release::check_indexed_source_release(journal_root, &entry.path);
    let media_reply = if pre_allowed {
        if is_photo {
            let (state, kept) =
                crate::photo_picture::photo_picture(journal_root, &entry.path, size);
            Some((state, kept.map(|picture| picture.jpeg)))
        } else {
            import_folder.map(|folder| crate::screen_picture::imported(journal_root, folder, size))
        }
    } else {
        None
    };
    let post_allowed = pre_allowed
        && live_allows(
            journal_root,
            &query_boundary,
            &entry.path,
            Some(&entry.stream),
        )
        && crate::source_release::check_indexed_source_release(journal_root, &entry.path);
    #[cfg(feature = "test-hooks")]
    crate::source_freeze_barrier::checkpoint(crate::source_freeze_barrier::FreezePoint::After);

    if !post_allowed {
        return Err(not_found());
    }
    let (info, jpeg) = media_reply.ok_or_else(not_found)?;
    let bytes = match jpeg {
        Some(bytes) => bytes,
        None => return Err(picture_state_error(&info)),
    };
    Ok(LoadedResource {
        mime_type: "image/jpeg",
        bytes,
        target: format!("{}#{}", entry.path, entry.idx),
    })
}

fn read_entity_resource(
    journal_root: &Path,
    principal: DispatchPrincipal<'_>,
    snapshot: &ConnectionReadSnapshot,
    reference: &str,
) -> Result<LoadedResource, ResourceError> {
    let not_found = || ResourceError::NotFound {
        reason: "not_found",
        message: "resource not found".to_owned(),
    };
    let (entity_id, facet) = if crate::names::is_name(reference) {
        crate::names::parse_entity(reference).ok_or_else(not_found)?
    } else {
        let ReferenceTarget::Entity(entity_ref) = codec()
            .resolve(
                reference,
                ReferenceKind::Entity,
                principal.connection,
                snapshot.generation,
            )
            .map_err(|_| not_found())?
        else {
            return Err(not_found());
        };
        (entity_ref.entity_id, entity_ref.facet_id)
    };
    let Some((facet_id, facet_dir)) = crate::may_see::reached_facet(journal_root, snapshot, &facet)
    else {
        return Err(not_found());
    };
    let present = read_scoped_entities(journal_root, &facet_dir)
        .map_err(|_| ResourceError::Failed {
            reason: "unreadable",
            message: "the journal could not read this entity".to_owned(),
        })?
        .into_iter()
        .any(|item| item.entity_id == entity_id);
    if !present {
        return Err(not_found());
    }
    if !solstone_core_entities::photos::enabled(journal_root).map_err(|_| {
        ResourceError::Failed {
            reason: "unreadable",
            message: "the journal could not read this picture".to_owned(),
        }
    })? {
        return Err(not_found());
    }
    #[cfg(feature = "test-hooks")]
    crate::source_freeze_barrier::checkpoint(crate::source_freeze_barrier::FreezePoint::Before);
    let photo = solstone_core_entities::photos::photo(journal_root, &entity_id, Some(&facet_dir))
        .map_err(|_| ResourceError::Failed {
        reason: "unreadable",
        message: "the journal could not read this picture".to_owned(),
    })?;
    let still_enabled = solstone_core_entities::photos::enabled(journal_root).unwrap_or(false)
        && crate::may_see::reached_facet(journal_root, snapshot, &facet).is_some();
    #[cfg(feature = "test-hooks")]
    crate::source_freeze_barrier::checkpoint(crate::source_freeze_barrier::FreezePoint::After);

    if !still_enabled {
        return Err(not_found());
    }
    let Some(raw_bytes) = photo.jpeg else {
        return Err(ResourceError::NotFound {
            reason: "not_kept",
            message: "no profile pic is kept for this entity".to_owned(),
        });
    };
    let bytes = bounded_photo(&raw_bytes).map_err(|_| ResourceError::Failed {
        reason: "unreadable",
        message: "the journal could not read this picture".to_owned(),
    })?;
    Ok(LoadedResource {
        mime_type: "image/jpeg",
        bytes,
        target: format!("facet:{facet_id}/entity:{entity_id}"),
    })
}

fn resolve_segment_reference(
    journal_root: &Path,
    principal: DispatchPrincipal<'_>,
    snapshot: &ConnectionReadSnapshot,
    reference: &str,
) -> Result<SegmentAddress, ResourceError> {
    let not_found = || ResourceError::NotFound {
        reason: "not_found",
        message: "resource not found".to_owned(),
    };
    if crate::names::is_name(reference) {
        return crate::names::resolve_segment(journal_root, reference).ok_or_else(not_found);
    }
    match codec()
        .resolve_segment_or_span(reference, principal.connection, snapshot.generation)
        .map_err(|_| not_found())?
    {
        ReferenceTarget::Segment(segment_ref) => Ok(segment_ref.into()),
        ReferenceTarget::SourceSpan(_) | ReferenceTarget::SegmentSpan(_) => Err(
            ResourceError::InvalidParams("pass a single segment reference, not a span".to_owned()),
        ),
        _ => Err(not_found()),
    }
}

fn read_screen_resource(
    journal_root: &Path,
    principal: DispatchPrincipal<'_>,
    snapshot: &ConnectionReadSnapshot,
    reference: &str,
    offset_ms: u32,
    display: Option<&str>,
) -> Result<LoadedResource, ResourceError> {
    let not_found = || ResourceError::NotFound {
        reason: "not_found",
        message: "resource not found".to_owned(),
    };
    let address = resolve_segment_reference(journal_root, principal, snapshot, reference)?;
    let segment =
        iter_segments_skipping_unreadable_streams(journal_root, PathOrDay::Day(&address.day))
            .map_err(|_| not_found())?
            .into_iter()
            .find(|segment| {
                segment
                    .record_identity()
                    .is_ok_and(|id| id.stream == address.stream && id.name == address.segment)
            })
            .ok_or_else(not_found)?;
    if crate::may_see::segment_shown(
        journal_root,
        snapshot,
        &crate::body_source::BodyHides::read(journal_root),
        None,
        &address.day,
        &address.stream,
        &address.segment,
        &segment,
    )
    .is_none()
    {
        return Err(not_found());
    }
    #[cfg(feature = "test-hooks")]
    crate::source_freeze_barrier::checkpoint(crate::source_freeze_barrier::FreezePoint::Before);
    let pre_allowed = crate::source_release::check_segment_transcript_release(
        journal_root,
        &address.day,
        &address.stream,
        &address.segment,
    );
    let moment_res = if pre_allowed {
        Some(crate::screen_picture::moment(
            journal_root,
            segment.path(),
            &address,
            offset_ms,
            display,
        ))
    } else {
        None
    };
    let post_allowed = pre_allowed
        && crate::may_see::segment_scope(
            journal_root,
            snapshot,
            &address.day,
            &address.stream,
            &address.segment,
        ) == Some(true)
        && crate::source_release::check_segment_transcript_release(
            journal_root,
            &address.day,
            &address.stream,
            &address.segment,
        );
    #[cfg(feature = "test-hooks")]
    crate::source_freeze_barrier::checkpoint(crate::source_freeze_barrier::FreezePoint::After);

    if !post_allowed {
        return Err(not_found());
    }
    let (info, jpeg) = match moment_res.ok_or_else(not_found)? {
        Ok(reply) => reply,
        Err(DispatchError::InvalidInput) => {
            return Err(ResourceError::InvalidParams(
                "offset_ms is outside this segment's duration".to_owned(),
            ));
        }
        Err(DispatchError::Tool(ToolError::ReferenceNotFound)) => return Err(not_found()),
        Err(_) => {
            return Err(ResourceError::Failed {
                reason: "unreadable",
                message: "the journal could not read this picture".to_owned(),
            });
        }
    };
    let bytes = match jpeg {
        Some(bytes) => bytes,
        None => return Err(picture_state_error(&info)),
    };
    Ok(LoadedResource {
        mime_type: "image/jpeg",
        bytes,
        target: format!("{}/{}/{}", address.day, address.stream, address.segment),
    })
}

fn read_audio_resource(
    journal_root: &Path,
    principal: DispatchPrincipal<'_>,
    snapshot: &ConnectionReadSnapshot,
    reference: &str,
    start_ms: u64,
    duration_ms: u64,
    track: Option<&str>,
) -> Result<LoadedResource, ResourceError> {
    let not_found = || ResourceError::NotFound {
        reason: "not_found",
        message: "resource not found".to_owned(),
    };
    let address = resolve_segment_reference(journal_root, principal, snapshot, reference)?;
    let segment =
        iter_segments_skipping_unreadable_streams(journal_root, PathOrDay::Day(&address.day))
            .map_err(|_| not_found())?
            .into_iter()
            .find(|segment| {
                segment
                    .record_identity()
                    .is_ok_and(|id| id.stream == address.stream && id.name == address.segment)
            })
            .ok_or_else(not_found)?;
    if crate::may_see::segment_shown(
        journal_root,
        snapshot,
        &crate::body_source::BodyHides::read(journal_root),
        None,
        &address.day,
        &address.stream,
        &address.segment,
        &segment,
    )
    .is_none()
    {
        return Err(not_found());
    }
    let clip_request = AudioClipRequest {
        start_ms,
        duration_ms,
        track: track.map(str::to_owned),
        encoding: AudioEncoding::Content,
    };
    #[cfg(feature = "test-hooks")]
    crate::source_freeze_barrier::checkpoint(crate::source_freeze_barrier::FreezePoint::Before);
    let pre_allowed = crate::source_release::check_segment_transcript_release(
        journal_root,
        &address.day,
        &address.stream,
        &address.segment,
    );
    let clip_res = if pre_allowed {
        Some(crate::audio_media::clip(
            journal_root,
            segment.path(),
            &address,
            &clip_request,
        ))
    } else {
        None
    };
    let post_allowed = pre_allowed
        && crate::may_see::segment_scope(
            journal_root,
            snapshot,
            &address.day,
            &address.stream,
            &address.segment,
        ) == Some(true)
        && crate::source_release::check_segment_transcript_release(
            journal_root,
            &address.day,
            &address.stream,
            &address.segment,
        );
    #[cfg(feature = "test-hooks")]
    crate::source_freeze_barrier::checkpoint(crate::source_freeze_barrier::FreezePoint::After);

    if !post_allowed {
        return Err(not_found());
    }
    let (info, wav) = match clip_res.ok_or_else(not_found)? {
        Ok(reply) => reply,
        Err(DispatchError::Tool(ToolError::ReferenceNotFound)) => return Err(not_found()),
        Err(_) => {
            return Err(ResourceError::Failed {
                reason: "unreadable",
                message: "the journal could not read this audio recording".to_owned(),
            });
        }
    };
    let bytes = match wav {
        Some(bytes) => bytes,
        None => return Err(audio_state_error(&info)),
    };
    Ok(LoadedResource {
        mime_type: "audio/wav",
        bytes,
        target: format!("{}/{}/{}", address.day, address.stream, address.segment),
    })
}

fn picture_state_error(info: &Value) -> ResourceError {
    let state = info.get("state").and_then(Value::as_str).unwrap_or("");
    let message = info
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("resource not found")
        .to_owned();
    match state {
        "choose_display" => {
            let displays = info
                .get("displays")
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .unwrap_or_default();
            ResourceError::InvalidParams(if displays.is_empty() {
                "pass display in the resource URI".to_owned()
            } else {
                format!("pass display in the resource URI; available displays: {displays}")
            })
        }
        "not_kept" => ResourceError::NotFound {
            reason: "not_kept",
            message,
        },
        "no_frame" => ResourceError::NotFound {
            reason: "no_frame",
            message,
        },
        "unreadable" => ResourceError::Failed {
            reason: "unreadable",
            message,
        },
        _ => ResourceError::NotFound {
            reason: "not_found",
            message,
        },
    }
}

fn audio_state_error(info: &Value) -> ResourceError {
    let state = info.get("state").and_then(Value::as_str).unwrap_or("");
    let message = info
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("resource not found")
        .to_owned();
    match state {
        "choose_track" => {
            let tracks = info
                .get("tracks")
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .unwrap_or_default();
            ResourceError::InvalidParams(if tracks.is_empty() {
                "pass track in the resource URI".to_owned()
            } else {
                format!("pass track in the resource URI; available tracks: {tracks}")
            })
        }
        "not_kept" => ResourceError::NotFound {
            reason: "not_kept",
            message,
        },
        "out_of_range" => ResourceError::NotFound {
            reason: "out_of_range",
            message,
        },
        "empty" => ResourceError::NotFound {
            reason: "empty",
            message,
        },
        "unreadable" => ResourceError::Failed {
            reason: "unreadable",
            message,
        },
        _ => ResourceError::NotFound {
            reason: "not_found",
            message,
        },
    }
}

fn audit_resource_call(
    journal_root: &Path,
    now: Timestamp,
    principal: DispatchPrincipal<'_>,
    arguments: Map<String, Value>,
    snapshot: Option<&ConnectionReadSnapshot>,
) -> Result<AuditCoordinates, ResourceError> {
    audit::write_admitted_interaction(
        journal_root,
        now,
        &Admission {
            connection: principal.connection,
            agent_identity: principal.agent_identity,
            tool_name: ToolName::ReadResource,
            arguments,
            permission: snapshot.map(audit_permission_snapshot),
        },
    )
    .map_err(|_| ResourceError::AuditUnavailable)
}

fn recorded_resource_arguments(target: &ParsedResourceUri) -> Map<String, Value> {
    let mut arguments = Map::new();
    match target {
        ParsedResourceUri::Entry { reference, size } => {
            arguments.insert("resource_kind".to_owned(), json!("entry"));
            arguments.insert(
                "reference".to_owned(),
                json!(reference_fingerprint(reference)),
            );
            arguments.insert("size".to_owned(), json!(size.as_str()));
        }
        ParsedResourceUri::Entity { reference } => {
            arguments.insert("resource_kind".to_owned(), json!("entity"));
            arguments.insert(
                "reference".to_owned(),
                json!(reference_fingerprint(reference)),
            );
        }
        ParsedResourceUri::Screen {
            reference,
            offset_ms,
            display,
        } => {
            arguments.insert("resource_kind".to_owned(), json!("screen"));
            arguments.insert(
                "reference".to_owned(),
                json!(reference_fingerprint(reference)),
            );
            arguments.insert("offset_ms".to_owned(), json!(offset_ms));
            if let Some(display) = display {
                arguments.insert("display".to_owned(), json!(display));
            }
        }
        ParsedResourceUri::Audio {
            reference,
            start_ms,
            duration_ms,
            track,
        } => {
            arguments.insert("resource_kind".to_owned(), json!("audio"));
            arguments.insert(
                "reference".to_owned(),
                json!(reference_fingerprint(reference)),
            );
            arguments.insert("start_ms".to_owned(), json!(start_ms));
            arguments.insert("duration_ms".to_owned(), json!(duration_ms));
            if let Some(track) = track {
                arguments.insert("track".to_owned(), json!(track));
            }
        }
    }
    arguments
}

fn parse_query_params(query: Option<&str>) -> Result<Vec<(String, String)>, ParseResourceError> {
    let Some(query) = query else {
        return Ok(Vec::new());
    };
    if query.is_empty() {
        return Err(ParseResourceError::InvalidParams(
            "resource URI has an empty query string".to_owned(),
        ));
    }
    let mut seen = BTreeSet::new();
    let mut pairs = Vec::new();
    for part in query.split('&') {
        let Some((raw_key, raw_value)) = part.split_once('=') else {
            return Err(ParseResourceError::InvalidParams(
                "invalid query parameter".to_owned(),
            ));
        };
        if raw_key.is_empty() || raw_value.is_empty() {
            return Err(ParseResourceError::InvalidParams(
                "invalid query parameter".to_owned(),
            ));
        }
        if !seen.insert(raw_key.to_owned()) {
            return Err(ParseResourceError::InvalidParams(
                "duplicate query parameter".to_owned(),
            ));
        }
        let decoded = percent_decode(raw_value).ok_or_else(|| {
            ParseResourceError::InvalidParams("invalid percent-encoding in query".to_owned())
        })?;
        if decoded.is_empty() {
            return Err(ParseResourceError::InvalidParams(
                "invalid query parameter".to_owned(),
            ));
        }
        pairs.push((raw_key.to_owned(), decoded));
    }
    Ok(pairs)
}

fn percent_encode_query_value(value: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(byte as char);
        } else {
            out.push('%');
            out.push(HEX[(byte >> 4) as usize] as char);
            out.push(HEX[(byte & 0x0F) as usize] as char);
        }
    }
    out
}

fn percent_decode(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        if byte == b'%' {
            if index + 2 >= bytes.len() {
                return None;
            }
            let hi = hex_nibble(bytes[index + 1])?;
            let lo = hex_nibble(bytes[index + 2])?;
            let decoded = (hi << 4) | lo;
            if decoded < 0x20 || decoded == 0x7F {
                return None;
            }
            out.push(decoded);
            index += 3;
        } else {
            if byte < 0x20 || byte == 0x7F {
                return None;
            }
            out.push(byte);
            index += 1;
        }
    }
    String::from_utf8(out).ok()
}

const fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn short_blob_hash(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(8);
    for &byte in &digest[..4] {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0F) as usize] as char);
    }
    out
}

fn segment_clock_prefix(segment: &str) -> &str {
    segment
        .split('_')
        .next()
        .filter(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
        .unwrap_or("000000")
}

fn sanitize_filename_component(raw: &str, fallback: &str, max_len: usize) -> String {
    let clean: String = raw
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-'))
        .take(max_len)
        .collect();
    if clean.is_empty() {
        fallback.to_owned()
    } else {
        clean
    }
}

#[cfg(all(test, not(feature = "full-tests")))]
mod tests {
    use std::collections::BTreeSet;

    use serde_json::json;
    use solstone_core_indexer_query::{AdmittedCategory, ConnectionScope};

    use super::*;

    fn snapshot_with(categories: &[AdmittedCategory]) -> PermissionDecision {
        PermissionDecision::Snapshot(ConnectionReadSnapshot {
            categories: categories.iter().copied().collect::<BTreeSet<_>>(),
            scope: ConnectionScope::WholeJournal,
            generation: 1,
            places_precision: None,
        })
    }

    #[test]
    fn resource_uri_builders_round_trip_through_parser() {
        let entry_full = entry_picture_link(
            "sealed_entry_1",
            "20260412",
            EntryPictureKind::Photo,
            PictureSize::Full,
            b"jpeg-a",
        );
        assert_eq!(entry_full.uri, "solstone://media/entry/sealed_entry_1");
        assert_eq!(
            parse_resource_uri(&entry_full.uri),
            Ok(ParsedResourceUri::Entry {
                reference: "sealed_entry_1".to_owned(),
                size: PictureSize::Full,
            })
        );

        let entry_thumb = entry_picture_link(
            "sealed_entry_2",
            "20260412",
            EntryPictureKind::Screenshot,
            PictureSize::Thumb,
            b"jpeg-b",
        );
        assert_eq!(
            entry_thumb.uri,
            "solstone://media/entry/sealed_entry_2?size=thumb"
        );
        assert_ne!(entry_full.name, entry_thumb.name);
        assert_eq!(
            parse_resource_uri(&entry_thumb.uri),
            Ok(ParsedResourceUri::Entry {
                reference: "sealed_entry_2".to_owned(),
                size: PictureSize::Thumb,
            })
        );

        let entity_link = entity_profile_pic_link("alice@work", "alice", b"jpeg-c");
        assert_eq!(entity_link.uri, "solstone://media/entity/alice@work");
        assert_eq!(
            parse_resource_uri(&entity_link.uri),
            Ok(ParsedResourceUri::Entity {
                reference: "alice@work".to_owned(),
            })
        );
        // RFC 6570 simple expansion percent-encodes `@` to `%40`; accept both.
        assert_eq!(
            parse_resource_uri("solstone://media/entity/alice%40work"),
            Ok(ParsedResourceUri::Entity {
                reference: "alice@work".to_owned(),
            })
        );

        let seg_addr = SegmentAddress {
            day: "20260412".to_owned(),
            stream: "screen.mac".to_owned(),
            segment: "091500_60".to_owned(),
            version: None,
        };
        let screen_link = screen_moment_link(
            "20260412/091500_60/abcdef",
            &seg_addr,
            5_000,
            "main display+1",
            b"jpeg-d",
        );
        assert!(!screen_link.name.contains("screen.mac"));
        assert_eq!(
            parse_resource_uri(&screen_link.uri),
            Ok(ParsedResourceUri::Screen {
                reference: "20260412/091500_60/abcdef".to_owned(),
                offset_ms: 5_000,
                display: Some("main display+1".to_owned()),
            })
        );

        let audio_link = audio_clip_link(
            "20260412/091500_60/abcdef",
            &seg_addr,
            2_000,
            30_000,
            "mic+left 1",
            "20260412_091500_micleft1_2000ms.wav",
            b"RIFFWAVE",
        );
        assert_eq!(
            parse_resource_uri(&audio_link.uri),
            Ok(ParsedResourceUri::Audio {
                reference: "20260412/091500_60/abcdef".to_owned(),
                start_ms: 2_000,
                duration_ms: 30_000,
                track: Some("mic+left 1".to_owned()),
            })
        );
    }

    #[test]
    fn two_same_day_photos_produce_distinct_collision_free_names() {
        let first = entry_picture_link(
            "ref_a",
            "20260412",
            EntryPictureKind::Photo,
            PictureSize::Full,
            b"photo-bytes-one",
        );
        let second = entry_picture_link(
            "ref_b",
            "20260412",
            EntryPictureKind::Photo,
            PictureSize::Full,
            b"photo-bytes-two",
        );
        assert_ne!(first.name, second.name);
        assert!(first.name.starts_with("20260412_photo_"));
        assert!(first.name.ends_with("_full.jpg"));
    }

    #[test]
    fn parser_rejects_malformed_traversal_unknown_and_out_of_range_uris() {
        assert_eq!(
            parse_resource_uri("https://example.com/photo.jpg"),
            Err(ParseResourceError::UnknownRoute)
        );
        assert_eq!(
            parse_resource_uri("solstone://media/unknown/abc"),
            Err(ParseResourceError::UnknownRoute)
        );
        for bad in [
            "",
            "solstone://media/entry/",
            "solstone://media/entry/abc#frag",
            "solstone://media/entry/abc?",
            "solstone://media/entry/abc?size=huge",
            "solstone://media/entry/abc?size=full&size=thumb",
            "solstone://media/entry/abc?extra=1",
            "solstone://media/entry/a/b",
            "solstone://media/entry/abc%00def",
            "solstone://media/entry/abc%ZZ",
            "solstone://media/entity/alice@work?size=full",
            "solstone://media/screen/../secret",
            "solstone://media/screen/20260412/090000_60/abcdef?offset_ms=-1",
            "solstone://media/screen/20260412/090000_60/abcdef?display=",
            "solstone://media/audio/20260412/090000_60/abcdef?duration_ms=500",
            "solstone://media/audio/20260412/090000_60/abcdef?duration_ms=60001",
            "solstone://media/audio/20260412/090000_60/abcdef?start_ms=86400001",
            "solstone://media/audio/20260412/090000_60/abcdef?foo=bar",
        ] {
            assert!(
                matches!(
                    parse_resource_uri(bad),
                    Err(ParseResourceError::InvalidParams(_))
                ),
                "expected InvalidParams for {bad:?}, got {:?}",
                parse_resource_uri(bad)
            );
        }
    }

    #[test]
    fn list_and_read_param_validators_accept_meta_and_reject_bad_shapes() {
        assert!(validate_list_params(None).is_ok());
        assert!(validate_list_params(Some(&json!(null))).is_ok());
        assert!(validate_list_params(Some(&json!({}))).is_ok());
        assert!(validate_list_params(Some(&json!({"_meta": {"progressToken": 2}}))).is_ok());
        assert!(validate_list_params(Some(&json!({"cursor": null}))).is_ok());
        assert!(validate_list_params(Some(&json!({"cursor": "abc"}))).is_err());
        assert!(validate_list_params(Some(&json!({"unknown": 1}))).is_err());
        assert!(validate_list_params(Some(&json!([]))).is_err());

        assert_eq!(
            extract_read_uri(Some(&json!({
                "_meta": {"progressToken": 3},
                "uri": "solstone://media/entity/alice@work"
            }))),
            Ok("solstone://media/entity/alice@work")
        );
        assert!(extract_read_uri(None).is_err());
        assert!(extract_read_uri(Some(&json!({}))).is_err());
        assert!(extract_read_uri(Some(&json!({"uri": ""}))).is_err());
        assert!(extract_read_uri(Some(&json!({"uri": "solstone://media/a", "extra": 1}))).is_err());
    }

    #[test]
    fn templates_follow_connection_grants_and_owner_faces_switch() {
        let journal = tempfile::Builder::new()
            .prefix("solstone-mcp-res-tpl-")
            .tempdir_in(crate::test_scratch())
            .unwrap();

        let denied = advertised_resource_templates(
            journal.path(),
            &PermissionDecision::Denied {
                reason: "no_permission",
            },
        );
        assert_eq!(denied["resourceTemplates"].as_array().unwrap().len(), 0);

        let photos_only = advertised_resource_templates(
            journal.path(),
            &snapshot_with(&[AdmittedCategory::Photos]),
        );
        let names = |v: &Value| {
            v["resourceTemplates"]
                .as_array()
                .unwrap()
                .iter()
                .map(|t| t["name"].as_str().unwrap().to_owned())
                .collect::<Vec<_>>()
        };
        assert_eq!(names(&photos_only), vec!["journal_entry_picture"]);

        let audio_and_screen = advertised_resource_templates(
            journal.path(),
            &snapshot_with(&[
                AdmittedCategory::ScreenText,
                AdmittedCategory::Screenshots,
                AdmittedCategory::Transcripts,
                AdmittedCategory::AudioMedia,
            ]),
        );
        assert_eq!(
            names(&audio_and_screen),
            vec![
                "journal_entry_picture",
                "screen_picture",
                "transcript_audio_clip"
            ]
        );

        // Profile pics are advertised when enabled by default, and hidden when
        // the owner turns faces off in config/journal.json.
        let with_profile_switch_on = advertised_resource_templates(
            journal.path(),
            &snapshot_with(&[AdmittedCategory::Entities, AdmittedCategory::ProfilePics]),
        );
        assert_eq!(names(&with_profile_switch_on), vec!["entity_profile_pic"]);

        std::fs::create_dir_all(journal.path().join("config")).unwrap();
        std::fs::write(
            journal.path().join("config/journal.json"),
            r#"{"faces":{"enabled":false}}"#,
        )
        .unwrap();
        let with_profile_switch_off = advertised_resource_templates(
            journal.path(),
            &snapshot_with(&[AdmittedCategory::Entities, AdmittedCategory::ProfilePics]),
        );
        assert!(!names(&with_profile_switch_off).contains(&"entity_profile_pic".to_owned()));
    }
}
