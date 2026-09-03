// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Native generic audio import.

use std::fs;
use std::future::Future;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::time::Duration;

use chrono::{NaiveDateTime, Timelike};
use ffmpeg_next as ffmpeg;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use solstone_core_callosum::CallosumSocketConnection;
use solstone_core_journal_io::{
    HealthMarkerKind, JsonWriteOptions, health_marker_path, write_json,
};
use solstone_core_processing_record::{
    predicate::{TerminalProofOutcome, evaluate_terminal_proof, is_failure_exhausted},
    vocab,
};
use solstone_core_segment::{ImportSource, Kind, StreamHints, touch_stream_health_marker};
use solstone_core_system_health::sanitize_os_bytes_for_terminal_bounded;
use tokio::time::{Instant, timeout};

use solstone_core_import::ImportError;
use solstone_core_import::events::{
    EventEmitter, ObservingMeta, ObservingSegment, emit_observe_observing,
};
use solstone_core_import::publish::CreatedSegment;
use solstone_core_import::staging::{ensure_import_private_chain, import_directory};

const AUDIO_RECORD_SCHEMA: &str = "solstone.import.audio.v1";
const AUDIO_RECORD_NAME: &str = "audio-import.json";
const CHUNK_SECONDS: f64 = 300.0;
/// A collision walk must remain safely short of the next five-minute chunk key.
pub const COLLISION_FORWARD_PROBE_LIMIT: u32 = 60;

/// Request for a generic stream-copy audio import.
#[derive(Clone, Debug)]
pub struct AudioImportRequest {
    pub source_media: PathBuf,
    pub journal_root: PathBuf,
    pub day: String,
    pub base_timestamp: NaiveDateTime,
    pub import_id: String,
    pub stream: String,
    pub facet: Option<String>,
    pub setting: Option<String>,
    pub wait_for_processing: bool,
    pub stall_timeout: Duration,
    pub poll_interval: Duration,
}

/// Probe failure classification used by the injectable duration seam.
#[derive(Clone, Debug)]
pub enum AudioProbeError {
    Unavailable { detail: String },
    InputUnreadable { detail: String },
}

/// Slice failure classification used by the injectable remux seam.
#[derive(Clone, Debug)]
pub enum AudioSliceError {
    /// The input was not readable as an audio container and aborts the import.
    InputUnreadable { detail: String },
    /// The input has no audio stream and aborts the import.
    NoAudioStream,
    /// A one-chunk output/remux failure that may be recorded as a dropped chunk.
    Remux { error: ffmpeg::Error },
}

impl AudioSliceError {
    fn is_tolerated(&self) -> bool {
        matches!(
            self,
            Self::Remux {
                error: ffmpeg::Error::InvalidData
                    | ffmpeg::Error::Other {
                        errno: ffmpeg::error::EINVAL
                    }
                    | ffmpeg::Error::OutputChanged
            }
        )
    }

    fn detail(&self) -> String {
        match self {
            Self::InputUnreadable { detail } => detail.clone(),
            Self::NoAudioStream => "input has no audio stream".to_owned(),
            Self::Remux { error } => error.to_string(),
        }
    }
}

/// Owned processing-wait callback. Returns a `'static` future so it can be joined.
pub type ProcessingWaitFn =
    fn(
        AudioImportRequest,
        PathBuf,
        AudioImportRecord,
    ) -> Pin<Box<dyn Future<Output = Result<ProcessingWaitOutcome, ImportError>> + Send>>;

/// Named, generic import side effects. Tests inject closures without media fixtures.
pub struct AudioImportSeams<P, S, E> {
    pub duration_probe: P,
    pub slice: S,
    pub emit_observing: E,
    pub wait: ProcessingWaitFn,
}

/// A chunk that could not be remuxed while other chunks were created.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DroppedAudioChunk {
    pub index: u64,
    pub start_offset_seconds: f64,
    pub duration_seconds: f64,
    pub reason: String,
}

/// Terminal processing status returned by optional waiting.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProcessingWaitOutcome {
    pub requested: bool,
    pub failed_segments: Vec<String>,
    pub stalled_segments: Vec<String>,
}

/// Successful outputs shared by complete and partial imports.
#[derive(Clone, Debug)]
pub struct AudioImportComplete {
    pub segments: Vec<CreatedSegment>,
    pub files_created: Vec<PathBuf>,
    pub record_path: PathBuf,
    pub processing: ProcessingWaitOutcome,
}

/// A successful import with one or more deliberately dropped remux chunks.
#[derive(Clone, Debug)]
pub struct AudioImportPartial {
    pub created: AudioImportComplete,
    pub dropped_chunks: Vec<DroppedAudioChunk>,
}

