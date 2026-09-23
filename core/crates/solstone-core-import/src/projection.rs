// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Unified result projection across import attempts, publication outcomes, and retained manifests.

use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use solstone_core_journal_io::path_lexists;

use crate::metadata::{
    AttemptFacts, AttemptRead, AttemptState, IMPORT_FAILED_REASON, IMPORT_UNCONFIRMED_REASON,
    read_attempt_facts, read_provenance,
};
use crate::publish::{PublicationRecord, PublicationStatus};

/// User-facing projection status of an import attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectionStatus {
    Success,
    Failed,
    Running,
    Pending,
    Unavailable,
    Unconfirmed,
}

impl ProjectionStatus {
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Failed => "failed",
            Self::Running => "running",
            Self::Pending => "pending",
            Self::Unavailable => "unavailable",
            Self::Unconfirmed => "unconfirmed",
        }
    }
}

/// Unified projection of an import item across all stages and record files.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportProjection {
    pub import_id: String,
    pub source_type: String,
    pub source_display: String,
    pub status: ProjectionStatus,
    pub error: Option<String>,
    pub error_stage: Option<String>,
    pub created_at: f64,
    pub imported_at: f64,
    pub entries_written: Option<u64>,
    pub entities_seeded: Option<u64>,
    pub total_files_created: Option<u64>,
    pub duration_ms: Option<u64>,
    pub date_range: Option<(String, String)>,
    pub days_affected: Vec<String>,
    pub target_day: Option<String>,
    pub unavailable_description: Option<String>,
    pub unavailable_pages: Option<u64>,
    pub has_gaps: bool,
    pub task_id: Option<String>,
    pub upload_timestamp: Option<f64>,
    pub original_filename: Option<String>,
    pub file_size: Option<u64>,
    pub mime_type: Option<String>,
    pub setting: Option<String>,
    pub staged_path: Option<String>,
    pub principal_collision: Option<Value>,
    pub merge_summary: Option<Value>,
    pub attempt: Option<AttemptFacts>,
    pub generation: Option<u64>,
    pub attempt_id: Option<String>,
    pub raw_metadata: Option<Value>,
    pub raw_publication: Option<Value>,
}

impl ImportProjection {
    /// True for an import the native producer wrote: an attempt record or a typed
    /// publication record. Rows from the other importers keep their recorded shape.
    #[must_use]
    pub fn is_native(&self) -> bool {
        self.attempt.is_some()
            || self
                .raw_publication
                .as_ref()
                .and_then(|record| record.get("schema"))
                .and_then(Value::as_str)
                == Some(crate::publish::PUBLICATION_SCHEMA)
    }

    /// The facts a native row carries beyond the recorded legacy row shape. A measured
    /// zero stays `0` and an unknown value stays `null`; nothing is summed or defaulted.
    #[must_use]
    pub fn native_row_overlay(&self) -> Map<String, Value> {
        let number =
            |value: Option<u64>| value.map_or(Value::Null, |count| serde_json::json!(count));
        let mut map = Map::new();
        map.insert(
            "source_type".to_owned(),
            Value::String(self.source_type.clone()),
        );
        map.insert(
            "target_day".to_owned(),
            self.target_day
                .as_ref()
                .map_or(Value::Null, |day| Value::String(day.clone())),
        );
        map.insert("entries_written".to_owned(), number(self.entries_written));
        map.insert("entities_seeded".to_owned(), number(self.entities_seeded));
        map.insert(
            "total_files_created".to_owned(),
            number(self.total_files_created),
        );
        map.insert("duration_ms".to_owned(), number(self.duration_ms));
        map.insert(
            "date_range".to_owned(),
            self.date_range
                .as_ref()
                .map_or(Value::Null, |(start, end)| serde_json::json!([start, end])),
        );
        map.insert("generation".to_owned(), number(self.generation));
        if let Some(attempt_id) = &self.attempt_id {
            map.insert("attempt_id".to_owned(), Value::String(attempt_id.clone()));
        }
        // Only that the description is missing goes out on a row: the provider's own
        // error text stays in the durable record and is not an owner-facing fact.
        map.insert(
            "unavailable_description".to_owned(),
            self.unavailable_description
                .as_ref()
                .map_or(Value::Null, |_| {
                    Value::String("description unavailable".to_owned())
                }),
        );
        map.insert(
            "unavailable_pages".to_owned(),
            number(self.unavailable_pages),
        );
        map.insert("has_gaps".to_owned(), Value::Bool(self.has_gaps));
        if let Some(failed) = self.attempt.as_ref().and_then(|facts| facts.input_failures) {
            map.insert("input_failures".to_owned(), serde_json::json!(failed));
        }
        map
    }

