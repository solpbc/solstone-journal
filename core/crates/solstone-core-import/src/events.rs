// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Typed, best-effort Callosum events emitted by native import publication.

use std::path::Path;
use std::time::Duration;

use serde_json::{Map, Value, json};
use solstone_core_callosum::CallosumOneShotSender;

const SOCKET_TIMEOUT: Duration = Duration::from_secs(2);

/// A journal-local, best-effort Callosum event sender.
pub struct EventEmitter<'a> {
    journal: &'a Path,
    revision: Option<&'a str>,
}

impl<'a> EventEmitter<'a> {
    #[must_use]
    pub fn new(journal: &'a Path, revision: Option<&'a str>) -> Self {
        Self { journal, revision }
    }

    fn send(&self, tract: &str, event: &str, mut fields: Map<String, Value>) {
        fields.insert("tract".to_owned(), json!(tract));
        fields.insert("event".to_owned(), json!(event));
        let _ = self.revision;
        let Ok(line) = serde_json::to_string(&fields) else {
            return;
        };
        let line = format!("{line}\n");
        let _ =
            CallosumOneShotSender::new(self.journal.join("health/callosum.sock"), SOCKET_TIMEOUT)
                .send_line(&line);
    }
}

#[derive(Clone, Debug)]
pub struct ImporterStatus {
    pub import_id: String,
    pub stage: String,
    pub elapsed_ms: u64,
    pub stage_elapsed_ms: u64,
    pub items_processed: u64,
    pub items_total: u64,
    pub earliest_date: Option<String>,
    pub latest_date: Option<String>,
    pub entities_found: u64,
    pub source_type: Option<String>,
    pub source_display: Option<String>,
}

#[derive(Clone, Debug)]
pub struct ImporterStarted {
    pub import_id: String,
    pub input_file: String,
    pub file_type: String,
    pub day: String,
    pub facet: Option<String>,
    pub setting: Option<String>,
    pub options: Map<String, Value>,
    pub stage: String,
    pub stream: String,
}

#[derive(Clone, Debug)]
pub struct FileImported {
    pub import_id: String,
    pub importer: String,
    pub entries_written: u64,
    pub entities_seeded: u64,
    pub files_created: u64,
    pub errors: u64,
    pub stream: String,
    pub source_display: String,
    pub date_range: Option<(String, String)>,
    pub merge_summary: Option<Value>,
    pub merge_log_path: Option<String>,
    pub merge_staging_path: Option<String>,
    pub summary_errors: Option<Vec<String>>,
    pub principal_collision: Option<Value>,
}

#[derive(Clone, Debug)]
pub struct ObservedSegment {
    pub segment: String,
    pub day: String,
    pub stream: String,
}

#[derive(Clone, Debug)]
pub struct ObservingMeta {
    pub import_id: String,
    pub stream: String,
    pub facet: Option<String>,
    pub setting: Option<String>,
}

#[derive(Clone, Debug)]
pub struct ObservingSegment {
    pub segment: String,
    pub day: String,
    pub files: Vec<String>,
    pub meta: ObservingMeta,
    pub stream: String,
}

#[derive(Clone, Debug)]
pub struct EnrichmentReady {
    pub import_id: String,
    pub importer: String,
    pub days: Vec<String>,
    pub entries_written: u64,
}

#[derive(Clone, Debug)]
pub struct ImporterCompleted {
    pub import_id: String,
    pub stage: String,
    pub duration_ms: u64,
    pub total_files_created: u64,
    pub output_files: Vec<String>,
    pub metadata_file: String,
    pub stages_run: Vec<String>,
    pub segments: Vec<String>,
    pub stream: String,
    pub source_type: Option<String>,
    pub source_display: Option<String>,
    pub entries_written: u64,
    pub entities_seeded: u64,
    pub date_range: Option<(String, String)>,
}

#[derive(Clone, Debug)]
pub struct ImporterError {
    pub import_id: String,
    pub stage: String,
    pub error: String,
    pub duration_ms: u64,
    pub partial_outputs: Vec<String>,
}

fn object(value: Value) -> Map<String, Value> {
    value
        .as_object()
        .expect("event fields must be a JSON object")
        .clone()
}

pub(crate) fn importer_status_fields(value: &ImporterStatus) -> Map<String, Value> {
    object(json!({
        "import_id": value.import_id,
        "stage": value.stage,
        "elapsed_ms": value.elapsed_ms,
        "stage_elapsed_ms": value.stage_elapsed_ms,
        "items_processed": value.items_processed,
        "items_total": value.items_total,
        "earliest_date": value.earliest_date,
        "latest_date": value.latest_date,
        "entities_found": value.entities_found,
        "source_type": value.source_type,
        "source_display": value.source_display,
    }))
}