/// The structurally distinct result for full and partial audio imports.
#[derive(Clone, Debug)]
pub enum AudioImportOutcome {
    Complete(AudioImportComplete),
    Partial(AudioImportPartial),
}

impl AudioImportOutcome {
    #[must_use]
    pub fn created(&self) -> &AudioImportComplete {
        match self {
            Self::Complete(created) => created,
            Self::Partial(partial) => &partial.created,
        }
    }

    #[must_use]
    pub fn dropped_chunks(&self) -> &[DroppedAudioChunk] {
        match self {
            Self::Complete(_) => &[],
            Self::Partial(partial) => &partial.dropped_chunks,
        }
    }

    /// Partial imports must not create a positive manifest that suppresses recovery.
    #[must_use]
    pub fn writes_dedupe_manifest(&self) -> bool {
        matches!(self, Self::Complete(_))
    }
}

/// Durable, audio-owned import result record.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AudioImportRecord {
    pub schema: String,
    pub source_media: PathBuf,
    pub day: String,
    pub stream: String,
    pub import_id: String,
    pub created_segments: Vec<AudioImportRecordSegment>,
    pub dropped_chunks: Vec<DroppedAudioChunk>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub abort: Option<AudioImportAbort>,
    pub wait: AudioWaitRecord,
}

/// Context retained when audio preparation cannot complete.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AudioImportAbort {
    pub chunk_index: Option<u64>,
    pub start_offset_seconds: Option<f64>,
    pub duration_seconds: Option<f64>,
    pub reason: String,
}

/// A created segment's true source-time data, independent of its collision key.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AudioImportRecordSegment {
    pub key: String,
    pub day: String,
    pub stream: String,
    pub chunk_index: u64,
    pub start_offset_seconds: f64,
    pub start_timestamp: String,
    pub duration_seconds: f64,
    pub file_path: PathBuf,
    pub processing: AudioProcessingState,
}

/// Processing state persisted beside a native audio import.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AudioProcessingState {
    NotRequested,
    Pending,
    Succeeded,
    Failed,
    Stalled,
}

/// State of a requested processing wait.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum AudioWaitRecord {
    NotRequested,
    Waiting,
    Finished {
        failed_segments: Vec<String>,
        stalled_segments: Vec<String>,
    },
}

/// Import audio using ffmpeg-next and the journal's existing observing emitter.
pub async fn import_audio(request: AudioImportRequest) -> Result<AudioImportOutcome, ImportError> {
    let journal = request.journal_root.clone();
    import_audio_with_seams(
        request,
        AudioImportSeams {
            duration_probe: native_duration_probe,
            slice: native_remux_slice,
            emit_observing: move |segment: &ObservingSegment| {
                emit_observe_observing(&EventEmitter::new(journal.as_path(), None), segment);
            },
            wait: native_processing_wait,
        },
    )
    .await
}