    /// Convert projection into a JSON-compatible map preserving null vs measured zero.
    #[must_use]
    pub fn to_json_map(&self) -> Map<String, Value> {
        let mut map = Map::new();
        map.insert(
            "import_id".to_owned(),
            Value::String(self.import_id.clone()),
        );
        map.insert(
            "timestamp".to_owned(),
            Value::String(self.import_id.clone()),
        );
        map.insert("created_at".to_owned(), serde_json::json!(self.created_at));
        map.insert(
            "imported_at".to_owned(),
            serde_json::json!(self.imported_at),
        );
        map.insert(
            "source_type".to_owned(),
            Value::String(self.source_type.clone()),
        );
        map.insert(
            "source_display".to_owned(),
            Value::String(self.source_display.clone()),
        );
        map.insert(
            "status".to_owned(),
            Value::String(self.status.as_str().to_owned()),
        );
        map.insert(
            "generation".to_owned(),
            self.generation
                .map_or(Value::Null, |g| serde_json::json!(g)),
        );
        if let Some(att) = &self.attempt_id {
            map.insert("attempt_id".to_owned(), Value::String(att.clone()));
        }
        map.insert(
            "error".to_owned(),
            self.error
                .as_ref()
                .map_or(Value::Null, |e| Value::String(e.clone())),
        );
        map.insert(
            "error_stage".to_owned(),
            self.error_stage
                .as_ref()
                .map_or(Value::Null, |s| Value::String(s.clone())),
        );
        map.insert(
            "entries_written".to_owned(),
            self.entries_written
                .map_or(Value::Null, |c| serde_json::json!(c)),
        );
        map.insert(
            "entities_seeded".to_owned(),
            self.entities_seeded
                .map_or(Value::Null, |c| serde_json::json!(c)),
        );
        map.insert(
            "total_files_created".to_owned(),
            self.total_files_created
                .map_or(Value::Null, |c| serde_json::json!(c)),
        );
        map.insert(
            "duration_ms".to_owned(),
            self.duration_ms
                .map_or(Value::Null, |d| serde_json::json!(d)),
        );
        map.insert(
            "date_range".to_owned(),
            self.date_range
                .as_ref()
                .map_or(Value::Null, |(s, e)| serde_json::json!([s, e])),
        );
        map.insert(
            "days_affected".to_owned(),
            serde_json::json!(self.days_affected),
        );
        map.insert(
            "target_day".to_owned(),
            self.target_day
                .as_ref()
                .map_or(Value::Null, |d| Value::String(d.clone())),
        );
        map.insert(
            "unavailable_description".to_owned(),
            self.unavailable_description
                .as_ref()
                .map_or(Value::Null, |u| Value::String(u.clone())),
        );
        map.insert(
            "unavailable_pages".to_owned(),
            self.unavailable_pages
                .map_or(Value::Null, |p| serde_json::json!(p)),
        );
        map.insert("has_gaps".to_owned(), Value::Bool(self.has_gaps));
        map.insert(
            "task_id".to_owned(),
            self.task_id
                .as_ref()
                .map_or(Value::Null, |t| Value::String(t.clone())),
        );
        map.insert(
            "upload_timestamp".to_owned(),
            self.upload_timestamp
                .map_or(Value::Null, |u| serde_json::json!(u)),
        );
        map.insert(
            "original_filename".to_owned(),
            self.original_filename
                .as_ref()
                .map_or(Value::Null, |f| Value::String(f.clone())),
        );
        map.insert(
            "file_size".to_owned(),
            self.file_size.map_or(Value::Null, |s| serde_json::json!(s)),
        );
        map.insert(
            "mime_type".to_owned(),
            self.mime_type
                .as_ref()
                .map_or(Value::Null, |m| Value::String(m.clone())),
        );
        map.insert(
            "setting".to_owned(),
            self.setting
                .as_ref()
                .map_or(Value::Null, |s| Value::String(s.clone())),
        );
        map.insert(
            "staged_path".to_owned(),
            self.staged_path
                .as_ref()
                .map_or(Value::Null, |p| Value::String(p.clone())),
        );
        map.insert(
            "principal_collision".to_owned(),
            self.principal_collision.clone().unwrap_or(Value::Null),
        );
        map.insert(
            "merge_summary".to_owned(),
            self.merge_summary.clone().unwrap_or(Value::Null),
        );
        map.insert(
            "attempt".to_owned(),
            self.attempt.as_ref().map_or(Value::Null, |a| {
                serde_json::to_value(a).unwrap_or(Value::Null)
            }),
        );
        map
    }
}

/// Project an import directory's durable records into an authoritative `ImportProjection`.
///
/// This read-only function inspects:
/// - `imports/<import_id>/import.json` (metadata & attempt facts)
/// - `imports/<import_id>/imported.json` (`PublicationRecord`)
/// - `imports/<import_id>/manifest.json` (Image import manifest)
/// - `imports/<import_id>/content_manifest.jsonl` (Document content manifest)
#[must_use]
pub fn project_import_result(journal_root: &Path, import_id: &str) -> ImportProjection {
    let now_sec = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64();
    project_import_result_with_clock(journal_root, import_id, now_sec)
}