pub fn emit_importer_status(emitter: &EventEmitter<'_>, value: &ImporterStatus) {
    emitter.send("importer", "status", importer_status_fields(value));
}

pub(crate) fn importer_started_fields(value: &ImporterStarted) -> Map<String, Value> {
    object(json!({
        "import_id": value.import_id,
        "input_file": value.input_file,
        "file_type": value.file_type,
        "day": value.day,
        "facet": value.facet,
        "setting": value.setting,
        "options": value.options,
        "stage": value.stage,
        "stream": value.stream,
    }))
}

pub fn emit_importer_started(emitter: &EventEmitter<'_>, value: &ImporterStarted) {
    emitter.send("importer", "started", importer_started_fields(value));
}

pub(crate) fn file_imported_fields(value: &FileImported) -> Map<String, Value> {
    let mut fields = object(json!({
        "import_id": value.import_id,
        "importer": value.importer,
        "entries_written": value.entries_written,
        "entities_seeded": value.entities_seeded,
        "files_created": value.files_created,
        "errors": value.errors,
        "stream": value.stream,
        "source_display": value.source_display,
        "date_range": value.date_range,
    }));
    for (name, optional) in [
        ("merge_summary", value.merge_summary.clone()),
        ("principal_collision", value.principal_collision.clone()),
    ] {
        if let Some(field) = optional {
            fields.insert(name.to_owned(), field);
        }
    }
    if let Some(path) = &value.merge_log_path {
        fields.insert("merge_log_path".to_owned(), json!(path));
    }
    if let Some(path) = &value.merge_staging_path {
        fields.insert("merge_staging_path".to_owned(), json!(path));
    }
    if let Some(errors) = &value.summary_errors {
        fields.insert("summary_errors".to_owned(), json!(errors));
    }
    fields
}

pub fn emit_file_imported(emitter: &EventEmitter<'_>, value: &FileImported) {
    emitter.send("importer", "file_imported", file_imported_fields(value));
}

pub(crate) fn observed_fields(value: &ObservedSegment) -> Map<String, Value> {
    object(json!({
        "segment": value.segment,
        "day": value.day,
        "stream": value.stream,
        "batch": true,
    }))
}

pub fn emit_observe_observed(emitter: &EventEmitter<'_>, value: &ObservedSegment) {
    emitter.send("observe", "observed", observed_fields(value));
}

pub fn observing_fields(value: &ObservingSegment) -> Map<String, Value> {
    let mut meta = object(json!({
        "import_id": value.meta.import_id,
        "stream": value.meta.stream,
    }));
    if let Some(facet) = &value.meta.facet {
        meta.insert("facet".to_owned(), json!(facet));
    }
    if let Some(setting) = &value.meta.setting {
        meta.insert("setting".to_owned(), json!(setting));
    }
    object(json!({
        "segment": value.segment,
        "day": value.day,
        "files": value.files,
        "meta": meta,
        "stream": value.stream,
        "batch": true,
    }))
}

pub fn emit_observe_observing(emitter: &EventEmitter<'_>, value: &ObservingSegment) {
    emitter.send("observe", "observing", observing_fields(value));
}

pub(crate) fn enrichment_ready_fields(value: &EnrichmentReady) -> Map<String, Value> {
    object(json!({
        "import_id": value.import_id,
        "importer": value.importer,
        "days": value.days,
        "entries_written": value.entries_written,
    }))
}

pub fn emit_enrichment_ready(emitter: &EventEmitter<'_>, value: &EnrichmentReady) {
    if !value.days.is_empty() {
        emitter.send(
            "importer",
            "enrichment_ready",
            enrichment_ready_fields(value),
        );
    }
}

pub(crate) fn importer_completed_fields(value: &ImporterCompleted) -> Map<String, Value> {
    object(json!({
        "import_id": value.import_id,
        "stage": value.stage,
        "duration_ms": value.duration_ms,
        "total_files_created": value.total_files_created,
        "output_files": value.output_files,
        "metadata_file": value.metadata_file,
        "stages_run": value.stages_run,
        "segments": value.segments,
        "stream": value.stream,
        "source_type": value.source_type,
        "source_display": value.source_display,
        "entries_written": value.entries_written,
        "entities_seeded": value.entities_seeded,
        "date_range": value.date_range,
    }))
}