/// Import audio with named, generic probe, slice, and observing seams.
pub async fn import_audio_with_seams<P, S, E>(
    request: AudioImportRequest,
    mut seams: AudioImportSeams<P, S, E>,
) -> Result<AudioImportOutcome, ImportError>
where
    P: for<'path> FnMut(&'path Path) -> Result<f64, AudioProbeError>,
    S: for<'source, 'output> FnMut(
        &'source Path,
        &'output Path,
        f64,
        f64,
    ) -> Result<(), AudioSliceError>,
    E: for<'segment> FnMut(&'segment ObservingSegment),
{
    let duration = (seams.duration_probe)(&request.source_media).map_err(|error| match error {
        AudioProbeError::Unavailable { detail } => ImportError::AudioDurationUnavailable {
            path: request.source_media.clone(),
            detail,
        },
        AudioProbeError::InputUnreadable { detail } => ImportError::AudioInputUnreadable {
            path: request.source_media.clone(),
            detail,
        },
    })?;
    if !duration.is_finite() || duration < 0.0 {
        return Err(ImportError::AudioDurationUnavailable {
            path: request.source_media.clone(),
            detail: "duration probe returned a non-finite or negative value".to_owned(),
        });
    }

    let count = (((duration + 299.0) / CHUNK_SECONDS).floor() as u64).max(1);
    let extension = request
        .source_media
        .extension()
        .and_then(|extension| extension.to_str())
        .ok_or_else(|| ImportError::AudioInputUnreadable {
            path: request.source_media.clone(),
            detail: "source media has no UTF-8 container extension".to_owned(),
        })?;
    let segment_parent = request
        .journal_root
        .join("chronicle")
        .join(&request.day)
        .join(&request.stream);
    fs::create_dir_all(&segment_parent).map_err(|error| ImportError::AudioSegmentDirectory {
        path: segment_parent.clone(),
        message: error.to_string(),
    })?;

    let mut created = Vec::new();
    let mut created_records = Vec::new();
    let mut files_created = Vec::new();
    let mut dropped_chunks = Vec::new();
    let mut observing = Vec::new();

    for index in 0..count {
        let start_offset_seconds = index as f64 * CHUNK_SECONDS;
        let chunk_duration_seconds = if index + 1 == count {
            duration - start_offset_seconds
        } else {
            CHUNK_SECONDS
        };
        let true_start = request.base_timestamp
            + chrono::Duration::seconds((index * CHUNK_SECONDS as u64) as i64);
        let key_duration = chunk_duration_seconds.ceil().max(1.0) as u64;
        let (segment, segment_dir) =
            match allocate_segment_directory(&request, &segment_parent, true_start, key_duration) {
                Ok(allocation) => allocation,
                Err(error) => {
                    write_aborted_audio_import_record(
                        &request,
                        created_records,
                        dropped_chunks,
                        AudioImportAbort {
                            chunk_index: Some(index),
                            start_offset_seconds: Some(start_offset_seconds),
                            duration_seconds: Some(chunk_duration_seconds),
                            reason: error.to_string(),
                        },
                    )?;
                    return Err(error);
                }
            };
        let output = segment_dir.join(format!("imported_audio.{extension}"));

        match (seams.slice)(
            &request.source_media,
            &output,
            start_offset_seconds,
            chunk_duration_seconds,
        ) {
            Ok(()) => {
                let hints = StreamHints {
                    kind: Some(Kind::Imported(ImportSource::Named("audio".to_owned()))),
                    host: None,
                    platform: None,
                };
                created.push(CreatedSegment {
                    day: request.day.clone(),
                    segment: segment.clone(),
                    stream: request.stream.clone(),
                    hints,
                });
                files_created.push(output.clone());
                created_records.push(AudioImportRecordSegment {
                    key: segment.clone(),
                    day: request.day.clone(),
                    stream: request.stream.clone(),
                    chunk_index: index,
                    start_offset_seconds,
                    start_timestamp: true_start.format("%Y-%m-%dT%H:%M:%S").to_string(),
                    duration_seconds: chunk_duration_seconds,
                    file_path: output.clone(),
                    processing: if request.wait_for_processing {
                        AudioProcessingState::Pending
                    } else {
                        AudioProcessingState::NotRequested
                    },
                });
                observing.push(ObservingSegment {
                    segment,
                    day: request.day.clone(),
                    files: vec![format!("imported_audio.{extension}")],
                    meta: ObservingMeta {
                        import_id: request.import_id.clone(),
                        stream: request.stream.clone(),
                        facet: request.facet.clone(),
                        setting: request.setting.clone(),
                    },
                    stream: request.stream.clone(),
                });
            }
            Err(error) if error.is_tolerated() => {
                if let Err(remove_error) = fs::remove_dir_all(&segment_dir) {
                    let error = ImportError::AudioSegmentDirectory {
                        path: segment_dir,
                        message: remove_error.to_string(),
                    };
                    write_aborted_audio_import_record(
                        &request,
                        created_records,
                        dropped_chunks,
                        AudioImportAbort {
                            chunk_index: Some(index),
                            start_offset_seconds: Some(start_offset_seconds),
                            duration_seconds: Some(chunk_duration_seconds),
                            reason: error.to_string(),
                        },
                    )?;
                    return Err(error);
                }
                dropped_chunks.push(DroppedAudioChunk {
                    index,
                    start_offset_seconds,
                    duration_seconds: chunk_duration_seconds,
                    reason: error.detail(),
                });
            }
            Err(error) => {
                let cleanup_note = fs::remove_dir_all(&segment_dir).err().map(|cleanup_error| {
                    format!("; cleanup left {}: {cleanup_error}", segment_dir.display())
                });
                let error = ImportError::AudioSliceRejected {
                    path: request.source_media.clone(),
                    chunk_index: index,
                    start_offset_seconds,
                    duration_seconds: chunk_duration_seconds,
                    detail: error.detail(),
                };
                write_aborted_audio_import_record(
                    &request,
                    created_records,
                    dropped_chunks,
                    AudioImportAbort {
                        chunk_index: Some(index),
                        start_offset_seconds: Some(start_offset_seconds),
                        duration_seconds: Some(chunk_duration_seconds),
                        reason: format!("{error}{}", cleanup_note.unwrap_or_default()),
                    },
                )?;
                return Err(error);
            }
        }
    }

    if created.is_empty() {
        let error = ImportError::NoAudioSegmentsCreated {
            path: request.source_media.clone(),
        };
        write_aborted_audio_import_record(
            &request,
            created_records,
            dropped_chunks,
            AudioImportAbort {
                chunk_index: None,
                start_offset_seconds: None,
                duration_seconds: None,
                reason: error.to_string(),
            },
        )?;
        return Err(error);
    }

    let mut record = AudioImportRecord {
        schema: AUDIO_RECORD_SCHEMA.to_owned(),
        source_media: request.source_media.clone(),
        day: request.day.clone(),
        stream: request.stream.clone(),
        import_id: request.import_id.clone(),
        created_segments: created_records,
        dropped_chunks: dropped_chunks.clone(),
        abort: None,
        wait: if request.wait_for_processing {
            AudioWaitRecord::Waiting
        } else {
            AudioWaitRecord::NotRequested
        },
    };
    let record_path =
        write_audio_import_record(&request.journal_root, &request.import_id, &record)?;

    if let Err(error) = touch_stream_health_marker(&request.journal_root, &request.day) {
        let reason = format!("could not advance stream marker after audio import: {error}");
        record.abort = Some(AudioImportAbort {
            chunk_index: None,
            start_offset_seconds: None,
            duration_seconds: None,
            reason: reason.clone(),
        });
        record.wait = AudioWaitRecord::NotRequested;
        let diagnostic_error =
            write_audio_import_record(&request.journal_root, &request.import_id, &record)
                .err()
                .map(|error| {
                    format!("; additionally could not persist the terminal record: {error}")
                })
                .unwrap_or_default();
        return Err(ImportError::StreamMarkerWrite {
            path: health_marker_path(
                &request.journal_root,
                &request.day,
                HealthMarkerKind::Stream,
            ),
            message: format!("{reason}{diagnostic_error}"),
        });
    }

    for segment in &observing {
        (seams.emit_observing)(segment);
    }

    let processing = if request.wait_for_processing {
        join_owned_wait(seams.wait, request.clone(), record_path.clone(), record).await?
    } else {
        ProcessingWaitOutcome::default()
    };

    let complete = AudioImportComplete {
        segments: created,
        files_created,
        record_path,
        processing,
    };
    if dropped_chunks.is_empty() {
        Ok(AudioImportOutcome::Complete(complete))
    } else {
        Ok(AudioImportOutcome::Partial(AudioImportPartial {
            created: complete,
            dropped_chunks,
        }))
    }
}