#[must_use]
pub fn project_import_result_with_clock(
    journal_root: &Path,
    import_id: &str,
    now_sec: f64,
) -> ImportProjection {
    let import_dir = journal_root.join("imports").join(import_id);
    let (metadata, raw_metadata, metadata_corrupted) =
        match read_provenance(journal_root, import_id) {
            Ok(Some(meta)) => {
                let raw = Value::Object(meta.clone());
                (Some(meta), Some(raw), false)
            }
            Ok(None) => (None, None, false),
            Err(_) => (None, None, true),
        };

    let imported_path = import_dir.join("imported.json");
    let (imported_raw_value, imported_json_corrupted) =
        if path_lexists(&imported_path).unwrap_or(false) {
            match fs::read(&imported_path) {
                Ok(bytes) => match serde_json::from_slice::<Value>(&bytes) {
                    Ok(val) => (Some(val), false),
                    Err(_) => (None, true),
                },
                Err(_) => (None, true),
            }
        } else {
            (None, false)
        };

    // Read manifest.json if present
    let manifest_path = import_dir.join("manifest.json");
    let (manifest_value, manifest_corrupted) = if manifest_path.is_file() {
        match fs::read(&manifest_path) {
            Ok(bytes) => match serde_json::from_slice::<Value>(&bytes) {
                Ok(val) => (Some(val), false),
                Err(_) => (None, true),
            },
            Err(_) => (None, true),
        }
    } else {
        (None, false)
    };

    let attempt_read = metadata.as_ref().map(read_attempt_facts);
    let attempt_corrupted = matches!(attempt_read, Some(AttemptRead::Malformed));
    let is_corrupted =
        metadata_corrupted || imported_json_corrupted || attempt_corrupted || manifest_corrupted;

    let publication = imported_raw_value
        .as_ref()
        .and_then(|v| serde_json::from_value::<PublicationRecord>(v.clone()).ok());
    let raw_publication = imported_raw_value.clone();

    let attempt = match attempt_read {
        Some(AttemptRead::Present(facts)) => Some(facts),
        _ => None,
    };
    let generation = attempt.as_ref().map(|a| a.generation);
    let attempt_id_str = attempt.as_ref().map(|a| a.attempt_id.clone());

    // Read content_manifest.jsonl if present
    let content_manifest_path = import_dir.join("content_manifest.jsonl");
    let mut content_manifest_lines: Vec<Value> = Vec::new();
    let mut content_manifest_corrupted = false;
    if content_manifest_path.is_file() {
        match fs::read_to_string(&content_manifest_path) {
            Ok(content) => {
                for line in content.lines() {
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    match serde_json::from_str::<Value>(trimmed) {
                        Ok(val) => content_manifest_lines.push(val),
                        Err(_) => {
                            content_manifest_corrupted = true;
                            break;
                        }
                    }
                }
            }
            Err(_) => {
                content_manifest_corrupted = true;
            }
        }
    }
    if content_manifest_corrupted {
        content_manifest_lines.clear();
    }

    // Determine source_type and display name
    let raw_pub_obj = imported_raw_value.as_ref().and_then(Value::as_object);
    let source_type = derive_source_type(
        metadata.as_ref(),
        publication.as_ref(),
        raw_pub_obj,
        manifest_value.as_ref(),
        &content_manifest_lines,
    );
    let source_display = derive_source_display(&source_type);

    // Extract metadata fields
    let task_id = metadata
        .as_ref()
        .and_then(|m| m.get("task_id").and_then(Value::as_str))
        .map(ToOwned::to_owned);
    let upload_timestamp = metadata.as_ref().and_then(|m| {
        m.get("upload_timestamp").and_then(|v| {
            v.as_f64()
                .or_else(|| v.as_u64().map(|n| n as f64))
                .or_else(|| v.as_i64().map(|n| n as f64))
        })
    });
    let original_filename = metadata
        .as_ref()
        .and_then(|m| m.get("original_filename").and_then(Value::as_str))
        .map(ToOwned::to_owned);
    let file_size = metadata
        .as_ref()
        .and_then(|m| m.get("file_size").and_then(Value::as_u64));
    let mime_type = metadata
        .as_ref()
        .and_then(|m| m.get("mime_type").and_then(Value::as_str))
        .map(ToOwned::to_owned);
    let setting = metadata
        .as_ref()
        .and_then(|m| m.get("setting").and_then(Value::as_str))
        .map(ToOwned::to_owned);
    let staged_path = metadata
        .as_ref()
        .and_then(|m| m.get("file_path").and_then(Value::as_str))
        .map(ToOwned::to_owned);
    let principal_collision = metadata
        .as_ref()
        .and_then(|m| m.get("principal_collision").cloned());
    let merge_summary = metadata
        .as_ref()
        .and_then(|m| m.get("merge_summary").cloned());

    // Extract gaps from attempt or manifests
    let unavailable_description = attempt
        .as_ref()
        .and_then(|a| a.unavailable_description.clone())
        .or_else(|| {
            metadata.as_ref().and_then(|m| {
                m.get("unavailable_description")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned)
            })
        });

    let unavailable_pages = {
        if content_manifest_corrupted || content_manifest_lines.is_empty() {
            None
        } else {
            let mut sum = 0u64;
            let mut all_present = true;
            for entry in &content_manifest_lines {
                if let Some(pages) = entry
                    .get("meta")
                    .and_then(|meta| meta.get("unavailable_pages"))
                    .and_then(Value::as_u64)
                {
                    sum = sum.saturating_add(pages);
                } else {
                    all_present = false;
                    break;
                }
            }
            if all_present { Some(sum) } else { None }
        }
    };

    let has_gaps = content_manifest_corrupted
        || unavailable_description.is_some()
        || unavailable_pages.is_some_and(|pages| pages > 0)
        || attempt
            .as_ref()
            .and_then(|facts| facts.input_failures)
            .is_some_and(|failed| failed > 0);
    let created_at = fs::metadata(&import_dir)
        .and_then(|m| m.created().or_else(|_| m.modified()))
        .map(|t| {
            t.duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs_f64()
        })
        // An unreadable directory has no age: treat it as new rather than as instantly timed out.
        .unwrap_or(now_sec);
    let imported_at = upload_timestamp.map(|ms| ms / 1000.0).unwrap_or(created_at);

    // Determine status & error
    let (status, error, error_stage) = derive_status_and_errors(
        metadata.as_ref(),
        publication.as_ref(),
        raw_publication.as_ref(),
        attempt.as_ref(),
        is_corrupted,
        now_sec,
        created_at,
    );

    // Compute metrics
    let duration_ms = attempt.as_ref().and_then(|a| a.duration_ms).or_else(|| {
        metadata
            .as_ref()
            .and_then(|m| m.get("duration_ms").and_then(Value::as_u64))
    });

    let mut metrics = derive_metrics(
        metadata.as_ref(),
        publication.as_ref(),
        raw_publication.as_ref(),
        manifest_value.as_ref(),
        &content_manifest_lines,
    );
    if content_manifest_corrupted || is_corrupted {
        metrics.entries_written = None;
        metrics.total_files_created = None;
    }

    // Entities seeded: 0 for image/pdf/document; None if unknown
    let entities_seeded = if is_corrupted {
        None
    } else {
        match source_type.as_str() {
            "image" | "document" => Some(0),
            _ => metadata
                .as_ref()
                .and_then(|m| m.get("entities_seeded").and_then(Value::as_u64)),
        }
    };

    ImportProjection {
        import_id: import_id.to_owned(),
        source_type,
        source_display,
        status,
        error,
        error_stage,
        created_at,
        imported_at,
        entries_written: metrics.entries_written,
        entities_seeded,
        total_files_created: metrics.total_files_created,
        duration_ms,
        date_range: metrics.date_range,
        days_affected: metrics.days_affected,
        target_day: metrics.target_day,
        unavailable_description,
        unavailable_pages,
        has_gaps,
        task_id,
        upload_timestamp,
        original_filename,
        file_size,
        mime_type,
        setting,
        staged_path,
        principal_collision,
        merge_summary,
        attempt,
        generation,
        attempt_id: attempt_id_str,
        raw_metadata,
        raw_publication,
    }
}

fn derive_source_type(
    metadata: Option<&Map<String, Value>>,
    publication: Option<&PublicationRecord>,
    raw_publication: Option<&Map<String, Value>>,
    manifest: Option<&Value>,
    content_manifest_lines: &[Value],
) -> String {
    if let Some(st) = manifest
        .and_then(|m| m.get("source_type"))
        .and_then(Value::as_str)
    {
        return st.to_owned();
    }
    if let Some(t) = content_manifest_lines
        .first()
        .and_then(|first| first.get("type"))
        .and_then(Value::as_str)
    {
        return t.to_owned();
    }
    if let Some(pub_rec) = publication {
        for seg in &pub_rec.segments {
            if let Some(suffix) = seg.stream.strip_prefix("import.") {
                return suffix.to_owned();
            }
        }
    }
    if let Some(raw_pub) = raw_publication {
        if let Some(st) = raw_pub.get("source_type").and_then(Value::as_str) {
            return st.to_owned();
        }
        if let Some(imp) = raw_pub.get("importer").and_then(Value::as_str) {
            return imp.to_owned();
        }
    }
    if let Some(meta) = metadata {
        if let Some(st) = meta.get("source_type").and_then(Value::as_str) {
            return st.to_owned();
        }
        if let Some(src) = meta.get("source").and_then(Value::as_str) {
            return src.to_owned();
        }
        // The native producer records the registry source it was started for.
        if let Some(hint) = meta
            .get("source_hint")
            .and_then(Value::as_str)
            .filter(|hint| {
                !hint.is_empty() && hint.chars().all(|c| c.is_ascii_lowercase() || c == '_')
            })
        {
            return hint.to_owned();
        }
        if let Some(mime) = meta.get("mime_type").and_then(Value::as_str) {
            if mime.starts_with("image/") {
                return "image".to_owned();
            }
            if mime == "application/pdf" {
                return "document".to_owned();
            }
        }
        if let Some(name) = meta.get("original_filename").and_then(Value::as_str) {
            if name.ends_with(".png")
                || name.ends_with(".jpg")
                || name.ends_with(".jpeg")
                || name.ends_with(".webp")
                || name.ends_with(".gif")
            {
                return "image".to_owned();
            }
            if name.ends_with(".pdf") {
                return "document".to_owned();
            }
            if name.ends_with(".zip") {
                return "archive".to_owned();
            }
        }
    }
    "import".to_owned()
}

