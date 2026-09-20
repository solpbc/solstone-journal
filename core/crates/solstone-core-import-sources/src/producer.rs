// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Unified lifecycle producer for native Image and PDF/Document imports.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use solstone_core_generate::{ClientError, GenerateRequest, GenerateResponse};
use solstone_core_import::RegistrySource;
use solstone_core_import::events::{
    EventEmitter, ImporterCompleted, ImporterError, ImporterStarted, emit_importer_completed,
    emit_importer_error, emit_importer_started,
};
use solstone_core_import::metadata::{
    AttemptState, admit_running_attempt, get_attempt_facts, read_provenance,
    record_completed_attempt_unlocked, record_completed_attempt_with_input_failures_unlocked,
    record_unconfirmed_attempt, record_unconfirmed_attempt_unlocked,
};
use solstone_core_import::publish::{
    PublicationInput, PublicationOperations, PublicationRecord, PublicationStatus,
    publish_with_operations,
};

use crate::document::{self, DocumentImportRequest, DocumentModelClient, PdfWorker};
use crate::image::{self, DescriptionOutcome, WireClient};

/// Null wire client for non-vision execution paths or tests.
#[derive(Debug, Default, Clone, Copy)]
pub struct NullWireClient;

impl WireClient for NullWireClient {
    fn execute(&self, _request: &GenerateRequest) -> Result<GenerateResponse, ClientError> {
        Err(ClientError::Resolve(
            "NullWireClient does not execute requests".to_owned(),
        ))
    }
}

/// Null PDF worker for image execution paths or tests.
#[derive(Debug, Default, Clone, Copy)]
pub struct NullPdfWorker;

impl PdfWorker for NullPdfWorker {
    fn execute(
        &self,
        _request: &document::PdfWorkerRequest,
    ) -> Result<document::PdfPayload, document::WorkerFailure> {
        Err(document::WorkerFailure::Process {
            exit_code: Some(1),
            error: "NullPdfWorker does not execute worker requests".to_owned(),
            detail: None,
        })
    }
}

/// Null document model client for non-document execution paths or tests.
#[derive(Debug, Default, Clone, Copy)]
pub struct NullDocumentModelClient;

impl DocumentModelClient for NullDocumentModelClient {
    fn execute(&self, _request: &GenerateRequest) -> Result<GenerateResponse, ClientError> {
        Err(ClientError::Resolve(
            "NullDocumentModelClient does not execute model requests".to_owned(),
        ))
    }
}

#[derive(Clone)]
pub struct NativeProducerRequest<'a> {
    pub journal_root: &'a Path,
    pub source_path: &'a Path,
    pub import_id: &'a str,
    pub source: RegistrySource,
    pub revision: Option<&'a str>,
    pub password: Option<&'a str>,
    pub force: bool,
    pub expected_generation: Option<u64>,
    #[cfg(test)]
    pub before_publication: Option<std::sync::Arc<dyn Fn() + Send + Sync>>,
}

impl<'a> std::fmt::Debug for NativeProducerRequest<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NativeProducerRequest")
            .field("journal_root", &self.journal_root)
            .field("source_path", &self.source_path)
            .field("import_id", &self.import_id)
            .field("source", &self.source)
            .field("revision", &self.revision)
            .field("force", &self.force)
            .field("expected_generation", &self.expected_generation)
            .finish()
    }
}

#[derive(Debug)]
pub struct NativeProducerOutcome {
    pub import_id: String,
    pub entries_written: u64,
    pub total_files_created: u64,
    pub files_created: Vec<PathBuf>,
    pub duration_ms: u64,
    pub days_affected: Vec<String>,
    pub unavailable_description: Option<String>,
    pub unavailable_pages: Option<u64>,
    pub publication: Option<PublicationRecord>,
    pub errors: Vec<String>,
}

#[derive(Debug)]
pub enum NativeProducerError {
    AttemptInitFailed { detail: String },
    SourceFailed { detail: String },
    PublicationFailed { detail: String },
    UnsupportedSource { source: RegistrySource },
}

impl std::fmt::Display for NativeProducerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AttemptInitFailed { detail } => {
                write!(f, "failed to record running attempt: {detail}")
            }
            Self::SourceFailed { detail } => write!(f, "source processing failed: {detail}"),
            Self::PublicationFailed { detail } => write!(f, "publication failed: {detail}"),
            Self::UnsupportedSource { source } => {
                write!(f, "unsupported native source: {}", source.name())
            }
        }
    }
}

impl std::error::Error for NativeProducerError {}