fn write_aborted_audio_import_record(
    request: &AudioImportRequest,
    created_segments: Vec<AudioImportRecordSegment>,
    dropped_chunks: Vec<DroppedAudioChunk>,
    abort: AudioImportAbort,
) -> Result<(), ImportError> {
    let has_created_content = !created_segments.is_empty();
    let record = AudioImportRecord {
        schema: AUDIO_RECORD_SCHEMA.to_owned(),
        source_media: request.source_media.clone(),
        day: request.day.clone(),
        stream: request.stream.clone(),
        import_id: request.import_id.clone(),
        created_segments,
        dropped_chunks,
        abort: Some(abort),
        wait: AudioWaitRecord::NotRequested,
    };
    write_audio_import_record(&request.journal_root, &request.import_id, &record)?;
    if has_created_content {
        touch_stream_health_marker(&request.journal_root, &request.day).map_err(|error| {
            ImportError::StreamMarkerWrite {
                path: health_marker_path(
                    &request.journal_root,
                    &request.day,
                    HealthMarkerKind::Stream,
                ),
                message: format!(
                    "could not advance stream marker after aborted audio import: {error}"
                ),
            }
        })?;
    }
    Ok(())
}

/// Read the audio-owned durable record for one import, if it exists.
pub fn read_audio_import_record(
    journal_root: &Path,
    import_id: &str,
) -> Result<Option<AudioImportRecord>, ImportError> {
    let path = import_directory(journal_root, import_id)?.join(AUDIO_RECORD_NAME);
    match fs::read(&path) {
        Ok(bytes) => {
            serde_json::from_slice(&bytes)
                .map(Some)
                .map_err(|error| ImportError::AudioRecordRead {
                    path,
                    message: error.to_string(),
                })
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(ImportError::AudioRecordRead {
            path,
            message: error.to_string(),
        }),
    }
}

/// Atomically write the audio-owned durable record for one import.
pub fn write_audio_import_record(
    journal_root: &Path,
    import_id: &str,
    record: &AudioImportRecord,
) -> Result<PathBuf, ImportError> {
    let path = ensure_import_private_chain(journal_root, import_id)?.join(AUDIO_RECORD_NAME);
    write_json(&path, record, JsonWriteOptions::default()).map_err(|error| {
        ImportError::AudioRecordWrite {
            path: path.clone(),
            message: error.to_string(),
        }
    })?;
    Ok(path)
}

fn allocate_segment_directory(
    request: &AudioImportRequest,
    parent: &Path,
    true_start: NaiveDateTime,
    key_duration: u64,
) -> Result<(String, PathBuf), ImportError> {
    for attempt in 0..COLLISION_FORWARD_PROBE_LIMIT {
        let candidate = true_start + chrono::Duration::seconds(i64::from(attempt));
        if candidate.format("%Y%m%d").to_string() != request.day {
            return Err(ImportError::AudioSegmentDayOverflow {
                day: request.day.clone(),
                stream: request.stream.clone(),
                start: true_start.format("%Y-%m-%dT%H:%M:%S").to_string(),
            });
        }
        let segment = format!(
            "{:02}{:02}{:02}_{key_duration}",
            candidate.hour(),
            candidate.minute(),
            candidate.second()
        );
        let path = parent.join(&segment);
        match fs::create_dir(&path) {
            Ok(()) => return Ok((segment, path)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(ImportError::AudioSegmentDirectory {
                    path,
                    message: error.to_string(),
                });
            }
        }
    }
    Err(ImportError::AudioSegmentCollision {
        day: request.day.clone(),
        stream: request.stream.clone(),
        start: true_start.format("%Y-%m-%dT%H:%M:%S").to_string(),
        attempts: COLLISION_FORWARD_PROBE_LIMIT,
    })
}

fn native_duration_probe(path: &Path) -> Result<f64, AudioProbeError> {
    ffmpeg::init().map_err(|error| AudioProbeError::Unavailable {
        detail: error.to_string(),
    })?;
    let input = ffmpeg::format::input(path).map_err(|error| AudioProbeError::InputUnreadable {
        detail: error.to_string(),
    })?;
    let raw_duration = input.duration();
    if raw_duration == ffmpeg::ffi::AV_NOPTS_VALUE {
        return Err(AudioProbeError::Unavailable {
            detail: "container duration is unavailable".to_owned(),
        });
    }
    Ok(raw_duration as f64 / f64::from(ffmpeg::ffi::AV_TIME_BASE))
}

fn native_remux_slice(
    source: &Path,
    output: &Path,
    start_seconds: f64,
    duration_seconds: f64,
) -> Result<(), AudioSliceError> {
    ffmpeg::init().map_err(|error| AudioSliceError::InputUnreadable {
        detail: error.to_string(),
    })?;
    let mut input =
        ffmpeg::format::input(source).map_err(|error| AudioSliceError::InputUnreadable {
            detail: error.to_string(),
        })?;
    let input_stream = input
        .streams()
        .best(ffmpeg::media::Type::Audio)
        .ok_or(AudioSliceError::NoAudioStream)?;
    let input_index = input_stream.index();
    let input_time_base = input_stream.time_base();
    let parameters = input_stream.parameters();
    let seek_timestamp = (start_seconds * f64::from(ffmpeg::ffi::AV_TIME_BASE)) as i64;
    input
        .seek(seek_timestamp, ..)
        .map_err(|error| AudioSliceError::Remux { error })?;

    let mut output_context =
        ffmpeg::format::output(output).map_err(|error| AudioSliceError::Remux { error })?;
    let output_index = {
        let mut output_stream = output_context
            .add_stream(ffmpeg::encoder::find(ffmpeg::codec::Id::None))
            .map_err(|error| AudioSliceError::Remux { error })?;
        output_stream.set_parameters(parameters);
        output_stream.index()
    };
    output_context
        .write_header()
        .map_err(|error| AudioSliceError::Remux { error })?;

    let end_seconds = start_seconds + duration_seconds;
    let mut output_origin_timestamp = None;
    for (stream, mut packet) in input.packets() {
        if stream.index() != input_index {
            continue;
        }
        let Some(timestamp) = packet.pts().or_else(|| packet.dts()) else {
            continue;
        };
        let packet_seconds = timestamp as f64 * f64::from(input_time_base);
        if packet_seconds < start_seconds {
            continue;
        }
        if packet_seconds >= end_seconds {
            break;
        }
        let origin_timestamp = *output_origin_timestamp.get_or_insert(timestamp);
        if let Some(pts) = packet.pts() {
            packet.set_pts(Some(pts.saturating_sub(origin_timestamp)));
        }
        if let Some(dts) = packet.dts() {
            packet.set_dts(Some(dts.saturating_sub(origin_timestamp)));
        }
        let output_time_base = output_context
            .stream(output_index)
            .expect("newly added output stream must remain present")
            .time_base();
        packet.rescale_ts(input_time_base, output_time_base);
        packet.set_position(-1);
        packet.set_stream(output_index);
        packet
            .write_interleaved(&mut output_context)
            .map_err(|error| AudioSliceError::Remux { error })?;
    }
    output_context
        .write_trailer()
        .map_err(|error| AudioSliceError::Remux { error })
}

/// Join an owned processing-wait future so a panic becomes `AudioProcessingWait`.
async fn join_owned_wait(
    wait: ProcessingWaitFn,
    request: AudioImportRequest,
    record_path: PathBuf,
    record: AudioImportRecord,
) -> Result<ProcessingWaitOutcome, ImportError> {
    match tokio::spawn(wait(request, record_path, record)).await {
        Ok(Ok(outcome)) => Ok(outcome),
        Ok(Err(error)) => Err(error),
        Err(join_error) => Err(ImportError::AudioProcessingWait {
            detail: join_error_detail(join_error),
        }),
    }
}

fn join_error_detail(error: tokio::task::JoinError) -> String {
    if error.is_cancelled() {
        return sanitize_os_bytes_for_terminal_bounded(b"audio processing wait was cancelled");
    }
    let payload = error.into_panic();
    if let Some(message) = payload.downcast_ref::<String>() {
        return sanitize_os_bytes_for_terminal_bounded(message.as_bytes());
    }
    if let Some(message) = payload.downcast_ref::<&str>() {
        return sanitize_os_bytes_for_terminal_bounded(message.as_bytes());
    }
    sanitize_os_bytes_for_terminal_bounded(b"task panicked")
}

/// Production processing wait. Boxes today's `wait_for_processing` body.
pub fn native_processing_wait(
    request: AudioImportRequest,
    record_path: PathBuf,
    mut record: AudioImportRecord,
) -> Pin<Box<dyn Future<Output = Result<ProcessingWaitOutcome, ImportError>> + Send>> {
    Box::pin(async move { wait_for_processing(&request, &record_path, &mut record).await })
}

async fn wait_for_processing(
    request: &AudioImportRequest,
    record_path: &Path,
    record: &mut AudioImportRecord,
) -> Result<ProcessingWaitOutcome, ImportError> {
    let mut connection = CallosumSocketConnection::new(
        request.journal_root.join("health/callosum.sock"),
        Map::new(),
    );
    connection.start();

    let mut last_progress = Instant::now();
    if reconcile_processing_from_disk(record) {
        last_progress = Instant::now();
        write_audio_record_at(record_path, record)?;
    }
    while record.created_segments.iter().any(is_pending) {
        let now = Instant::now();
        let deadline = last_progress + request.stall_timeout;
        if now >= deadline {
            break;
        }
        let remaining = deadline - now;
        let poll_timeout = request.poll_interval.min(remaining);
        if let Ok(Some(envelope)) = timeout(poll_timeout, connection.next_message()).await {
            let disk_progress = reconcile_processing_from_disk(record);
            let event_progress =
                apply_observed_event(record, &envelope.tract, &envelope.event, &envelope.extra);
            if disk_progress || event_progress {
                last_progress = Instant::now();
                write_audio_record_at(record_path, record)?;
            }
        }
        if reconcile_processing_from_disk(record) {
            last_progress = Instant::now();
            write_audio_record_at(record_path, record)?;
        }
    }
    connection.stop().await;
    if reconcile_processing_from_disk(record) {
        write_audio_record_at(record_path, record)?;
    }
    for segment in &mut record.created_segments {
        if segment.processing == AudioProcessingState::Pending {
            segment.processing = AudioProcessingState::Stalled;
        }
    }
    let processing = processing_outcome(record);
    record.wait = AudioWaitRecord::Finished {
        failed_segments: processing.failed_segments.clone(),
        stalled_segments: processing.stalled_segments.clone(),
    };
    write_audio_record_at(record_path, record)?;
    Ok(processing)
}

fn reconcile_processing_from_disk(record: &mut AudioImportRecord) -> bool {
    let mut progressed = false;
    for segment in &mut record.created_segments {
        if !is_pending(segment) {
            continue;
        }
        let Some(value) = read_processing_record(&segment.file_path) else {
            continue;
        };
        let input_size = fs::metadata(&segment.file_path)
            .map(|metadata| metadata.len())
            .unwrap_or_default();
        let state = if evaluate_terminal_proof(Some(&value), vocab::HANDLER_TRANSCRIBE, input_size)
            == TerminalProofOutcome::Held
        {
            Some(AudioProcessingState::Succeeded)
        } else if is_failure_exhausted(&value) {
            Some(AudioProcessingState::Failed)
        } else {
            None
        };
        if let Some(state) = state {
            segment.processing = state;
            progressed = true;
        }
    }
    progressed
}

fn read_processing_record(raw_file: &Path) -> Option<Value> {
    let sidecar = raw_file.with_extension("jsonl");
    let file = fs::File::open(sidecar).ok()?;
    let mut reader = BufReader::new(file);
    let mut row = Vec::new();
    loop {
        row.clear();
        let count = reader.read_until(b'\n', &mut row).ok()?;
        if count == 0 || row.len() > vocab::MAX_FIRST_ROW_BYTES {
            return None;
        }
        let line = std::str::from_utf8(&row).ok()?.trim();
        if !line.is_empty() {
            return serde_json::from_str::<Value>(line)
                .ok()
                .and_then(|value| value.get("_solstone_processing").cloned());
        }
    }
}

fn write_audio_record_at(path: &Path, record: &AudioImportRecord) -> Result<(), ImportError> {
    write_json(path, record, JsonWriteOptions::default()).map_err(|error| {
        ImportError::AudioRecordWrite {
            path: path.to_path_buf(),
            message: error.to_string(),
        }
    })
}

fn apply_observed_event(
    record: &mut AudioImportRecord,
    tract: &str,
    event: &str,
    fields: &Map<String, Value>,
) -> bool {
    if tract != "observe" || event != "observed" {
        return false;
    }
    let Some(key) = fields.get("segment").and_then(Value::as_str) else {
        return false;
    };
    let failed = fields.contains_key("error");
    let Some(segment) = record
        .created_segments
        .iter_mut()
        .find(|segment| segment.key == key && segment.processing == AudioProcessingState::Pending)
    else {
        return false;
    };
    segment.processing = if failed {
        AudioProcessingState::Failed
    } else {
        AudioProcessingState::Succeeded
    };
    true
}

fn is_pending(segment: &AudioImportRecordSegment) -> bool {
    segment.processing == AudioProcessingState::Pending
}

fn processing_outcome(record: &AudioImportRecord) -> ProcessingWaitOutcome {
    ProcessingWaitOutcome {
        requested: true,
        failed_segments: record
            .created_segments
            .iter()
            .filter(|segment| segment.processing == AudioProcessingState::Failed)
            .map(|segment| segment.key.clone())
            .collect(),
        stalled_segments: record
            .created_segments
            .iter()
            .filter(|segment| segment.processing == AudioProcessingState::Stalled)
            .map(|segment| segment.key.clone())
            .collect(),
    }
}

#[cfg(feature = "native-remux-corpus")]
#[doc(hidden)]
pub mod native_long_audio_remux_corpus {
    use std::collections::hash_map::DefaultHasher;
    use std::fs;
    use std::hash::{Hash, Hasher};
    use std::path::{Path, PathBuf};

    use super::*;

    const CHUNK_DURATION_SECONDS: f64 = 300.0;
    const MINIMUM_LONG_SOURCE_SECONDS: f64 = 600.0;
    const DURATION_TOLERANCE_SECONDS: f64 = 0.2;
    const MAX_PACKET_BYTES: usize = 64 * 1024;

    struct LongRemuxCase {
        name: &'static str,
        sha256: &'static str,
    }

    const LONG_REMUX_CASES: &[LongRemuxCase] = &[
        LongRemuxCase {
            name: "long-remux.m4a",
            sha256: "9fa32cc1abf18fd45df3baa262cfde34b19b3af06683fc94533ab96e54be7b03",
        },
        LongRemuxCase {
            name: "long-remux.mp3",
            sha256: "ef3d1abba9574662379855f6ca712b3da9e897e5e263da18884c9960ea98c5a8",
        },
        LongRemuxCase {
            name: "long-remux.opus",
            sha256: "af5ee11e2dbe9e27a5e2e6a0cd93278efb107ca6b2b04f5d5f37e9d7f3ec7029",
        },
        LongRemuxCase {
            name: "long-remux.wav",
            sha256: "344a3af761f706271f6d7b23105caa8758b089773762f36046c96b419636eef7",
        },
    ];

    #[derive(Debug, PartialEq, Eq)]
    struct CodecConfiguration {
        id: ffmpeg::codec::Id,
    }

    #[derive(Debug, PartialEq, Eq)]
    struct PacketSummary {
        codec: CodecConfiguration,
        packet_count: u64,
        packet_bytes: u64,
        payload_hash: u64,
        max_packet_bytes: usize,
    }

    fn repository_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..")
    }

    fn fixture_path(name: &str) -> PathBuf {
        repository_root()
            .join("core/fixtures/long_audio_remux_corpus")
            .join(name)
    }

    fn codec_configuration(stream: &ffmpeg::format::stream::Stream<'_>) -> CodecConfiguration {
        let parameters = stream.parameters();
        CodecConfiguration {
            id: parameters.id(),
        }
    }

    fn packets_in_slice(path: &Path, start_seconds: f64, duration_seconds: f64) -> PacketSummary {
        ffmpeg::init().expect("initialize FFmpeg for long remux proof");
        let mut input = ffmpeg::format::input(path).expect("open long remux fixture");
        let stream = input
            .streams()
            .best(ffmpeg::media::Type::Audio)
            .expect("long remux fixture has audio");
        let input_index = stream.index();
        let input_time_base = stream.time_base();
        let codec = codec_configuration(&stream);
        let seek_timestamp = (start_seconds * f64::from(ffmpeg::ffi::AV_TIME_BASE)) as i64;
        input
            .seek(seek_timestamp, ..)
            .expect("seek long remux fixture");

        let end_seconds = start_seconds + duration_seconds;
        let mut payload_hasher = DefaultHasher::new();
        let mut packet_count = 0_u64;
        let mut packet_bytes = 0_u64;
        let mut max_packet_bytes = 0_usize;
        let mut last_pts = None;
        let mut last_dts = None;
        for (packet_stream, packet) in input.packets() {
            if packet_stream.index() != input_index {
                continue;
            }
            let Some(timestamp) = packet.pts().or_else(|| packet.dts()) else {
                continue;
            };
            let packet_seconds = timestamp as f64 * f64::from(input_time_base);
            if packet_seconds < start_seconds {
                continue;
            }
            if packet_seconds >= end_seconds {
                break;
            }
            if let Some(pts) = packet.pts() {
                assert!(
                    last_pts.is_none_or(|last| pts >= last),
                    "non-monotonic packet PTS"
                );
                last_pts = Some(pts);
            }
            if let Some(dts) = packet.dts() {
                assert!(
                    last_dts.is_none_or(|last| dts >= last),
                    "non-monotonic packet DTS"
                );
                last_dts = Some(dts);
            }
            let payload = packet.data().unwrap_or_default();
            payload.hash(&mut payload_hasher);
            packet_count += 1;
            packet_bytes += payload.len() as u64;
            max_packet_bytes = max_packet_bytes.max(payload.len());
        }
        assert!(packet_count > 0, "long remux slice carries audio packets");
        PacketSummary {
            codec,
            packet_count,
            packet_bytes,
            payload_hash: payload_hasher.finish(),
            max_packet_bytes,
        }
    }

    pub fn run() {
        for case in LONG_REMUX_CASES {
            let source = fixture_path(case.name);
            assert_eq!(
                solstone_core_import::hash_source(&source)
                    .expect("hash long remux fixture")
                    .into_inner(),
                case.sha256,
                "fixture digest changed: {}",
                source.display()
            );
            let source_duration =
                native_duration_probe(&source).expect("read long remux source duration");
            assert!(
                source_duration > MINIMUM_LONG_SOURCE_SECONDS,
                "{} is not a longer-than-ten-minute source",
                source.display()
            );

            let temporary = tempfile::tempdir().expect("create long remux temporary directory");
            let corrupt = temporary.path().join(format!("corrupt-{}", case.name));
            fs::write(&corrupt, b"not a media container").expect("write corrupt source");
            assert!(matches!(
                native_remux_slice(&corrupt, &temporary.path().join(case.name), 0.0, 1.0),
                Err(AudioSliceError::InputUnreadable { .. })
            ));

            for (chunk_index, start_seconds) in [0.0, 300.0, 600.0].into_iter().enumerate() {
                let expected_duration =
                    (source_duration - start_seconds).clamp(0.0, CHUNK_DURATION_SECONDS);
                let expected = packets_in_slice(&source, start_seconds, expected_duration);
                let output = temporary
                    .path()
                    .join(format!("{chunk_index}-{}", case.name));
                native_remux_slice(&source, &output, start_seconds, expected_duration)
                    .expect("packet-preserving native remux succeeds");
                let actual = packets_in_slice(&output, 0.0, f64::INFINITY);
                assert_eq!(actual.codec, expected.codec, "codec configuration changes");
                assert_eq!(
                    actual.packet_count, expected.packet_count,
                    "packet count changes"
                );
                assert_eq!(
                    actual.packet_bytes, expected.packet_bytes,
                    "packet bytes change"
                );
                assert_eq!(
                    actual.payload_hash, expected.payload_hash,
                    "packet payload changes"
                );
                assert!(
                    actual.max_packet_bytes <= MAX_PACKET_BYTES,
                    "a remux packet exceeded the bounded packet budget"
                );
                let output_duration =
                    native_duration_probe(&output).expect("read packet-preserving remux duration");
                assert!(
                    (output_duration - expected_duration).abs() <= DURATION_TOLERANCE_SECONDS,
                    "remux duration {output_duration} differs from expected {expected_duration} for {}",
                    output.display()
                );
            }
        }
    }
}