fn derive_source_display(source_type: &str) -> String {
    match source_type {
        "image" => "Image".to_owned(),
        "document" => "PDF Document".to_owned(),
        "archive" => "Journal Archive".to_owned(),
        "chatgpt" => "ChatGPT".to_owned(),
        "claude" => "Claude".to_owned(),
        "gemini" => "Gemini".to_owned(),
        "kindle" => "Kindle".to_owned(),
        "obsidian" => "Obsidian".to_owned(),
        "oura" => "Oura".to_owned(),
        "plaud" => "Plaud".to_owned(),
        "audio" => "audio".to_owned(),
        _ => {
            let mut chars = source_type.chars();
            match chars.next() {
                None => "Import".to_owned(),
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
            }
        }
    }
}

fn derive_status_and_errors(
    metadata: Option<&Map<String, Value>>,
    publication: Option<&PublicationRecord>,
    raw_publication: Option<&Value>,
    attempt: Option<&AttemptFacts>,
    is_corrupted: bool,
    now_sec: f64,
    created_at: f64,
) -> (ProjectionStatus, Option<String>, Option<String>) {
    if is_corrupted {
        return (
            ProjectionStatus::Unavailable,
            Some("import status unavailable".to_owned()),
            Some("storage".to_owned()),
        );
    }

    if metadata.is_none() && publication.is_none() && raw_publication.is_none() {
        return (
            ProjectionStatus::Unavailable,
            Some("import status unavailable".to_owned()),
            Some("storage".to_owned()),
        );
    }

    if let Some(att) = attempt {
        match att.state {
            AttemptState::Running => {
                if let Some(pub_rec) = publication {
                    if pub_rec.status == PublicationStatus::Failure {
                        return (
                            ProjectionStatus::Failed,
                            Some(IMPORT_FAILED_REASON.to_owned()),
                            Some("publication".to_owned()),
                        );
                    }
                    if pub_rec.status == PublicationStatus::Success {
                        return (
                            ProjectionStatus::Unconfirmed,
                            Some(IMPORT_UNCONFIRMED_REASON.to_owned()),
                            Some("finalization".to_owned()),
                        );
                    }
                }
                // 1h wall-clock Running bound with no heartbeat: a slow but live
                // PDF import older than this flips to Unconfirmed.
                let now_ms = (now_sec * 1000.0) as u64;
                if now_ms.saturating_sub(att.started_at_ms)
                    > crate::metadata::RUNNING_ATTEMPT_BOUND_MS
                {
                    (
                        ProjectionStatus::Unconfirmed,
                        Some(IMPORT_UNCONFIRMED_REASON.to_owned()),
                        Some("timeout".to_owned()),
                    )
                } else {
                    (ProjectionStatus::Running, None, None)
                }
            }
            AttemptState::Unconfirmed => {
                // The producer records Unconfirmed both when it was interrupted and when
                // publication definitively failed; the publication record tells them apart.
                if publication.is_some_and(|record| record.status == PublicationStatus::Failure) {
                    return (
                        ProjectionStatus::Failed,
                        Some(IMPORT_FAILED_REASON.to_owned()),
                        Some("publication".to_owned()),
                    );
                }
                // A producer that knew the import had failed says so with the fixed reason;
                // anything else (an interruption, a completion it could not record, text an
                // earlier build stored) is unconfirmed, and the stored text is never shown.
                if att.failure_reason.as_deref() == Some(IMPORT_FAILED_REASON) {
                    return (
                        ProjectionStatus::Failed,
                        Some(IMPORT_FAILED_REASON.to_owned()),
                        Some("execution".to_owned()),
                    );
                }
                (
                    ProjectionStatus::Unconfirmed,
                    Some(IMPORT_UNCONFIRMED_REASON.to_owned()),
                    Some("finalization".to_owned()),
                )
            }
            AttemptState::Completed => {
                if let Some(pub_rec) = publication {
                    match pub_rec.status {
                        PublicationStatus::Success => (ProjectionStatus::Success, None, None),
                        PublicationStatus::Failure => (
                            ProjectionStatus::Failed,
                            Some(IMPORT_FAILED_REASON.to_owned()),
                            Some("publication".to_owned()),
                        ),
                    }
                } else if let Some(raw) = raw_publication {
                    if let Some(err) = raw.get("error").filter(|v| !v.is_null()) {
                        let err_str = err.as_str().unwrap_or("import failed").to_owned();
                        let stage = raw
                            .get("error_stage")
                            .and_then(Value::as_str)
                            .map(ToOwned::to_owned);
                        (ProjectionStatus::Failed, Some(err_str), stage)
                    } else if raw.get("processed") == Some(&Value::Bool(true)) {
                        (ProjectionStatus::Success, None, None)
                    } else if att.failure_reason.is_some() {
                        (
                            ProjectionStatus::Failed,
                            Some(IMPORT_FAILED_REASON.to_owned()),
                            None,
                        )
                    } else {
                        (
                            ProjectionStatus::Unconfirmed,
                            Some("import status unavailable".to_owned()),
                            Some("publication".to_owned()),
                        )
                    }
                } else if att.failure_reason.is_some() {
                    (
                        ProjectionStatus::Failed,
                        Some(IMPORT_FAILED_REASON.to_owned()),
                        None,
                    )
                } else {
                    (ProjectionStatus::Success, None, None)
                }
            }
        }
    } else {
        // Historical / legacy import without attempt facts
        if let Some(err_val) = metadata
            .and_then(|meta| meta.get("error"))
            .filter(|v| !v.is_null())
        {
            let err_str = err_val.as_str().unwrap_or("import failed").to_owned();
            let err_stage = metadata
                .and_then(|meta| meta.get("error_stage"))
                .and_then(Value::as_str)
                .map(ToOwned::to_owned);
            return (ProjectionStatus::Failed, Some(err_str), err_stage);
        }

        if let Some(pub_rec) = publication {
            match pub_rec.status {
                PublicationStatus::Success => (ProjectionStatus::Success, None, None),
                PublicationStatus::Failure => (
                    ProjectionStatus::Failed,
                    Some("import failed".to_owned()),
                    Some("publication".to_owned()),
                ),
            }
        } else if let Some(raw) = raw_publication {
            if let Some(err) = raw.get("error").filter(|v| !v.is_null()) {
                let err_str = err.as_str().unwrap_or("import failed").to_owned();
                let stage = raw
                    .get("error_stage")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned);
                (ProjectionStatus::Failed, Some(err_str), stage)
            } else if raw.get("processed") == Some(&Value::Bool(true))
                || raw.get("processing_completed") == Some(&Value::Bool(true))
                || raw.get("status") == Some(&Value::String("success".to_owned()))
            {
                (ProjectionStatus::Success, None, None)
            } else if raw.get("status") == Some(&Value::String("failure".to_owned()))
                || raw.get("processed") == Some(&Value::Bool(false))
            {
                (
                    ProjectionStatus::Failed,
                    Some("import failed".to_owned()),
                    None,
                )
            } else {
                (
                    ProjectionStatus::Unavailable,
                    Some("import status unavailable".to_owned()),
                    Some("publication".to_owned()),
                )
            }
        } else if let Some(meta) = metadata {
            let processed = meta
                .get("processed")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let completed = meta
                .get("processing_completed")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            if processed || completed {
                (ProjectionStatus::Success, None, None)
            } else if meta
                .get("task_id")
                .is_some_and(|v| !v.is_null() && !v.as_str().is_some_and(|s| s.is_empty()))
            {
                let imported_at = meta
                    .get("upload_timestamp")
                    .and_then(|v| {
                        v.as_f64()
                            .or_else(|| v.as_u64().map(|n| n as f64))
                            .or_else(|| v.as_i64().map(|n| n as f64))
                    })
                    .map(|ms| ms / 1000.0)
                    .unwrap_or(created_at);
                // 1h wall-clock bound: legacy unfinalized task rows time out after 3600 seconds
                #[allow(clippy::cast_precision_loss)]
                let bound_secs = crate::metadata::RUNNING_ATTEMPT_BOUND_MS as f64 / 1000.0;
                if now_sec - imported_at > bound_secs {
                    (
                        ProjectionStatus::Failed,
                        Some("Import never completed".to_owned()),
                        Some("timeout".to_owned()),
                    )
                } else {
                    (ProjectionStatus::Running, None, None)
                }
            } else {
                (ProjectionStatus::Pending, None, None)
            }
        } else {
            (ProjectionStatus::Unavailable, None, None)
        }
    }
}