pub fn emit_importer_completed(emitter: &EventEmitter<'_>, value: &ImporterCompleted) {
    emitter.send("importer", "completed", importer_completed_fields(value));
}

pub(crate) fn importer_error_fields(value: &ImporterError) -> Map<String, Value> {
    object(json!({
        "import_id": value.import_id,
        "stage": value.stage,
        "error": value.error,
        "duration_ms": value.duration_ms,
        "partial_outputs": value.partial_outputs,
    }))
}

pub fn emit_importer_error(emitter: &EventEmitter<'_>, value: &ImporterError) {
    emitter.send("importer", "error", importer_error_fields(value));
}

pub(crate) fn supervisor_drain_fields(day: &str) -> Map<String, Value> {
    object(json!({"day": day}))
}

pub fn emit_supervisor_drain(emitter: &EventEmitter<'_>, day: &str) {
    emitter.send("supervisor", "drain", supervisor_drain_fields(day));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn observed_fields_are_exact() {
        let fields = observed_fields(&ObservedSegment {
            segment: "120000_60".to_owned(),
            day: "20260804".to_owned(),
            stream: "import.apple".to_owned(),
        });
        assert_eq!(
            fields,
            object(json!({
                "segment": "120000_60",
                "day": "20260804",
                "stream": "import.apple",
                "batch": true,
            }))
        );
    }

    #[test]
    fn enrichment_fields_preserve_supplied_days() {
        let fields = enrichment_ready_fields(&EnrichmentReady {
            import_id: "import-1".to_owned(),
            importer: "ics".to_owned(),
            days: vec!["20260802".to_owned(), "20260804".to_owned()],
            entries_written: 2,
        });
        assert_eq!(fields["days"], json!(["20260802", "20260804"]));
    }

    #[test]
    fn supervisor_drain_fields_are_exact() {
        assert_eq!(
            supervisor_drain_fields("20260804"),
            object(json!({"day": "20260804"}))
        );
    }

    #[test]
    fn remaining_event_builders_produce_objects() {
        assert!(
            !importer_status_fields(&ImporterStatus {
                import_id: "id".to_owned(),
                stage: "importing".to_owned(),
                elapsed_ms: 1,
                stage_elapsed_ms: 1,
                items_processed: 1,
                items_total: 2,
                earliest_date: None,
                latest_date: None,
                entities_found: 0,
                source_type: Some("ics".to_owned()),
                source_display: Some("Calendar".to_owned()),
            })
            .is_empty()
        );
        assert!(
            !importer_started_fields(&ImporterStarted {
                import_id: "id".to_owned(),
                input_file: "calendar.ics".to_owned(),
                file_type: "ics".to_owned(),
                day: "20260804".to_owned(),
                facet: None,
                setting: None,
                options: Map::new(),
                stage: "initialization".to_owned(),
                stream: "import.ics".to_owned(),
            })
            .is_empty()
        );
        assert!(
            !file_imported_fields(&FileImported {
                import_id: "id".to_owned(),
                importer: "ics".to_owned(),
                entries_written: 1,
                entities_seeded: 0,
                files_created: 1,
                errors: 0,
                stream: "import.ics".to_owned(),
                source_display: "Calendar".to_owned(),
                date_range: None,
                merge_summary: None,
                merge_log_path: None,
                merge_staging_path: None,
                summary_errors: None,
                principal_collision: None,
            })
            .is_empty()
        );
        assert!(
            !observing_fields(&ObservingSegment {
                segment: "120000_60".to_owned(),
                day: "20260804".to_owned(),
                files: vec!["audio.wav".to_owned()],
                stream: "import.audio".to_owned(),
                meta: ObservingMeta {
                    import_id: "id".to_owned(),
                    stream: "import.audio".to_owned(),
                    facet: None,
                    setting: None
                },
            })
            .is_empty()
        );
        assert!(
            !importer_completed_fields(&ImporterCompleted {
                import_id: "id".to_owned(),
                stage: "done".to_owned(),
                duration_ms: 1,
                total_files_created: 1,
                output_files: vec![],
                metadata_file: "imports/id/imported.json".to_owned(),
                stages_run: vec![],
                segments: vec![],
                stream: "import.ics".to_owned(),
                source_type: None,
                source_display: None,
                entries_written: 1,
                entities_seeded: 0,
                date_range: None,
            })
            .is_empty()
        );
        assert!(
            !importer_error_fields(&ImporterError {
                import_id: "id".to_owned(),
                stage: "importing".to_owned(),
                error: "failed".to_owned(),
                duration_ms: 1,
                partial_outputs: vec![],
            })
            .is_empty()
        );
    }
}