pub fn run_native_producer<W, P, D, Pub>(
    request: NativeProducerRequest<'_>,
    wire: &W,
    pdf_worker: &P,
    doc_model: &D,
    publication: &Pub,
) -> Result<NativeProducerOutcome, NativeProducerError>
where
    W: WireClient,
    P: PdfWorker,
    D: DocumentModelClient,
    Pub: PublicationOperations,
{
    if request.source != RegistrySource::Image && request.source != RegistrySource::Document {
        return Err(NativeProducerError::UnsupportedSource {
            source: request.source,
        });
    }

    let started_at_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;

    // Step 1: Ensure running attempt is admitted and matches expected generation (fail closed)
    let (generation, attempt_id) = {
        let metadata = read_provenance(request.journal_root, request.import_id)
            .ok()
            .flatten();
        let existing = metadata.as_ref().and_then(get_attempt_facts);
        if let Some(expected) = request.expected_generation {
            let Some(f) =
                existing.filter(|f| f.state == AttemptState::Running && f.generation == expected)
            else {
                return Err(NativeProducerError::AttemptInitFailed {
                    detail: format!(
                        "running attempt generation mismatch for {}: expected generation {}",
                        request.import_id, expected
                    ),
                });
            };
            (f.generation, f.attempt_id)
        } else if let Some(f) = existing.filter(|f| f.state == AttemptState::Running) {
            (f.generation, f.attempt_id)
        } else {
            match admit_running_attempt(
                request.journal_root,
                request.import_id,
                started_at_ms,
                Some(request.source.name()),
            ) {
                Ok(f) => (f.generation, f.attempt_id),
                Err(err) => {
                    return Err(NativeProducerError::AttemptInitFailed {
                        detail: err.to_string(),
                    });
                }
            }
        }
    };

    let stream = match request.source {
        RegistrySource::Image => "import.image".to_owned(),
        RegistrySource::Document => "import.document".to_owned(),
        _ => "import".to_owned(),
    };

    // Step 2: Emit importer.started
    let emitter = EventEmitter::new(request.journal_root, None);
    emit_importer_started(
        &emitter,
        &ImporterStarted {
            import_id: request.import_id.to_owned(),
            input_file: request.source_path.to_string_lossy().into_owned(),
            file_type: request.source.name().to_owned(),
            day: String::new(),
            facet: None,
            setting: None,
            options: serde_json::Map::new(),
            stage: "execution".to_owned(),
            stream: stream.clone(),
            generation: Some(generation),
            attempt_id: Some(attempt_id.clone()),
        },
    );

    // Step 3: Source heavy processing OUTSIDE publication lock
    enum PreparedSource {
        Image(image::PreparedImage),
        Document(document::PreparedDocumentImport),
    }

    let prepared_source = match request.source {
        RegistrySource::Image => match image::prepare_image(request.source_path, wire) {
            Ok(prep) => PreparedSource::Image(prep),
            Err(err) => {
                let finished_at_ms = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as u64;
                let duration_ms = finished_at_ms.saturating_sub(started_at_ms);
                let _ = record_unconfirmed_attempt(
                    request.journal_root,
                    request.import_id,
                    generation,
                    finished_at_ms,
                    Some(err.to_string()),
                );
                emit_importer_error(
                    &emitter,
                    &ImporterError {
                        import_id: request.import_id.to_owned(),
                        stage: "execution".to_owned(),
                        error: "import failed".to_owned(),
                        duration_ms,
                        partial_outputs: vec![],
                        generation: Some(generation),
                        attempt_id: Some(attempt_id.clone()),
                    },
                );
                return Err(NativeProducerError::SourceFailed {
                    detail: err.to_string(),
                });
            }
        },
        RegistrySource::Document => {
            let import_dir = request.journal_root.join("imports").join(request.import_id);
            let doc_req = DocumentImportRequest {
                source: request.source_path,
                journal_root: request.journal_root,
                import_dir: &import_dir,
                import_id: request.import_id,
                revision: request.revision,
                password: request.password,
                force: request.force,
                now: SystemTime::now(),
            };
            let prep = document::prepare_document_import(&doc_req, pdf_worker, doc_model);
            if prep.items.is_empty() && !prep.hard_failures.is_empty() {
                let finished_at_ms = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as u64;
                let duration_ms = finished_at_ms.saturating_sub(started_at_ms);
                let err_msg = prep.hard_failures.join("; ");
                let _ = record_unconfirmed_attempt(
                    request.journal_root,
                    request.import_id,
                    generation,
                    finished_at_ms,
                    Some(err_msg.clone()),
                );
                emit_importer_error(
                    &emitter,
                    &ImporterError {
                        import_id: request.import_id.to_owned(),
                        stage: "execution".to_owned(),
                        error: "import failed".to_owned(),
                        duration_ms,
                        partial_outputs: vec![],
                        generation: Some(generation),
                        attempt_id: Some(attempt_id.clone()),
                    },
                );
                return Err(NativeProducerError::SourceFailed { detail: err_msg });
            }
            PreparedSource::Document(prep)
        }
        _ => unreachable!(),
    };

    // Test hook before taking publication lock
    #[cfg(test)]
    if let Some(hook) = &request.before_publication {
        hook();
    }

    // Step 4: Publication Critical Section under hold_import_lock
    let _lock =
        solstone_core_import::metadata::hold_import_lock(request.journal_root, request.import_id)
            .map_err(|error| NativeProducerError::PublicationFailed {
            detail: format!("the import is busy: {error}"),
        })?;

    // Check generation under lock
    let current_facts = read_provenance(request.journal_root, request.import_id)
        .ok()
        .flatten()
        .and_then(|m| get_attempt_facts(&m));
    if current_facts.as_ref().map(|f| (f.generation, f.state))
        != Some((generation, AttemptState::Running))
    {
        // Interrupted or superseded by newer generation! Do NOT write publication record or imported.json.
        emit_importer_error(
            &emitter,
            &ImporterError {
                import_id: request.import_id.to_owned(),
                stage: "publication".to_owned(),
                error: "this import couldn't be confirmed as finished.".to_owned(),
                duration_ms: 0,
                partial_outputs: vec![],
                generation: Some(generation),
                attempt_id: Some(attempt_id.clone()),
            },
        );
        return Err(NativeProducerError::PublicationFailed {
            detail: "this import couldn't be confirmed as finished.".to_owned(),
        });
    }

    match prepared_source {
        PreparedSource::Image(prep_img) => {
            let image_outcome = match image::install_and_publish_image(
                &prep_img,
                request.journal_root,
                request.import_id,
                publication,
                None,
            ) {
                Ok(out) => out,
                Err(err) => {
                    let finished_at_ms = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis() as u64;
                    let duration_ms = finished_at_ms.saturating_sub(started_at_ms);
                    let _ = record_unconfirmed_attempt_unlocked(
                        request.journal_root,
                        request.import_id,
                        generation,
                        finished_at_ms,
                        Some(err.to_string()),
                    );
                    emit_importer_error(
                        &emitter,
                        &ImporterError {
                            import_id: request.import_id.to_owned(),
                            stage: "publication".to_owned(),
                            error: "import failed".to_owned(),
                            duration_ms,
                            partial_outputs: vec![],
                            generation: Some(generation),
                            attempt_id: Some(attempt_id.clone()),
                        },
                    );
                    return Err(NativeProducerError::PublicationFailed {
                        detail: err.to_string(),
                    });
                }
            };

            let import_dir = request.journal_root.join("imports").join(request.import_id);
            let may_write = || {
                read_provenance(request.journal_root, request.import_id)
                    .ok()
                    .flatten()
                    .and_then(|m| get_attempt_facts(&m))
                    .map(|f| f.generation == generation)
                    .unwrap_or(false)
            };
            let segments = [image_outcome.created_segment.clone()];
            let pub_rec_res = publish_with_operations(
                PublicationInput {
                    journal: request.journal_root,
                    import_dir: Some(&import_dir),
                    import_id: request.import_id,
                    importer: "image",
                    revision: request.revision,
                    segments: &segments,
                    files_created: &image_outcome.files_created,
                    may_write_record: Some(&may_write),
                },
                publication,
            );

            let finished_at_ms = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64;
            let duration_ms = finished_at_ms.saturating_sub(started_at_ms);

            let unavailable_description = match &image_outcome.description {
                DescriptionOutcome::Unavailable { reason } => Some(reason.clone()),
                _ => None,
            };

            match pub_rec_res {
                Ok(pub_rec) if pub_rec.status == PublicationStatus::Success => {
                    if let Err(meta_err) = record_completed_attempt_unlocked(
                        request.journal_root,
                        request.import_id,
                        generation,
                        finished_at_ms,
                        Some(duration_ms),
                        unavailable_description.clone(),
                    ) {
                        let _ = record_unconfirmed_attempt_unlocked(
                            request.journal_root,
                            request.import_id,
                            generation,
                            finished_at_ms,
                            Some(meta_err.to_string()),
                        );
                        emit_importer_error(
                            &emitter,
                            &ImporterError {
                                import_id: request.import_id.to_owned(),
                                stage: "publication".to_owned(),
                                error: "import failed".to_owned(),
                                duration_ms,
                                partial_outputs: vec![],
                                generation: Some(generation),
                                attempt_id: Some(attempt_id.clone()),
                            },
                        );
                        return Err(NativeProducerError::PublicationFailed {
                            detail: meta_err.to_string(),
                        });
                    }
                    emit_importer_completed(
                        &emitter,
                        &ImporterCompleted {
                            import_id: request.import_id.to_owned(),
                            stage: "complete".to_owned(),
                            duration_ms,
                            total_files_created: image_outcome.files_created.len() as u64,
                            output_files: image_outcome
                                .files_created
                                .iter()
                                .map(|p| p.to_string_lossy().into_owned())
                                .collect(),
                            metadata_file: import_dir
                                .join("import.json")
                                .to_string_lossy()
                                .into_owned(),
                            stages_run: vec!["execution".to_owned(), "publication".to_owned()],
                            segments: vec![image_outcome.created_segment.segment],
                            stream,
                            source_type: Some("image".to_owned()),
                            source_display: Some("Image".to_owned()),
                            entries_written: 1,
                            entities_seeded: 0,
                            date_range: Some((
                                image_outcome
                                    .days_affected
                                    .first()
                                    .cloned()
                                    .unwrap_or_default(),
                                image_outcome
                                    .days_affected
                                    .last()
                                    .cloned()
                                    .unwrap_or_default(),
                            )),
                            generation: Some(generation),
                            attempt_id: Some(attempt_id.clone()),
                            errors: Vec::new(),
                        },
                    );
                    Ok(NativeProducerOutcome {
                        import_id: request.import_id.to_owned(),
                        entries_written: 1,
                        total_files_created: image_outcome.files_created.len() as u64,
                        files_created: image_outcome.files_created.clone(),
                        duration_ms,
                        days_affected: image_outcome.days_affected,
                        unavailable_description,
                        unavailable_pages: None,
                        publication: Some(pub_rec),
                        errors: Vec::new(),
                    })
                }
                _ => {
                    let _ = record_unconfirmed_attempt_unlocked(
                        request.journal_root,
                        request.import_id,
                        generation,
                        finished_at_ms,
                        Some("import failed".to_owned()),
                    );
                    emit_importer_error(
                        &emitter,
                        &ImporterError {
                            import_id: request.import_id.to_owned(),
                            stage: "publication".to_owned(),
                            error: "import failed".to_owned(),
                            duration_ms,
                            partial_outputs: vec![],
                            generation: Some(generation),
                            attempt_id: Some(attempt_id.clone()),
                        },
                    );
                    Err(NativeProducerError::PublicationFailed {
                        detail: "one or more publication operations failed".to_owned(),
                    })
                }
            }
        }
        PreparedSource::Document(prep_doc) => {
            let import_dir = request.journal_root.join("imports").join(request.import_id);
            let doc_req = DocumentImportRequest {
                source: request.source_path,
                journal_root: request.journal_root,
                import_dir: &import_dir,
                import_id: request.import_id,
                revision: request.revision,
                password: request.password,
                force: request.force,
                now: SystemTime::now(),
            };
            let may_write = || {
                read_provenance(request.journal_root, request.import_id)
                    .ok()
                    .flatten()
                    .and_then(|m| get_attempt_facts(&m))
                    .map(|f| f.generation == generation)
                    .unwrap_or(false)
            };
            let import_res = document::install_and_publish_document(
                &prep_doc,
                &doc_req,
                publication,
                Some(&may_write),
            );

            let finished_at_ms = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64;
            let duration_ms = finished_at_ms.saturating_sub(started_at_ms);

            let proj = solstone_core_import::project_import_result(
                request.journal_root,
                request.import_id,
            );

            let is_success = import_res.entries_written > 0
                && import_res.hard_failures.is_empty()
                && proj.entries_written == Some(import_res.entries_written);

            if is_success {
                // Inputs that failed while others imported are recorded on the attempt, so a
                // reload still shows a partial outcome instead of a clean success.
                let completion = if import_res.errors.is_empty() {
                    record_completed_attempt_unlocked(
                        request.journal_root,
                        request.import_id,
                        generation,
                        finished_at_ms,
                        Some(duration_ms),
                        None,
                    )
                } else {
                    record_completed_attempt_with_input_failures_unlocked(
                        request.journal_root,
                        request.import_id,
                        generation,
                        finished_at_ms,
                        Some(duration_ms),
                        import_res.errors.len() as u64,
                    )
                };
                if let Err(meta_err) = completion {
                    let _ = record_unconfirmed_attempt_unlocked(
                        request.journal_root,
                        request.import_id,
                        generation,
                        finished_at_ms,
                        Some(meta_err.to_string()),
                    );
                    emit_importer_error(
                        &emitter,
                        &ImporterError {
                            import_id: request.import_id.to_owned(),
                            stage: "publication".to_owned(),
                            error: "import failed".to_owned(),
                            duration_ms,
                            partial_outputs: vec![],
                            generation: Some(generation),
                            attempt_id: Some(attempt_id.clone()),
                        },
                    );
                    return Err(NativeProducerError::PublicationFailed {
                        detail: meta_err.to_string(),
                    });
                }

                emit_importer_completed(
                    &emitter,
                    &ImporterCompleted {
                        import_id: request.import_id.to_owned(),
                        stage: "complete".to_owned(),
                        duration_ms,
                        total_files_created: import_res.files_created.len() as u64,
                        output_files: import_res.files_created.clone(),
                        metadata_file: import_dir
                            .join("import.json")
                            .to_string_lossy()
                            .into_owned(),
                        stages_run: vec!["execution".to_owned(), "publication".to_owned()],
                        segments: import_res
                            .segments
                            .as_ref()
                            .map(|segs| segs.iter().map(|(_, s)| s.clone()).collect())
                            .unwrap_or_default(),
                        stream,
                        source_type: Some("document".to_owned()),
                        source_display: Some("PDF Document".to_owned()),
                        entries_written: proj.entries_written.unwrap_or(import_res.entries_written),
                        entities_seeded: 0,
                        date_range: proj.date_range.clone().or(import_res.date_range.clone()),
                        generation: Some(generation),
                        attempt_id: Some(attempt_id.clone()),
                        errors: import_res.errors.clone(),
                    },
                );
                Ok(NativeProducerOutcome {
                    import_id: request.import_id.to_owned(),
                    entries_written: proj.entries_written.unwrap_or(import_res.entries_written),
                    total_files_created: proj
                        .total_files_created
                        .unwrap_or(import_res.files_created.len() as u64),
                    files_created: import_res.files_created.iter().map(PathBuf::from).collect(),
                    duration_ms,
                    days_affected: proj.days_affected,
                    unavailable_description: proj.unavailable_description,
                    unavailable_pages: proj.unavailable_pages,
                    publication: None,
                    errors: import_res.errors,
                })
            } else {
                let _ = record_unconfirmed_attempt_unlocked(
                    request.journal_root,
                    request.import_id,
                    generation,
                    finished_at_ms,
                    Some("import failed".to_owned()),
                );
                emit_importer_error(
                    &emitter,
                    &ImporterError {
                        import_id: request.import_id.to_owned(),
                        stage: "execution".to_owned(),
                        error: "import failed".to_owned(),
                        duration_ms,
                        partial_outputs: vec![],
                        generation: Some(generation),
                        attempt_id: Some(attempt_id.clone()),
                    },
                );
                Err(NativeProducerError::SourceFailed {
                    detail: if import_res.hard_failures.is_empty() {
                        "the document import produced no entries".to_owned()
                    } else {
                        import_res.hard_failures.join("; ")
                    },
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    use solstone_core_import::publish::CreatedSegment;
    use solstone_core_indexer_store::scan::RescanFileStatus;
    use solstone_core_segment::{StreamAdvance, UnboundStreamAdvanceError};
    use std::fs;
    use std::path::PathBuf;

    const TINY_PNG: &[u8] = &[
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F,
        0x15, 0xC4, 0x89, 0x00, 0x00, 0x00, 0x0A, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0x00,
        0x01, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00, 0x00, 0x00, 0x00, 0x49,
        0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
    ];

    struct FakePdfWorker {
        payload: document::PdfPayload,
    }

    impl FakePdfWorker {
        fn new(payload: document::PdfPayload) -> Self {
            Self { payload }
        }
    }

    impl document::PdfWorker for FakePdfWorker {
        fn execute(
            &self,
            _request: &document::PdfWorkerRequest,
        ) -> Result<document::PdfPayload, document::WorkerFailure> {
            Ok(self.payload.clone())
        }
    }

    struct FailingPublicationOps;

    impl PublicationOperations for FailingPublicationOps {
        fn advance_stream(
            &self,
            _journal: &Path,
            _segment: &CreatedSegment,
        ) -> Result<StreamAdvance, UnboundStreamAdvanceError> {
            Err(UnboundStreamAdvanceError::MarkerWrite {
                path: PathBuf::new(),
                source: solstone_core_journal_io::AtomicWriteError::Io {
                    path: PathBuf::new(),
                    source: std::io::Error::other("forced failure"),
                },
            })
        }

        fn rescan_file(&self, _journal: &Path, _path: &Path) -> Result<RescanFileStatus, String> {
            Ok(RescanFileStatus::Indexed { warnings: vec![] })
        }

        fn touch_stream_health_marker(&self, _journal: &Path, _day: &str) -> Result<(), String> {
            Err("forced health marker failure".to_owned())
        }

        fn emit_observed(
            &self,
            _journal: &Path,
            _revision: Option<&str>,
            _day: &str,
            _segment: &str,
            _stream: &str,
        ) {
        }

        fn emit_enrichment_ready(
            &self,
            _journal: &Path,
            _revision: Option<&str>,
            _import_id: &str,
            _importer: &str,
            _days: &[String],
            _entries_written: u64,
        ) {
        }

        fn emit_drain(&self, _journal: &Path, _revision: Option<&str>, _day: &str) {}
    }

    /// Real publication operations, except that the day's health marker can be touched
    /// only once: the touch when the original is installed succeeds and the one during
    /// publication fails, the way a marker blocked after install would.
    struct MarkerFailsAtPublication {
        touches: std::sync::atomic::AtomicUsize,
    }

    impl PublicationOperations for MarkerFailsAtPublication {
        fn advance_stream(
            &self,
            journal: &Path,
            segment: &CreatedSegment,
        ) -> Result<StreamAdvance, UnboundStreamAdvanceError> {
            solstone_core_import::NativePublicationOperations.advance_stream(journal, segment)
        }

        fn rescan_file(&self, journal: &Path, path: &Path) -> Result<RescanFileStatus, String> {
            solstone_core_import::NativePublicationOperations.rescan_file(journal, path)
        }

        fn touch_stream_health_marker(&self, journal: &Path, day: &str) -> Result<(), String> {
            if self
                .touches
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
                == 0
            {
                solstone_core_import::NativePublicationOperations
                    .touch_stream_health_marker(journal, day)
            } else {
                Err("blocked publication marker".to_owned())
            }
        }

        fn emit_observed(
            &self,
            journal: &Path,
            revision: Option<&str>,
            day: &str,
            segment: &str,
            stream: &str,
        ) {
            solstone_core_import::NativePublicationOperations
                .emit_observed(journal, revision, day, segment, stream);
        }

        fn emit_enrichment_ready(
            &self,
            journal: &Path,
            revision: Option<&str>,
            import_id: &str,
            importer: &str,
            days: &[String],
            entries_written: u64,
        ) {
            solstone_core_import::NativePublicationOperations.emit_enrichment_ready(
                journal,
                revision,
                import_id,
                importer,
                days,
                entries_written,
            );
        }

        fn emit_drain(&self, journal: &Path, revision: Option<&str>, day: &str) {
            solstone_core_import::NativePublicationOperations.emit_drain(journal, revision, day);
        }
    }

    #[test]
    fn a_day_marker_that_fails_at_publication_is_terminal_after_the_content_is_installed() {
        let temp = tempfile::Builder::new()
            .prefix("test-marker-at-publication-")
            .tempdir()
            .unwrap();
        let root = temp.path();
        let img_path = root.join("sample.png");
        fs::write(&img_path, TINY_PNG).unwrap();
        let id = "20260408_183000";

        let result = run_native_producer(
            NativeProducerRequest {
                journal_root: root,
                source_path: &img_path,
                import_id: id,
                source: RegistrySource::Image,
                revision: None,
                password: None,
                force: false,
                expected_generation: None,
                before_publication: None,
            },
            &NullWireClient,
            &NullPdfWorker,
            &crate::NullDocumentModelClient,
            &MarkerFailsAtPublication {
                touches: std::sync::atomic::AtomicUsize::new(0),
            },
        );
        assert!(
            result.is_err(),
            "a blocked publication marker is not success"
        );

        // The owner's original stays installed, the publication is recorded as a failure,
        // and the projection calls it failed (not merely unconfirmed).
        let installed = fs::read_dir(root.join("chronicle"))
            .unwrap()
            .flatten()
            .flat_map(|day| {
                fs::read_dir(day.path().join("import.image"))
                    .unwrap()
                    .flatten()
            })
            .any(|segment| segment.path().join("original.png").is_file());
        assert!(installed, "original must remain installed");
        let record: Value = serde_json::from_slice(
            &fs::read(root.join("imports").join(id).join("imported.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(record["status"], "failure");
        assert_eq!(record["day_markers"][0]["outcome"]["status"], "failed");
        let projection = solstone_core_import::project_import_result(root, id);
        assert_eq!(
            projection.status,
            solstone_core_import::ProjectionStatus::Failed,
            "{projection:?}"
        );
        assert_eq!(projection.error_stage.as_deref(), Some("publication"));
    }

    #[test]
    fn test_native_producer_image_lifecycle_success() {
        let temp = tempfile::Builder::new()
            .prefix("test-native-producer-")
            .tempdir()
            .unwrap();
        let root = temp.path();
        let img_path = root.join("test.png");
        fs::write(&img_path, TINY_PNG).unwrap();

        let id = "20260408_140000";
        let wire = NullWireClient;
        let worker = NullPdfWorker;
        let model = crate::NullDocumentModelClient;
        let publication = solstone_core_import::NativePublicationOperations;

        let req = NativeProducerRequest {
            journal_root: root,
            source_path: &img_path,
            import_id: id,
            source: RegistrySource::Image,
            revision: None,
            password: None,
            force: false,
            expected_generation: None,
            before_publication: None,
        };

        let outcome =
            run_native_producer(req, &wire, &worker, &model, &publication).expect("import success");
        assert_eq!(outcome.import_id, id);
        assert_eq!(outcome.entries_written, 1);
        assert!(outcome.publication.is_some());

        // Verify attempt completed on disk
        let meta = solstone_core_import::read_import_metadata(root, id).unwrap();
        let attempt = solstone_core_import::get_attempt_facts(&meta).unwrap();
        assert_eq!(
            attempt.state,
            solstone_core_import::metadata::AttemptState::Completed
        );
        assert!(attempt.unavailable_description.is_some());
    }

    #[test]
    fn test_native_producer_image_writes_manifest_imported_and_transcript() {
        let temp = tempfile::Builder::new()
            .prefix("test-native-img-")
            .tempdir()
            .unwrap();
        let root = temp.path();
        let img_path = root.join("sample.png");
        fs::write(&img_path, TINY_PNG).unwrap();

        let id = "20260408_150000";
        let req = NativeProducerRequest {
            journal_root: root,
            source_path: &img_path,
            import_id: id,
            source: RegistrySource::Image,
            revision: None,
            password: None,
            force: false,
            expected_generation: None,
            before_publication: None,
        };

        run_native_producer(
            req,
            &NullWireClient,
            &NullPdfWorker,
            &crate::NullDocumentModelClient,
            &solstone_core_import::NativePublicationOperations,
        )
        .expect("image producer run success");

        let import_dir = root.join("imports").join(id);
        let manifest_file = import_dir.join("manifest.json");
        let imported_file = import_dir.join("imported.json");
        assert!(manifest_file.exists());
        assert!(imported_file.exists());

        let imported_text = fs::read_to_string(imported_file).unwrap();
        let imported: Value = serde_json::from_str(&imported_text).unwrap();
        assert_eq!(
            imported.get("schema").and_then(Value::as_str),
            Some("solstone.import.publication.v1")
        );

        let proj = solstone_core_import::project_import_result(root, id);
        assert_eq!(proj.status, solstone_core_import::ProjectionStatus::Success);
        assert_eq!(proj.entries_written, Some(1));
    }

    #[test]
    fn test_native_producer_pdf_writes_content_manifest_and_no_manifest_json() {
        let temp = tempfile::Builder::new()
            .prefix("test-native-pdf-")
            .tempdir()
            .unwrap();
        let root = temp.path();
        let pdf_path = root.join("doc.pdf");
        fs::write(&pdf_path, b"%PDF-1.4 fake").unwrap();

        let id = "20260408_160000";
        let text =
            "Hello world! This is a valid document page with more than fifty characters of text.";
        let payload = document::PdfPayload {
            schema: "sol-pdf/1".to_owned(),
            page_count: 1,
            pages: vec![document::PdfPage {
                index: 0,
                chars: text.len(),
                text: Some(text.to_owned()),
                ..Default::default()
            }],
            ..Default::default()
        };
        let worker = FakePdfWorker::new(payload);
        let req = NativeProducerRequest {
            journal_root: root,
            source_path: &pdf_path,
            import_id: id,
            source: RegistrySource::Document,
            revision: None,
            password: None,
            force: false,
            expected_generation: None,
            before_publication: None,
        };

        run_native_producer(
            req,
            &NullWireClient,
            &worker,
            &crate::NullDocumentModelClient,
            &solstone_core_import::NativePublicationOperations,
        )
        .expect("pdf producer run success");

        let import_dir = root.join("imports").join(id);
        assert!(import_dir.join("content_manifest.jsonl").exists());
        assert!(import_dir.join("imported.json").exists());
        assert!(!import_dir.join("manifest.json").exists());

        let proj = solstone_core_import::project_import_result(root, id);
        assert_eq!(proj.status, solstone_core_import::ProjectionStatus::Success);
        assert_eq!(proj.entries_written, Some(1));
    }

    #[test]
    fn test_native_producer_failed_running_attempt_no_chronicle_mutation() {
        let temp = tempfile::Builder::new()
            .prefix("test-native-fail-attempt-")
            .tempdir()
            .unwrap();
        let root = temp.path();
        let img_path = root.join("sample.png");
        fs::write(&img_path, TINY_PNG).unwrap();

        let id = "20260408_170000";
        let import_dir = root.join("imports").join(id);
        fs::create_dir_all(&import_dir).unwrap();
        fs::write(import_dir.join("import.json"), b"invalid json").unwrap();

        let req = NativeProducerRequest {
            journal_root: root,
            source_path: &img_path,
            import_id: id,
            source: RegistrySource::Image,
            revision: None,
            password: None,
            force: false,
            expected_generation: None,
            before_publication: None,
        };

        let res = run_native_producer(
            req,
            &NullWireClient,
            &NullPdfWorker,
            &crate::NullDocumentModelClient,
            &solstone_core_import::NativePublicationOperations,
        );
        assert!(res.is_err());
        assert!(!root.join("chronicle").exists());
    }

    #[test]
    fn test_native_producer_expected_generation_mismatch_fails_closed() {
        let temp = tempfile::Builder::new()
            .prefix("test-expected-gen-mismatch-")
            .tempdir()
            .unwrap();
        let root = temp.path();
        let img_path = root.join("sample.png");
        fs::write(&img_path, TINY_PNG).unwrap();

        let id = "20260408_175000";
        let facts =
            solstone_core_import::admit_running_attempt(root, id, 1000, Some("image")).unwrap();
        assert_eq!(facts.generation, 1);

        let req = NativeProducerRequest {
            journal_root: root,
            source_path: &img_path,
            import_id: id,
            source: RegistrySource::Image,
            revision: None,
            password: None,
            force: false,
            expected_generation: Some(2), // Generation mismatch
            before_publication: None,
        };

        let res = run_native_producer(
            req,
            &NullWireClient,
            &NullPdfWorker,
            &crate::NullDocumentModelClient,
            &solstone_core_import::NativePublicationOperations,
        );
        assert!(res.is_err());
        assert!(!root.join("chronicle").exists());
    }

    #[test]
    fn test_native_producer_publication_failure_is_not_success() {
        let temp = tempfile::Builder::new()
            .prefix("test-native-pub-fail-")
            .tempdir()
            .unwrap();
        let root = temp.path();
        let img_path = root.join("sample.png");
        fs::write(&img_path, TINY_PNG).unwrap();

        let id = "20260408_180000";
        let req = NativeProducerRequest {
            journal_root: root,
            source_path: &img_path,
            import_id: id,
            source: RegistrySource::Image,
            revision: None,
            password: None,
            force: false,
            expected_generation: None,
            before_publication: None,
        };

        let res = run_native_producer(
            req,
            &NullWireClient,
            &NullPdfWorker,
            &crate::NullDocumentModelClient,
            &FailingPublicationOps,
        );
        assert!(res.is_err());

        let meta = solstone_core_import::read_import_metadata(root, id).unwrap();
        let attempt = solstone_core_import::get_attempt_facts(&meta).unwrap();
        assert_eq!(
            attempt.state,
            solstone_core_import::metadata::AttemptState::Unconfirmed
        );
    }

    #[test]
    fn test_native_producer_completed_attempt_failure_projection_unconfirmed() {
        let temp = tempfile::Builder::new()
            .prefix("test-proj-unconf-")
            .tempdir()
            .unwrap();
        let root = temp.path();
        let image_path = root.join("test.png");
        fs::write(&image_path, TINY_PNG).unwrap();

        let id = "20260408_190000";
        let req = NativeProducerRequest {
            journal_root: root,
            source_path: &image_path,
            import_id: id,
            source: RegistrySource::Image,
            revision: None,
            password: None,
            force: false,
            expected_generation: None,
            before_publication: None,
        };
        let res = run_native_producer(
            req,
            &NullWireClient,
            &NullPdfWorker,
            &crate::NullDocumentModelClient,
            &FailingPublicationOps,
        );
        assert!(
            res.is_err(),
            "FailingPublicationOps must cause producer to fail"
        );

        let proj = solstone_core_import::project_import_result(root, id);
        assert_ne!(proj.status, solstone_core_import::ProjectionStatus::Success);
        assert!(
            proj.status == solstone_core_import::ProjectionStatus::Unconfirmed
                || proj.status == solstone_core_import::ProjectionStatus::Failed
        );
    }

    #[test]
    fn test_native_producer_pdf_unavailable_pages_comparison() {
        // Run 1: with 1 unavailable page (page 0 is damaged) and 1 available page (page 1)
        let temp1 = tempfile::Builder::new()
            .prefix("test-pdf-unavail1-")
            .tempdir()
            .unwrap();
        let root1 = temp1.path();
        let pdf_path1 = root1.join("doc1.pdf");
        fs::write(&pdf_path1, b"%PDF-1.4 fake 1").unwrap();

        let id1 = "20260408_200000";
        let text1 =
            "Page one has plenty of text to exceed the required fifty character minimum threshold.";
        let payload1 = document::PdfPayload {
            schema: "sol-pdf/1".to_owned(),
            page_count: 2,
            pages: vec![
                document::PdfPage {
                    index: 0,
                    error: Some("damage".to_owned()),
                    text: None,
                    ..Default::default()
                },
                document::PdfPage {
                    index: 1,
                    chars: text1.len(),
                    text: Some(text1.to_owned()),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let req1 = NativeProducerRequest {
            journal_root: root1,
            source_path: &pdf_path1,
            import_id: id1,
            source: RegistrySource::Document,
            revision: None,
            password: None,
            force: false,
            expected_generation: None,
            before_publication: None,
        };
        run_native_producer(
            req1,
            &NullWireClient,
            &FakePdfWorker::new(payload1),
            &crate::NullDocumentModelClient,
            &solstone_core_import::NativePublicationOperations,
        )
        .unwrap();
        let proj1 = solstone_core_import::project_import_result(root1, id1);
        assert_eq!(proj1.unavailable_pages, Some(1));

        // Run 2: twin with 0 unavailable pages
        let temp2 = tempfile::Builder::new()
            .prefix("test-pdf-unavail2-")
            .tempdir()
            .unwrap();
        let root2 = temp2.path();
        let pdf_path2 = root2.join("doc2.pdf");
        fs::write(&pdf_path2, b"%PDF-1.4 fake 2").unwrap();

        let id2 = "20260408_210000";
        let text2 =
            "This clean page also contains well over fifty characters of good plain text content.";
        let payload2 = document::PdfPayload {
            schema: "sol-pdf/1".to_owned(),
            page_count: 1,
            pages: vec![document::PdfPage {
                index: 0,
                chars: text2.len(),
                text: Some(text2.to_owned()),
                ..Default::default()
            }],
            ..Default::default()
        };
        let req2 = NativeProducerRequest {
            journal_root: root2,
            source_path: &pdf_path2,
            import_id: id2,
            source: RegistrySource::Document,
            revision: None,
            password: None,
            force: false,
            expected_generation: None,
            before_publication: None,
        };
        run_native_producer(
            req2,
            &NullWireClient,
            &FakePdfWorker::new(payload2),
            &crate::NullDocumentModelClient,
            &solstone_core_import::NativePublicationOperations,
        )
        .unwrap();
        let proj2 = solstone_core_import::project_import_result(root2, id2);
        assert_eq!(proj2.unavailable_pages, Some(0));
    }

    #[test]
    fn test_document_zero_entries_no_errors_fails_source() {
        let temp = tempfile::Builder::new()
            .prefix("test-pdf-zero-")
            .tempdir()
            .unwrap();
        let root = temp.path();
        let empty_dir = root.join("empty_source_dir");
        fs::create_dir_all(&empty_dir).unwrap();

        let id = "20260408_220000";
        let payload = document::PdfPayload {
            schema: "sol-pdf/1".to_owned(),
            engine: "test".to_owned(),
            page_count: 0,
            pages: vec![],
            ..Default::default()
        };
        let req = NativeProducerRequest {
            journal_root: root,
            source_path: &empty_dir,
            import_id: id,
            source: RegistrySource::Document,
            revision: None,
            password: None,
            force: false,
            expected_generation: None,
            before_publication: None,
        };
        let res = run_native_producer(
            req,
            &NullWireClient,
            &FakePdfWorker::new(payload),
            &crate::NullDocumentModelClient,
            &solstone_core_import::NativePublicationOperations,
        );
        assert!(matches!(res, Err(NativeProducerError::SourceFailed { .. })));
        let proj = solstone_core_import::project_import_result(root, id);
        assert_ne!(proj.status, solstone_core_import::ProjectionStatus::Success);
    }

    #[test]
    fn test_manifest_or_content_manifest_missing_is_not_completed() {
        let temp = tempfile::Builder::new()
            .prefix("test-missing-manifest-")
            .tempdir()
            .unwrap();
        let root = temp.path();
        let id = "20260408_230000";
        let pdf_path = root.join("doc.pdf");
        fs::write(&pdf_path, b"%PDF-1.4 valid test pdf").unwrap();

        let text = "First producer page text content with enough characters for parsing.";
        let payload1 = document::PdfPayload {
            schema: "sol-pdf/1".to_owned(),
            engine: "test".to_owned(),
            page_count: 1,
            pages: vec![document::PdfPage {
                index: 0,
                chars: text.len(),
                text: Some(text.to_owned()),
                ..Default::default()
            }],
            ..Default::default()
        };

        let req1 = NativeProducerRequest {
            journal_root: root,
            source_path: &pdf_path,
            import_id: id,
            source: RegistrySource::Document,
            revision: None,
            password: None,
            force: false,
            expected_generation: None,
            before_publication: None,
        };

        // First producer succeeds and writes content_manifest.jsonl
        let res1 = run_native_producer(
            req1,
            &NullWireClient,
            &FakePdfWorker::new(payload1),
            &crate::NullDocumentModelClient,
            &solstone_core_import::NativePublicationOperations,
        );
        assert!(res1.is_ok(), "{res1:?}");
        let proj1 = solstone_core_import::project_import_result(root, id);
        assert_eq!(
            proj1.status,
            solstone_core_import::ProjectionStatus::Success
        );

        // Ensure manifest persistence failure leaves previous file in place but fails second attempt
        let import_dir = root.join("imports").join(id);
        let cm_path = import_dir.join("content_manifest.jsonl");
        assert!(
            cm_path.is_file(),
            "attempt 1 content manifest exists as a file"
        );
        fs::write(import_dir.join(".fail_manifest_write"), b"1").unwrap();

        // Second producer runs (e.g. force rerun or new attempt)
        let payload2 = document::PdfPayload {
            schema: "sol-pdf/1".to_owned(),
            engine: "test".to_owned(),
            page_count: 1,
            pages: vec![document::PdfPage {
                index: 0,
                chars: text.len(),
                text: Some(text.to_owned()),
                ..Default::default()
            }],
            ..Default::default()
        };
        let req2 = NativeProducerRequest {
            journal_root: root,
            source_path: &pdf_path,
            import_id: id,
            source: RegistrySource::Document,
            revision: None,
            password: None,
            force: true,
            expected_generation: None,
            before_publication: None,
        };
        let res2 = run_native_producer(
            req2,
            &NullWireClient,
            &FakePdfWorker::new(payload2),
            &crate::NullDocumentModelClient,
            &solstone_core_import::NativePublicationOperations,
        );

        assert!(
            res2.is_err(),
            "Second producer must fail when content_manifest write fails even if previous file exists"
        );
        assert!(
            cm_path.is_file(),
            "attempt 1 content manifest file is still in place"
        );

        let proj2 = solstone_core_import::project_import_result(root, id);
        assert_ne!(
            proj2.status,
            solstone_core_import::ProjectionStatus::Success
        );
    }

    #[test]
    fn test_late_generation_cannot_complete_over_newer_admission() {
        let temp = tempfile::Builder::new()
            .prefix("test-gen-race-")
            .tempdir()
            .unwrap();
        let root = temp.path();
        let id = "20260408_233000";
        let image_path = root.join("test.png");
        fs::write(&image_path, TINY_PNG).unwrap();

        // Attempt 1 admitted
        let facts1 =
            solstone_core_import::admit_running_attempt(root, id, 1700000000000, Some("image"))
                .unwrap();
        assert_eq!(facts1.generation, 1);

        // Attempt 2 admitted later for the same import
        let facts2 =
            solstone_core_import::admit_running_attempt(root, id, 1700000005000, Some("image"))
                .unwrap();
        assert_eq!(facts2.generation, 2);

        // Attempt 1 runs native producer with expected_generation: Some(1)
        let req1 = NativeProducerRequest {
            journal_root: root,
            source_path: &image_path,
            import_id: id,
            source: RegistrySource::Image,
            revision: None,
            password: None,
            force: false,
            expected_generation: Some(1),
            before_publication: None,
        };
        let res1 = run_native_producer(
            req1,
            &NullWireClient,
            &NullPdfWorker,
            &crate::NullDocumentModelClient,
            &solstone_core_import::NativePublicationOperations,
        );
        assert!(res1.is_err());

        // Producer with expected 2 can succeed
        let req2 = NativeProducerRequest {
            journal_root: root,
            source_path: &image_path,
            import_id: id,
            source: RegistrySource::Image,
            revision: None,
            password: None,
            force: false,
            expected_generation: Some(2),
            before_publication: None,
        };
        let res2 = run_native_producer(
            req2,
            &NullWireClient,
            &NullPdfWorker,
            &crate::NullDocumentModelClient,
            &solstone_core_import::NativePublicationOperations,
        );
        assert!(res2.is_ok(), "{res2:?}");

        // Assertion: projection generation is 2 and status is not failed-from-loser (i.e. it is Success)
        let proj = solstone_core_import::project_import_result(root, id);
        assert_eq!(proj.generation, Some(2));
        assert_eq!(proj.status, solstone_core_import::ProjectionStatus::Success);
    }

    #[test]
    fn test_superseded_producer_does_not_overwrite_winner_publication() {
        let temp = tempfile::Builder::new()
            .prefix("test-superseded-producer-")
            .tempdir()
            .unwrap();
        let root = temp.path();
        let id = "20260408_234000";
        let image_path = root.join("test.png");
        fs::write(&image_path, TINY_PNG).unwrap();

        // Admit generation 1
        let facts1 =
            solstone_core_import::admit_running_attempt(root, id, 1700000000000, Some("image"))
                .unwrap();
        assert_eq!(facts1.generation, 1);

        let root_clone = root.to_path_buf();
        let image_path_clone = image_path.clone();
        let id_str = id.to_owned();
        let winner_bytes = std::sync::Arc::new(std::sync::Mutex::new(None::<Vec<u8>>));
        let winner_bytes_hook = winner_bytes.clone();

        // Hook for producer 1: right before publication critical section, admit gen 2 and run producer 2 to completion
        let hook = std::sync::Arc::new(move || {
            let facts2 = solstone_core_import::admit_running_attempt(
                &root_clone,
                &id_str,
                1700000005000,
                Some("image"),
            )
            .unwrap();
            assert_eq!(facts2.generation, 2);

            let req2 = NativeProducerRequest {
                journal_root: &root_clone,
                source_path: &image_path_clone,
                import_id: &id_str,
                source: RegistrySource::Image,
                revision: None,
                password: None,
                force: false,
                expected_generation: Some(2),
                before_publication: None,
            };
            let res2 = run_native_producer(
                req2,
                &NullWireClient,
                &NullPdfWorker,
                &crate::NullDocumentModelClient,
                &solstone_core_import::NativePublicationOperations,
            );
            assert!(res2.is_ok(), "Producer 2 must complete successfully");
            let imported = root_clone
                .join("imports")
                .join(&id_str)
                .join("imported.json");
            let bytes = fs::read(&imported).unwrap();
            *winner_bytes_hook.lock().unwrap() = Some(bytes);
            fs::write(
                &imported,
                br#"{"schema":"solstone.import.publication.v1","status":"success","sentinel":"winner-gen-2","segments":[],"indexing":{"published":[],"declined":[],"errored":[]},"day_markers":[]}"#,
            )
            .unwrap();
        });

        let req1 = NativeProducerRequest {
            journal_root: root,
            source_path: &image_path,
            import_id: id,
            source: RegistrySource::Image,
            revision: None,
            password: None,
            force: false,
            expected_generation: Some(1),
            before_publication: Some(hook),
        };

        let res1 = run_native_producer(
            req1,
            &NullWireClient,
            &NullPdfWorker,
            &crate::NullDocumentModelClient,
            &solstone_core_import::NativePublicationOperations,
        );
        assert!(res1.is_err(), "Superseded Producer 1 must fail");

        // Read imported.json and import.json
        let imported_path = root.join("imports").join(id).join("imported.json");
        let imported_bytes = fs::read(&imported_path).unwrap();
        assert!(
            winner_bytes.lock().unwrap().is_some(),
            "winner publication bytes captured"
        );
        let imported: Value = serde_json::from_slice(&imported_bytes).unwrap();
        assert_eq!(imported["status"], "success");
        assert_eq!(
            imported["sentinel"], "winner-gen-2",
            "superseded producer must not replace the winner publication record"
        );

        let proj = solstone_core_import::project_import_result(root, id);
        assert_eq!(proj.status, solstone_core_import::ProjectionStatus::Success);
        assert_eq!(proj.generation, Some(2));
    }

    #[test]
    fn test_image_import_does_not_require_pdf_worker() {
        struct PanickingPdfWorker;
        impl document::PdfWorker for PanickingPdfWorker {
            fn execute(
                &self,
                _request: &document::PdfWorkerRequest,
            ) -> Result<document::PdfPayload, document::WorkerFailure> {
                panic!("PDF worker should not be called for image import!");
            }
        }

        let temp = tempfile::Builder::new()
            .prefix("test-img-null-worker-")
            .tempdir()
            .unwrap();
        let root = temp.path();
        let image_path = root.join("test.png");
        fs::write(&image_path, TINY_PNG).unwrap();

        let id = "20260408_234500";
        let req = NativeProducerRequest {
            journal_root: root,
            source_path: &image_path,
            import_id: id,
            source: RegistrySource::Image,
            revision: None,
            password: None,
            force: false,
            expected_generation: None,
            before_publication: None,
        };
        let res = run_native_producer(
            req,
            &NullWireClient,
            &PanickingPdfWorker,
            &crate::NullDocumentModelClient,
            &solstone_core_import::NativePublicationOperations,
        );
        assert!(res.is_ok());
    }

    #[test]
    fn test_malformed_attempt_bytes_preserved_and_projection_unavailable() {
        let temp = tempfile::Builder::new()
            .prefix("test-malformed-attempt-")
            .tempdir()
            .unwrap();
        let root = temp.path();
        let id = "20260408_235000";
        let import_dir = root.join("imports").join(id);
        fs::create_dir_all(&import_dir).unwrap();

        let import_json_path = import_dir.join("import.json");
        let malformed_bytes = br#"{"source_type":"image","attempt":{"generation":"two"}}"#;
        fs::write(&import_json_path, malformed_bytes).unwrap();

        let image_path = root.join("test.png");
        fs::write(&image_path, TINY_PNG).unwrap();

        // 1. admit_running_attempt refuses and bytes remain unchanged
        let admit_res = solstone_core_import::admit_running_attempt(root, id, 1000, Some("image"));
        assert!(
            admit_res.is_err(),
            "admit must refuse malformed attempt bytes"
        );
        assert_eq!(
            fs::read(&import_json_path).unwrap(),
            malformed_bytes,
            "bytes must be preserved verbatim"
        );

        // 2. run_native_producer with expected_generation fails closed and bytes remain unchanged
        let req = NativeProducerRequest {
            journal_root: root,
            source_path: &image_path,
            import_id: id,
            source: RegistrySource::Image,
            revision: None,
            password: None,
            force: false,
            expected_generation: Some(1),
            before_publication: None,
        };
        let res = run_native_producer(
            req,
            &NullWireClient,
            &NullPdfWorker,
            &crate::NullDocumentModelClient,
            &solstone_core_import::NativePublicationOperations,
        );
        assert!(
            res.is_err(),
            "producer must fail closed on malformed attempt bytes"
        );
        assert_eq!(
            fs::read(&import_json_path).unwrap(),
            malformed_bytes,
            "bytes must remain untouched after failed producer run"
        );

        // 3. Projection is Unavailable, not Success or Running with generation 1
        let proj = solstone_core_import::project_import_result(root, id);
        assert_eq!(
            proj.status,
            solstone_core_import::ProjectionStatus::Unavailable
        );
        assert_ne!(proj.status, solstone_core_import::ProjectionStatus::Success);
        assert_ne!(proj.generation, Some(1));
    }

    #[test]
    fn test_dangling_import_json_projection_unavailable_never_success() {
        let temp = tempfile::Builder::new()
            .prefix("test-dangling-import-")
            .tempdir()
            .unwrap();
        let root = temp.path();
        let id = "20260408_235500";
        let import_dir = root.join("imports").join(id);
        fs::create_dir_all(&import_dir).unwrap();

        // Put a directory where import.json is expected (or dangling link)
        let import_json_dir = import_dir.join("import.json");
        fs::create_dir(&import_json_dir).unwrap();

        // Put a successful-looking imported.json
        let imported_json_path = import_dir.join("imported.json");
        let fake_imported =
            br#"{"schema":"sol-imported/1","importer":"image","status":"success","segments":[]}"#;
        fs::write(&imported_json_path, fake_imported).unwrap();

        // project_import_result driven by read_provenance must return Unavailable, never Success
        let proj = solstone_core_import::project_import_result(root, id);
        assert_eq!(
            proj.status,
            solstone_core_import::ProjectionStatus::Unavailable
        );
        assert_ne!(proj.status, solstone_core_import::ProjectionStatus::Success);
    }

    #[test]
    fn test_stalled_running_attempt_projection_not_running() {
        let temp = tempfile::Builder::new()
            .prefix("test-stalled-attempt-")
            .tempdir()
            .unwrap();
        let root = temp.path();
        let id = "20260408_235900";

        // Admit a Running attempt with started_at_ms two hours ago (7,200,000 ms ago)
        let two_hours_ago_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64
            - 7_200_000;

        let facts =
            solstone_core_import::admit_running_attempt(root, id, two_hours_ago_ms, Some("image"))
                .unwrap();
        assert_eq!(facts.generation, 1);

        // project_import_result must not report Running forever; it must report Unconfirmed/Failed timeout status
        let proj = solstone_core_import::project_import_result(root, id);
        assert_ne!(
            proj.status,
            solstone_core_import::ProjectionStatus::Running,
            "stalled attempt older than 1h must not be projected as Running"
        );
        assert_eq!(
            proj.status,
            solstone_core_import::ProjectionStatus::Unconfirmed
        );
    }

    #[test]
    fn test_document_success_forwards_per_input_errors() {
        let temp = tempfile::Builder::new()
            .prefix("test-doc-errors-")
            .tempdir()
            .unwrap();
        let root = temp.path();
        let pdf_path = root.join("doc.pdf");
        fs::write(&pdf_path, b"%PDF-1.4 warning case").unwrap();
        let text = "Warning-bearing page text with enough characters for a document import.";
        let mut payload = document::PdfPayload {
            schema: "sol-pdf/1".to_owned(),
            engine: "test".to_owned(),
            page_count: 1,
            pages: vec![document::PdfPage {
                index: 0,
                chars: text.len(),
                text: Some(text.to_owned()),
                ..Default::default()
            }],
            ..Default::default()
        };
        payload.warnings.push("page render glitch".to_owned());
        let id = "20260408_236000";
        let req = NativeProducerRequest {
            journal_root: root,
            source_path: &pdf_path,
            import_id: id,
            source: RegistrySource::Document,
            revision: None,
            password: None,
            force: false,
            expected_generation: None,
            before_publication: None,
        };
        let res = run_native_producer(
            req,
            &NullWireClient,
            &FakePdfWorker::new(payload),
            &crate::NullDocumentModelClient,
            &solstone_core_import::NativePublicationOperations,
        );
        let outcome = res.expect("partial document import with warnings is still success");
        assert!(
            !outcome.errors.is_empty(),
            "per-input warnings must reach NativeProducerOutcome.errors"
        );
    }
}