struct DerivedMetrics {
    entries_written: Option<u64>,
    total_files_created: Option<u64>,
    days_affected: Vec<String>,
    date_range: Option<(String, String)>,
    target_day: Option<String>,
}

fn derive_metrics(
    metadata: Option<&Map<String, Value>>,
    publication: Option<&PublicationRecord>,
    raw_publication: Option<&Value>,
    manifest: Option<&Value>,
    content_manifest_lines: &[Value],
) -> DerivedMetrics {
    let mut entries_written: Option<u64> = None;
    let mut total_files_created: Option<u64> = None;
    let mut days: Vec<String> = Vec::new();

    if let Some(m) = manifest {
        if let Some(count) = m.get("entry_count").and_then(Value::as_u64) {
            entries_written = Some(count);
        }
        if let Some(arr) = m.get("files_created").and_then(Value::as_array) {
            total_files_created = Some(arr.len() as u64);
        }
        if let Some(days_arr) = m.get("days_affected").and_then(Value::as_array) {
            for day in days_arr {
                if let Some(s) = day.as_str().filter(|s| !days.iter().any(|d| d == *s)) {
                    days.push(s.to_owned());
                }
            }
        }
    }

    if !content_manifest_lines.is_empty() {
        entries_written = Some(content_manifest_lines.len() as u64);
        for entry in content_manifest_lines {
            if let Some(segments) = entry.get("segments").and_then(Value::as_array) {
                for seg in segments {
                    if let Some(day) = seg
                        .get("day")
                        .and_then(Value::as_str)
                        .filter(|d| !days.iter().any(|x| x == *d))
                    {
                        days.push(day.to_owned());
                    }
                }
            }
            if let Some(date) = entry
                .get("date")
                .and_then(Value::as_str)
                .filter(|d| !days.iter().any(|x| x == *d))
            {
                days.push(date.to_owned());
            }
        }
    }

    if let Some(pub_rec) = publication {
        for seg in &pub_rec.segments {
            if !days.contains(&seg.day) {
                days.push(seg.day.clone());
            }
        }
        // Any publication with segments counts them when nothing more specific did. This
        // was an image/document allow-list, which meant every new source had to remember to
        // add itself to a list it has no reason to know about -- and a source that forgot
        // rendered a successful row with a blank count.
        if entries_written.is_none() {
            entries_written = Some(pub_rec.segments.len() as u64);
        }
    }

    // Fall back to raw_publication / metadata legacy fields if still not set
    if let Some(raw) = raw_publication {
        if entries_written.is_none() {
            entries_written = raw
                .get("entries_written")
                .and_then(Value::as_u64)
                .or_else(|| raw.get("files_written").and_then(Value::as_u64));
        }
        if total_files_created.is_none() {
            total_files_created = raw
                .get("total_files_created")
                .and_then(Value::as_u64)
                .or_else(|| raw.get("files_written").and_then(Value::as_u64));
        }
        if days.is_empty() {
            for day in raw
                .get("days")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                if let Some(s) = day.as_str().filter(|s| !days.iter().any(|d| d == *s)) {
                    days.push(s.to_owned());
                }
            }
        }
    }

    if entries_written.is_none() {
        entries_written =
            metadata.and_then(|meta| meta.get("entries_written").and_then(Value::as_u64));
    }
    if total_files_created.is_none() {
        total_files_created =
            metadata.and_then(|meta| meta.get("total_files_created").and_then(Value::as_u64));
    }

    days.sort();
    let date_range = if days.is_empty() {
        None
    } else {
        Some((days.first().unwrap().clone(), days.last().unwrap().clone()))
    };
    let target_day = days.first().cloned();

    DerivedMetrics {
        entries_written,
        total_files_created,
        days_affected: days,
        date_range,
        target_day,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_publication_failure_yields_failed_status() {
        let dir = tempdir().unwrap();
        let journal = dir.path();
        let import_id = "20260101_120000";
        let import_dir = journal.join("imports").join(import_id);
        fs::create_dir_all(&import_dir).unwrap();

        // Write import.json with completed attempt
        let metadata = serde_json::json!({
            "original_filename": "test.png",
            "attempt": {
                "attempt_id": import_id,
                "generation": 1,
                "state": "completed",
                "started_at_ms": 1000,
                "finished_at_ms": 2000,
                "duration_ms": 1000
            }
        });
        fs::write(
            import_dir.join("import.json"),
            serde_json::to_vec_pretty(&metadata).unwrap(),
        )
        .unwrap();

        // Write publication record with Failure
        let publication = serde_json::json!({
            "schema": "solstone.import.publication.v1",
            "status": "failure",
            "segments": [],
            "indexing": { "published": [], "declined": [], "errored": [] },
            "day_markers": []
        });
        fs::write(
            import_dir.join("imported.json"),
            serde_json::to_vec_pretty(&publication).unwrap(),
        )
        .unwrap();

        let projection = project_import_result(journal, import_id);
        assert_eq!(projection.status, ProjectionStatus::Failed);
    }

    fn write_attempt(journal: &Path, id: &str, state: &str, started_at_ms: u64) {
        let dir = journal.join("imports").join(id);
        fs::create_dir_all(&dir).unwrap();
        let metadata = serde_json::json!({
            "original_filename": "test.png",
            "task_id": id,
            "attempt": {
                "attempt_id": format!("{id}:1"),
                "generation": 1,
                "state": state,
                "started_at_ms": started_at_ms,
                "failure_reason": "publication failed"
            }
        });
        fs::write(
            dir.join("import.json"),
            serde_json::to_vec(&metadata).unwrap(),
        )
        .unwrap();
    }

    fn write_publication(journal: &Path, id: &str, status: &str) {
        let publication = serde_json::json!({
            "schema": "solstone.import.publication.v1",
            "status": status,
            "segments": [],
            "indexing": { "published": [], "declined": [], "errored": [] },
            "day_markers": []
        });
        fs::write(
            journal.join("imports").join(id).join("imported.json"),
            serde_json::to_vec(&publication).unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn an_unconfirmed_attempt_over_a_failed_publication_is_failed_not_unconfirmed() {
        let dir = tempdir().unwrap();
        let (journal, id) = (dir.path(), "20260101_120001");
        write_attempt(journal, id, "unconfirmed", 1000);
        write_publication(journal, id, "failure");
        let projection = project_import_result(journal, id);
        assert_eq!(projection.status, ProjectionStatus::Failed);
        assert_eq!(projection.error_stage.as_deref(), Some("publication"));

        // Control: an interrupted producer (no publication record) is still unconfirmed.
        let other = "20260101_120002";
        write_attempt(journal, other, "unconfirmed", 1000);
        assert_eq!(
            project_import_result(journal, other).status,
            ProjectionStatus::Unconfirmed
        );
    }

    #[test]
    fn inputs_that_failed_while_others_imported_read_as_gaps_after_a_reload() {
        let dir = tempdir().unwrap();
        let (journal, id) = (dir.path(), "20260101_120004");
        write_attempt(journal, id, "running", 1000);
        crate::metadata::record_completed_attempt_with_input_failures_unlocked(
            journal,
            id,
            1,
            2000,
            Some(1000),
            2,
        )
        .unwrap();
        write_publication(journal, id, "success");
        let projection = project_import_result(journal, id);
        assert_eq!(projection.status, ProjectionStatus::Success);
        assert!(projection.has_gaps, "{projection:?}");
        assert_eq!(projection.native_row_overlay()["input_failures"], 2);

        // Control: a clean completion has no gap and no input_failures key.
        let clean = "20260101_120005";
        write_attempt(journal, clean, "running", 1000);
        crate::metadata::record_completed_attempt_unlocked(
            journal,
            clean,
            1,
            2000,
            Some(1000),
            None,
        )
        .unwrap();
        write_publication(journal, clean, "success");
        let clean_projection = project_import_result(journal, clean);
        assert!(!clean_projection.has_gaps, "{clean_projection:?}");
        assert!(
            clean_projection
                .native_row_overlay()
                .get("input_failures")
                .is_none()
        );
    }

    fn write_attempt_with_reason(journal: &Path, id: &str, state: &str, reason: &str) {
        let dir = journal.join("imports").join(id);
        fs::create_dir_all(&dir).unwrap();
        let metadata = serde_json::json!({
            "original_filename": "test.png",
            "source_hint": "image",
            "attempt": {
                "attempt_id": format!("{id}:1"),
                "generation": 1,
                "state": state,
                "started_at_ms": 1000,
                "failure_reason": reason
            }
        });
        fs::write(
            dir.join("import.json"),
            serde_json::to_vec(&metadata).unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn a_definitive_failure_reads_failed_and_no_stored_text_reaches_the_owner() {
        let dir = tempdir().unwrap();
        let journal = dir.path();

        // A producer that knew it failed records the fixed reason: failed, with that reason.
        write_attempt_with_reason(
            journal,
            "20260101_120010",
            "unconfirmed",
            IMPORT_FAILED_REASON,
        );
        let failed = project_import_result(journal, "20260101_120010");
        assert_eq!(failed.status, ProjectionStatus::Failed, "{failed:?}");
        assert_eq!(failed.error.as_deref(), Some(IMPORT_FAILED_REASON));

        // Text an earlier build stored (a raw diagnostic with a path) is unconfirmed and is
        // never shown: the owner sees the fixed sentence instead.
        let raw = "cannot write /home/owner/journal/chronicle/20260101/health/stream.updated";
        write_attempt_with_reason(journal, "20260101_120011", "unconfirmed", raw);
        let unconfirmed = project_import_result(journal, "20260101_120011");
        assert_eq!(
            unconfirmed.status,
            ProjectionStatus::Unconfirmed,
            "{unconfirmed:?}"
        );
        assert_eq!(
            unconfirmed.error.as_deref(),
            Some(IMPORT_UNCONFIRMED_REASON)
        );
        assert!(!unconfirmed.error.unwrap_or_default().contains('/'));

        // A completed attempt carrying stored failure text also shows only the fixed reason.
        write_attempt_with_reason(journal, "20260101_120012", "completed", raw);
        let completed = project_import_result(journal, "20260101_120012");
        assert_eq!(completed.status, ProjectionStatus::Failed, "{completed:?}");
        assert_eq!(completed.error.as_deref(), Some(IMPORT_FAILED_REASON));
    }

    #[test]
    fn an_attempt_only_row_names_its_source_and_carries_its_day() {
        let dir = tempdir().unwrap();
        let (journal, id) = (dir.path(), "20260101_120013");
        write_attempt(journal, id, "running", 1000);
        // The attempt-only import.json the CLI path writes has a source hint, no source_type.
        let path = journal.join("imports").join(id).join("import.json");
        let mut metadata: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        metadata["source_hint"] = serde_json::json!("image");
        fs::write(&path, serde_json::to_vec(&metadata).unwrap()).unwrap();
        let projection = project_import_result(journal, id);
        assert_eq!(projection.source_type, "image", "{projection:?}");

        let overlay = ImportProjection {
            target_day: Some("20260101".to_owned()),
            ..projection
        }
        .native_row_overlay();
        assert_eq!(overlay["target_day"], "20260101");
        assert!(
            overlay.get("source_display").is_none(),
            "display names are the route's"
        );
    }

    #[test]
    fn a_running_attempt_is_bounded_server_side_by_the_wall_clock() {
        let dir = tempdir().unwrap();
        let (journal, id) = (dir.path(), "20260101_120003");
        let started_ms = 1_000_000_000_u64;
        write_attempt(journal, id, "running", started_ms);
        let started_s = started_ms as f64 / 1000.0;
        // Just inside the bound: still running. Just past it: unconfirmed, with the timeout copy.
        let inside = project_import_result_with_clock(journal, id, started_s + 3_599.0);
        assert_eq!(inside.status, ProjectionStatus::Running, "{inside:?}");
        let past = project_import_result_with_clock(journal, id, started_s + 3_601.0);
        assert_eq!(past.status, ProjectionStatus::Unconfirmed, "{past:?}");
        assert_eq!(
            past.error.as_deref(),
            Some("this import couldn't be confirmed as finished.")
        );
        assert_eq!(past.error_stage.as_deref(), Some("timeout"));
    }

    #[test]
    fn test_unknown_vs_zero_json_encoding() {
        let projection = ImportProjection {
            import_id: "20260101_120000".to_owned(),
            source_type: "image".to_owned(),
            source_display: "Image".to_owned(),
            status: ProjectionStatus::Success,
            error: None,
            error_stage: None,
            created_at: 0.0,
            imported_at: 0.0,
            entries_written: Some(0),  // Measured 0
            entities_seeded: Some(0),  // Measured 0
            total_files_created: None, // Unknown
            duration_ms: None,         // Unknown
            date_range: None,
            days_affected: vec![],
            target_day: None,
            unavailable_description: None,
            unavailable_pages: None,
            has_gaps: false,
            task_id: None,
            upload_timestamp: None,
            original_filename: None,
            file_size: None,
            mime_type: None,
            setting: None,
            staged_path: None,
            principal_collision: None,
            merge_summary: None,
            attempt: None,
            generation: None,
            attempt_id: None,
            raw_metadata: None,
            raw_publication: None,
        };

        let map = projection.to_json_map();
        assert_eq!(map["entries_written"], serde_json::json!(0));
        assert_eq!(map["entities_seeded"], serde_json::json!(0));
        assert_eq!(map["total_files_created"], Value::Null);
        assert_eq!(map["duration_ms"], Value::Null);
    }

    #[test]
    fn test_corrupted_metadata_yields_unavailable_even_with_publication_success() {
        let dir = tempdir().unwrap();
        let journal = dir.path();
        let import_id = "20260101_140000";
        let import_dir = journal.join("imports").join(import_id);
        fs::create_dir_all(&import_dir).unwrap();

        // Write corrupt import.json
        fs::write(import_dir.join("import.json"), b"corrupted { not json").unwrap();

        // Write publication record with Success
        let publication = serde_json::json!({
            "schema": "solstone.import.publication.v1",
            "status": "success",
            "segments": [],
            "indexing": { "published": [], "declined": [], "errored": [] },
            "day_markers": []
        });
        fs::write(
            import_dir.join("imported.json"),
            serde_json::to_vec_pretty(&publication).unwrap(),
        )
        .unwrap();

        let projection = project_import_result(journal, import_id);
        assert_eq!(projection.status, ProjectionStatus::Unavailable);
        assert_eq!(projection.error_stage.as_deref(), Some("storage"));
    }

    #[test]
    fn test_corrupted_content_manifest_fails_closed() {
        let dir = tempdir().unwrap();
        let journal = dir.path();
        let import_id = "20260101_150000";
        let import_dir = journal.join("imports").join(import_id);
        fs::create_dir_all(&import_dir).unwrap();

        let metadata = serde_json::json!({
            "original_filename": "doc.pdf",
            "attempt": {
                "attempt_id": format!("{import_id}:1"),
                "generation": 1,
                "state": "completed",
                "started_at_ms": 1000,
                "finished_at_ms": 2000,
                "duration_ms": 1000
            }
        });
        fs::write(
            import_dir.join("import.json"),
            serde_json::to_vec_pretty(&metadata).unwrap(),
        )
        .unwrap();

        let publication = serde_json::json!({
            "schema": "solstone.import.publication.v1",
            "status": "success",
            "segments": [],
            "indexing": { "published": [], "declined": [], "errored": [] },
            "day_markers": []
        });
        fs::write(
            import_dir.join("imported.json"),
            serde_json::to_vec_pretty(&publication).unwrap(),
        )
        .unwrap();

        // Write content_manifest with one valid line and one corrupt line
        fs::write(
            import_dir.join("content_manifest.jsonl"),
            b"{\"type\": \"document\", \"meta\": {\"unavailable_pages\": 2}}\nnot valid json line\n",
        )
        .unwrap();

        let projection = project_import_result(journal, import_id);
        assert!(projection.has_gaps);
        assert_eq!(projection.unavailable_pages, None);
        assert_eq!(projection.entries_written, None);
    }

    #[test]
    fn test_corrupted_imported_json_yields_unavailable() {
        let dir = tempdir().unwrap();
        let journal = dir.path();
        let import_id = "20260101_160000";
        let import_dir = journal.join("imports").join(import_id);
        fs::create_dir_all(&import_dir).unwrap();

        let metadata = serde_json::json!({
            "original_filename": "doc.pdf",
            "attempt": {
                "attempt_id": format!("{import_id}:1"),
                "generation": 1,
                "state": "completed",
                "started_at_ms": 1000,
                "finished_at_ms": 2000,
                "duration_ms": 1000
            }
        });
        fs::write(
            import_dir.join("import.json"),
            serde_json::to_vec_pretty(&metadata).unwrap(),
        )
        .unwrap();

        fs::write(import_dir.join("imported.json"), b"{ invalid json").unwrap();

        let projection = project_import_result(journal, import_id);
        assert_eq!(projection.status, ProjectionStatus::Unavailable);
        assert_eq!(projection.entries_written, None);
        assert_eq!(projection.entities_seeded, None);
    }

    #[test]
    fn test_empty_object_imported_json_legacy_yields_unavailable_not_success() {
        let dir = tempdir().unwrap();
        let journal = dir.path();
        let import_id = "20260101_170000";
        let import_dir = journal.join("imports").join(import_id);
        fs::create_dir_all(&import_dir).unwrap();

        // Historical import without attempt
        fs::write(import_dir.join("imported.json"), b"{}").unwrap();

        let projection = project_import_result(journal, import_id);
        assert_eq!(projection.status, ProjectionStatus::Unavailable);
    }

    #[test]
    fn test_content_manifest_missing_unavailable_pages_yields_none_sum() {
        let dir = tempdir().unwrap();
        let journal = dir.path();
        let import_id = "20260101_180000";
        let import_dir = journal.join("imports").join(import_id);
        fs::create_dir_all(&import_dir).unwrap();

        let metadata = serde_json::json!({
            "original_filename": "doc.pdf",
            "attempt": {
                "attempt_id": format!("{import_id}:1"),
                "generation": 1,
                "state": "completed",
                "started_at_ms": 1000,
                "finished_at_ms": 2000,
                "duration_ms": 1000
            }
        });
        fs::write(
            import_dir.join("import.json"),
            serde_json::to_vec_pretty(&metadata).unwrap(),
        )
        .unwrap();

        // Row 1 has unavailable_pages, row 2 does not
        fs::write(
            import_dir.join("content_manifest.jsonl"),
            b"{\"type\": \"document\", \"meta\": {\"unavailable_pages\": 2}}\n{\"type\": \"document\", \"meta\": {}}\n",
        )
        .unwrap();

        let projection = project_import_result(journal, import_id);
        assert_eq!(projection.unavailable_pages, None);
    }

    #[test]
    fn chatgpt_imported_json_source_type_is_not_generic_import() {
        let dir = tempdir().unwrap();
        let journal = dir.path();
        let import_id = "20260804_150000";
        let import_dir = journal.join("imports").join(import_id);
        fs::create_dir_all(&import_dir).unwrap();
        fs::write(
            import_dir.join("import.json"),
            serde_json::to_vec(&serde_json::json!({
                "original_filename": "conversations.json",
                "mime_type": "application/json",
                "source": "corpus"
            }))
            .unwrap(),
        )
        .unwrap();
        fs::write(
            import_dir.join("imported.json"),
            serde_json::to_vec(&serde_json::json!({
                "processed": true,
                "source_type": "chatgpt",
                "source_display": "ChatGPT"
            }))
            .unwrap(),
        )
        .unwrap();
        let projection = project_import_result(journal, import_id);
        assert_eq!(projection.source_type, "chatgpt");
        assert_eq!(projection.source_display, "ChatGPT");
        assert_eq!(projection.status, ProjectionStatus::Success);
    }

    #[test]
    fn attempt_only_completed_archive_projects_success_without_imported_json() {
        let dir = tempdir().unwrap();
        let journal = dir.path();
        let import_id = "20260809_090000";
        let import_dir = journal.join("imports").join(import_id);
        fs::create_dir_all(&import_dir).unwrap();
        fs::write(
            import_dir.join("import.json"),
            serde_json::to_vec(&serde_json::json!({
                "source_type": "journal_archive",
                "attempt": {
                    "attempt_id": format!("{import_id}:1"),
                    "generation": 1,
                    "state": "completed",
                    "started_at_ms": 1_000,
                    "finished_at_ms": 2_000,
                    "duration_ms": 1_000
                }
            }))
            .unwrap(),
        )
        .unwrap();

        let projection = project_import_result(journal, import_id);
        assert_eq!(projection.source_type, "journal_archive");
        assert_eq!(projection.status, ProjectionStatus::Success);
        assert_eq!(projection.error, None);
        assert_eq!(projection.error_stage, None);
    }

    #[test]
    fn attempt_only_failed_archive_projects_failed_without_imported_json() {
        let dir = tempdir().unwrap();
        let journal = dir.path();
        let import_id = "20260809_090001";
        let import_dir = journal.join("imports").join(import_id);
        fs::create_dir_all(&import_dir).unwrap();
        fs::write(
            import_dir.join("import.json"),
            serde_json::to_vec(&serde_json::json!({
                "source_type": "journal_archive",
                "attempt": {
                    "attempt_id": format!("{import_id}:1"),
                    "generation": 1,
                    "state": "unconfirmed",
                    "started_at_ms": 1_000,
                    "finished_at_ms": 2_000,
                    "duration_ms": 1_000,
                    "failure_reason": "import failed"
                }
            }))
            .unwrap(),
        )
        .unwrap();

        let projection = project_import_result(journal, import_id);
        assert_eq!(projection.source_type, "journal_archive");
        assert_eq!(projection.status, ProjectionStatus::Failed);
        assert_eq!(projection.error.as_deref(), Some("import failed"));
    }

    #[test]
    fn attempt_only_unconfirmed_archive_projects_unconfirmed() {
        let dir = tempdir().unwrap();
        let journal = dir.path();
        let import_id = "20260809_090002";
        let import_dir = journal.join("imports").join(import_id);
        fs::create_dir_all(&import_dir).unwrap();
        fs::write(
            import_dir.join("import.json"),
            serde_json::to_vec(&serde_json::json!({
                "source_type": "journal_archive",
                "attempt": {
                    "attempt_id": format!("{import_id}:1"),
                    "generation": 1,
                    "state": "unconfirmed",
                    "started_at_ms": 1_000,
                    "finished_at_ms": 2_000,
                    "duration_ms": 1_000,
                    "failure_reason": "this import couldn't be confirmed as finished."
                }
            }))
            .unwrap(),
        )
        .unwrap();

        let projection = project_import_result(journal, import_id);
        assert_eq!(projection.source_type, "journal_archive");
        assert_eq!(projection.status, ProjectionStatus::Unconfirmed);
        assert_eq!(
            projection.error.as_deref(),
            Some("this import couldn't be confirmed as finished.")
        );
    }
}
