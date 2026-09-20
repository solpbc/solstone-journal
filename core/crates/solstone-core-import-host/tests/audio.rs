// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::cell::{Cell, RefCell};
use std::fs;
use std::path::Path;
use std::rc::Rc;
use std::time::Duration;

use chrono::NaiveDateTime;
use serde_json::Map;
use serde_json::{Value, json};
use solstone_core_callosum::{CallosumEnvelope, CallosumSocketServer};
use solstone_core_import::events::observing_fields;
use solstone_core_import::{ImportError, ObservingSegment};
use solstone_core_import_host::audio::{
    AudioImportOutcome, AudioImportRequest, AudioImportSeams, AudioProbeError,
    AudioProcessingState, AudioSliceError, AudioWaitRecord, import_audio_with_seams,
    native_processing_wait, read_audio_import_record,
};
use tempfile::TempDir;
use tokio::time::timeout;

const ORACLE: &str = include_str!("../../../fixtures/import_audio_oracles.json");

fn request(temp: &TempDir, import_id: &str) -> AudioImportRequest {
    AudioImportRequest {
        source_media: temp.path().join("source.m4a"),
        journal_root: temp.path().join("journal"),
        day: "20260811".to_owned(),
        base_timestamp: NaiveDateTime::parse_from_str("2026-08-11T12:00:00", "%Y-%m-%dT%H:%M:%S")
            .unwrap(),
        import_id: import_id.to_owned(),
        stream: "import.audio".to_owned(),
        facet: None,
        setting: None,
        wait_for_processing: false,
        stall_timeout: Duration::from_secs(600),
        poll_interval: Duration::from_secs(1),
    }
}

async fn fake_import(
    request: AudioImportRequest,
    duration: f64,
    failed_chunk: Option<u64>,
    emitted: Rc<RefCell<Vec<ObservingSegment>>>,
) -> Result<AudioImportOutcome, ImportError> {
    fake_import_with_calls(
        request,
        duration,
        failed_chunk,
        emitted,
        Rc::new(RefCell::new(Vec::new())),
    )
    .await
}

async fn fake_import_with_calls(
    request: AudioImportRequest,
    duration: f64,
    failed_chunk: Option<u64>,
    emitted: Rc<RefCell<Vec<ObservingSegment>>>,
    calls: Rc<RefCell<Vec<(f64, f64)>>>,
) -> Result<AudioImportOutcome, ImportError> {
    import_audio_with_seams(
        request,
        AudioImportSeams {
            duration_probe: move |_: &Path| Ok(duration),
            slice: move |_: &Path, output: &Path, start: f64, chunk_duration: f64| {
                calls.borrow_mut().push((start, chunk_duration));
                if failed_chunk == Some((start / 300.0) as u64) {
                    return Err(AudioSliceError::Remux {
                        error: ffmpeg_next::Error::InvalidData,
                    });
                }
                fs::write(output, b"audio").map_err(|error| AudioSliceError::InputUnreadable {
                    detail: error.to_string(),
                })
            },
            emit_observing: move |segment: &ObservingSegment| {
                emitted.borrow_mut().push(segment.clone());
            },
            wait: native_processing_wait,
        },
    )
    .await
}

fn created(outcome: &AudioImportOutcome) -> &solstone_core_import_host::audio::AudioImportComplete {
    outcome.created()
}

fn stream_generation(request: &AudioImportRequest) -> u64 {
    let marker = request
        .journal_root
        .join("chronicle")
        .join(&request.day)
        .join("health/stream.updated");
    let value: Value = serde_json::from_slice(&fs::read(marker).unwrap()).unwrap();
    value["generation"].as_u64().unwrap()
}

fn write_analyzed_processing_record(sidecar: &Path) {
    fs::write(
        sidecar,
        "{\"_solstone_processing\":{\"schema\":\"solstone.processing.v1\",\"state\":\"analyzed\",\"handler\":\"transcribe\",\"input_size\":5}}\n",
    )
    .unwrap();
}

async fn wait_for_segment_state(
    request: &AudioImportRequest,
    key: &str,
    expected: AudioProcessingState,
) {
    timeout(Duration::from_secs(1), async {
        loop {
            let reached = read_audio_import_record(&request.journal_root, &request.import_id)
                .ok()
                .flatten()
                .is_some_and(|record| {
                    record
                        .created_segments
                        .iter()
                        .any(|segment| segment.key == key && segment.processing == expected)
                });
            if reached {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("observed event did not update the durable audio record");
}

#[tokio::test]
async fn ac1_integral_segment_arithmetic_matches_the_vendored_oracle() {
    let oracle: Value = serde_json::from_str(ORACLE).unwrap();
    let cases = [
        ("exactly_one_chunk", 120.0),
        ("exact_multiple", 600.0),
        ("ceiling_division", 601.0),
        ("zero_duration_floors_to_one", 0.0),
    ];

    for (name, duration) in cases {
        let temp = TempDir::new().unwrap();
        let emitted = Rc::new(RefCell::new(Vec::new()));
        let calls = Rc::new(RefCell::new(Vec::new()));
        let outcome = fake_import_with_calls(
            request(&temp, name),
            duration,
            None,
            emitted.clone(),
            calls.clone(),
        )
        .await
        .unwrap();
        let expected = oracle["cases"][name]["segments_returned"].as_u64().unwrap() as usize;
        assert_eq!(created(&outcome).segments.len(), expected, "{name}");
        let expected_calls = oracle["cases"][name]["slice_calls"]
            .as_array()
            .unwrap()
            .iter()
            .map(|call| {
                (
                    call["start_seconds"].as_f64().unwrap(),
                    call["chunk_duration"].as_f64().unwrap(),
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(*calls.borrow(), expected_calls, "{name}");
        assert_eq!(
            emitted.borrow().len(),
            oracle["cases"][name]["slice_attempts"].as_u64().unwrap() as usize,
            "{name}"
        );
    }
}

#[tokio::test]
async fn ac2_fractional_segment_arithmetic_uses_the_oracle_authority() {
    let oracle: Value = serde_json::from_str(ORACLE).unwrap();
    for case in oracle["fractional_durations"]["cases"].as_array().unwrap() {
        let duration = case["duration"].as_f64().unwrap();
        let temp = TempDir::new().unwrap();
        let emitted = Rc::new(RefCell::new(Vec::new()));
        let calls = Rc::new(RefCell::new(Vec::new()));
        let _outcome = fake_import_with_calls(
            request(&temp, &format!("fractional-{duration}")),
            duration,
            None,
            emitted,
            calls.clone(),
        )
        .await
        .unwrap();
        let record = read_audio_import_record(
            &request(&temp, &format!("fractional-{duration}")).journal_root,
            &format!("fractional-{duration}"),
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            record.created_segments.len(),
            case["segments"].as_u64().unwrap() as usize
        );
        let actual = record.created_segments.last().unwrap().duration_seconds;
        let expected = case["last_chunk_seconds"].as_f64().unwrap();
        assert!((actual - expected).abs() < 0.000_001, "duration {duration}");
        let expected_calls = (0..record.created_segments.len())
            .map(|index| {
                (
                    index as f64 * 300.0,
                    if index + 1 == record.created_segments.len() {
                        expected
                    } else {
                        300.0
                    },
                )
            })
            .collect::<Vec<_>>();
        let actual_calls = calls.borrow();
        assert_eq!(
            actual_calls.len(),
            expected_calls.len(),
            "duration {duration}"
        );
        for ((actual_start, actual_duration), (expected_start, expected_duration)) in
            actual_calls.iter().zip(expected_calls)
        {
            assert!((actual_start - expected_start).abs() < 0.000_001);
            assert!((actual_duration - expected_duration).abs() < 0.000_001);
        }
    }
}

#[tokio::test]
async fn ac3_duration_probe_failure_does_not_allocate_or_slice() {
    let temp = TempDir::new().unwrap();
    let slice_called = Cell::new(false);
    let result = import_audio_with_seams(
        request(&temp, "duration-failure"),
        AudioImportSeams {
            duration_probe: |_: &Path| {
                Err(AudioProbeError::Unavailable {
                    detail: "unavailable".to_owned(),
                })
            },
            slice: |_: &Path, _: &Path, _: f64, _: f64| {
                slice_called.set(true);
                Ok(())
            },
            emit_observing: |_: &ObservingSegment| {},
            wait: native_processing_wait,
        },
    )
    .await;
    assert!(matches!(
        result,
        Err(ImportError::AudioDurationUnavailable { .. })
    ));
    assert!(!slice_called.get());

    let non_finite = import_audio_with_seams(
        request(&temp, "non-finite"),
        AudioImportSeams {
            duration_probe: |_: &Path| Ok(f64::NAN),
            slice: |_: &Path, _: &Path, _: f64, _: f64| Ok(()),
            emit_observing: |_: &ObservingSegment| {},
            wait: native_processing_wait,
        },
    )
    .await;
    assert!(matches!(
        non_finite,
        Err(ImportError::AudioDurationUnavailable { .. })
    ));
}

#[tokio::test]
async fn ac4_middle_slice_failure_is_partial_and_total_loss_aborts() {
    let temp = TempDir::new().unwrap();
    let emitted = Rc::new(RefCell::new(Vec::new()));
    let outcome = fake_import(request(&temp, "middle-failure"), 900.0, Some(1), emitted)
        .await
        .unwrap();
    let AudioImportOutcome::Partial(partial) = outcome else {
        panic!("middle failure must be structurally partial");
    };
    assert_eq!(partial.created.segments.len(), 2);
    assert_eq!(partial.dropped_chunks.len(), 1);
    assert_eq!(partial.dropped_chunks[0].index, 1);
    assert_eq!(partial.dropped_chunks[0].start_offset_seconds, 300.0);
    assert_eq!(partial.dropped_chunks[0].duration_seconds, 300.0);
    assert!(!AudioImportOutcome::Partial(partial.clone()).writes_dedupe_manifest());

    let complete_temp = TempDir::new().unwrap();
    let complete = fake_import(
        request(&complete_temp, "complete"),
        900.0,
        None,
        Rc::new(RefCell::new(Vec::new())),
    )
    .await
    .unwrap();
    assert!(matches!(complete, AudioImportOutcome::Complete(_)));
    assert!(complete.dropped_chunks().is_empty());
    assert!(complete.writes_dedupe_manifest());

    let failed_temp = TempDir::new().unwrap();
    let all_failed_request = request(&failed_temp, "all-failed");
    let all_failed = import_audio_with_seams(
        all_failed_request.clone(),
        AudioImportSeams {
            duration_probe: |_: &Path| Ok(900.0),
            slice: |_: &Path, _: &Path, _: f64, _: f64| {
                Err(AudioSliceError::Remux {
                    error: ffmpeg_next::Error::InvalidData,
                })
            },
            emit_observing: |_: &ObservingSegment| {},
            wait: native_processing_wait,
        },
    )
    .await;
    assert!(matches!(
        all_failed,
        Err(ImportError::NoAudioSegmentsCreated { .. })
    ));
    let total_loss_record = read_audio_import_record(
        &all_failed_request.journal_root,
        &all_failed_request.import_id,
    )
    .unwrap()
    .unwrap();
    assert_eq!(total_loss_record.created_segments.len(), 0);
    assert_eq!(total_loss_record.dropped_chunks.len(), 3);
    let total_loss_abort = total_loss_record.abort.as_ref().unwrap();
    assert_eq!(total_loss_abort.chunk_index, None);
    assert_eq!(total_loss_abort.start_offset_seconds, None);
    assert_eq!(total_loss_abort.duration_seconds, None);
    assert!(
        total_loss_abort
            .reason
            .contains("no audio segments created")
    );

    let destination_temp = TempDir::new().unwrap();
    let destination_request = request(&destination_temp, "destination-error");
    let destination_error = import_audio_with_seams(
        destination_request.clone(),
        AudioImportSeams {
            duration_probe: |_: &Path| Ok(601.0),
            slice: |_: &Path, output: &Path, start: f64, _: f64| {
                if start == 0.0 {
                    fs::write(output, b"audio").unwrap();
                    Ok(())
                } else {
                    Err(AudioSliceError::Remux {
                        error: ffmpeg_next::Error::Other {
                            errno: ffmpeg_next::error::EACCES,
                        },
                    })
                }
            },
            emit_observing: |_: &ObservingSegment| {},
            wait: native_processing_wait,
        },
    )
    .await;
    assert!(matches!(
        destination_error,
        Err(ImportError::AudioSliceRejected {
            chunk_index: 1,
            start_offset_seconds: 300.0,
            duration_seconds: 300.0,
            ..
        })
    ));
    let destination_record = read_audio_import_record(
        &destination_request.journal_root,
        &destination_request.import_id,
    )
    .unwrap()
    .unwrap();
    assert_eq!(destination_record.created_segments.len(), 1);
    assert!(destination_record.dropped_chunks.is_empty());
    let destination_abort = destination_record.abort.as_ref().unwrap();
    assert_eq!(destination_abort.chunk_index, Some(1));
    assert_eq!(destination_abort.start_offset_seconds, Some(300.0));
    assert_eq!(destination_abort.duration_seconds, Some(300.0));
    assert!(destination_abort.reason.contains("audio slice rejected"));
    assert!(
        destination_abort.reason.contains(
            &ffmpeg_next::Error::Other {
                errno: ffmpeg_next::error::EACCES,
            }
            .to_string()
        )
    );
}

#[tokio::test]
async fn completed_audio_import_dirties_its_exact_day_before_success() {
    let temp = TempDir::new().unwrap();
    let request = request(&temp, "marker-complete");
    let outcome = fake_import(
        request.clone(),
        120.0,
        None,
        Rc::new(RefCell::new(Vec::new())),
    )
    .await
    .unwrap();

    assert!(matches!(outcome, AudioImportOutcome::Complete(_)));
    assert_eq!(stream_generation(&request), 1);
    assert!(
        !request
            .journal_root
            .join("chronicle/20260812/health/stream.updated")
            .exists()
    );
}

#[tokio::test]
async fn partial_audio_import_with_created_content_still_dirties_its_day() {
    let temp = TempDir::new().unwrap();
    let request = request(&temp, "marker-partial");
    let outcome = fake_import(
        request.clone(),
        900.0,
        Some(1),
        Rc::new(RefCell::new(Vec::new())),
    )
    .await
    .unwrap();

    assert!(matches!(outcome, AudioImportOutcome::Partial(_)));
    assert_eq!(outcome.created().segments.len(), 2);
    assert_eq!(stream_generation(&request), 1);
}

#[tokio::test]
async fn audio_marker_failure_is_terminal_and_retains_content_and_diagnostic_record() {
    let temp = TempDir::new().unwrap();
    let request = request(&temp, "marker-failure");
    let marker = request
        .journal_root
        .join("chronicle")
        .join(&request.day)
        .join("health/stream.updated");
    fs::create_dir_all(&marker).unwrap();
    let emitted = Rc::new(RefCell::new(Vec::new()));

    let result = fake_import(request.clone(), 120.0, None, emitted.clone()).await;

    assert!(matches!(
        result,
        Err(ImportError::StreamMarkerWrite { path, .. }) if path == marker
    ));
    assert!(emitted.borrow().is_empty());
    let record = read_audio_import_record(&request.journal_root, &request.import_id)
        .unwrap()
        .unwrap();
    assert_eq!(record.created_segments.len(), 1);
    assert!(record.created_segments[0].file_path.is_file());
    assert!(
        record
            .abort
            .as_ref()
            .unwrap()
            .reason
            .contains("could not advance stream marker")
    );
    assert_eq!(record.wait, AudioWaitRecord::NotRequested);
}

#[tokio::test]
async fn ac5_dropped_chunks_are_read_back_from_the_durable_record() {
    let temp = TempDir::new().unwrap();
    let import_id = "durable-drop";
    let outcome = fake_import(
        request(&temp, import_id),
        900.0,
        Some(1),
        Rc::new(RefCell::new(Vec::new())),
    )
    .await
    .unwrap();
    let record = read_audio_import_record(&request(&temp, import_id).journal_root, import_id)
        .unwrap()
        .unwrap();
    assert_eq!(record.dropped_chunks, outcome.dropped_chunks());
    assert_eq!(
        record.dropped_chunks[0].reason,
        ffmpeg_next::Error::InvalidData.to_string()
    );
}

#[tokio::test]
async fn ac6_allocation_is_exclusive_bounded_and_cleans_failed_leaves() {
    let temp = TempDir::new().unwrap();
    let initial = request(&temp, "collision");
    let parent = initial
        .journal_root
        .join("chronicle")
        .join(&initial.day)
        .join(&initial.stream);
    fs::create_dir_all(parent.join("120000_300")).unwrap();
    fs::write(parent.join("120000_300/marker"), b"original").unwrap();
    let outcome = fake_import(
        initial.clone(),
        300.0,
        None,
        Rc::new(RefCell::new(Vec::new())),
    )
    .await
    .unwrap();
    assert_ne!(created(&outcome).segments[0].segment, "120000_300");
    assert_eq!(
        fs::read(parent.join("120000_300/marker")).unwrap(),
        b"original"
    );

    let cleanup_temp = TempDir::new().unwrap();
    let failed = fake_import(
        request(&cleanup_temp, "cleanup"),
        900.0,
        Some(1),
        Rc::new(RefCell::new(Vec::new())),
    )
    .await
    .unwrap();
    let cleanup_parent = request(&cleanup_temp, "cleanup")
        .journal_root
        .join("chronicle/20260811/import.audio");
    assert_eq!(
        fs::read_dir(&cleanup_parent).unwrap().count(),
        created(&failed).segments.len()
    );

    let exhaustion = TempDir::new().unwrap();
    let exhaustion_request = request(&exhaustion, "exhaustion");
    let exhaustion_parent = exhaustion_request
        .journal_root
        .join("chronicle")
        .join(&exhaustion_request.day)
        .join(&exhaustion_request.stream);
    for second in 0..60 {
        fs::create_dir_all(exhaustion_parent.join(format!("1200{second:02}_300"))).unwrap();
    }
    let exhausted = fake_import(
        exhaustion_request,
        300.0,
        None,
        Rc::new(RefCell::new(Vec::new())),
    )
    .await;
    assert!(matches!(
        exhausted,
        Err(ImportError::AudioSegmentCollision { attempts: 60, .. })
    ));

    let mid_loop = TempDir::new().unwrap();
    let mid_loop_request = request(&mid_loop, "mid-loop-collision");
    let mid_loop_parent = mid_loop_request
        .journal_root
        .join("chronicle")
        .join(&mid_loop_request.day)
        .join(&mid_loop_request.stream);
    for second in 0..60 {
        fs::create_dir_all(mid_loop_parent.join(format!("1205{second:02}_300"))).unwrap();
    }
    let mid_loop_error = fake_import(
        mid_loop_request.clone(),
        601.0,
        None,
        Rc::new(RefCell::new(Vec::new())),
    )
    .await;
    assert!(matches!(
        mid_loop_error,
        Err(ImportError::AudioSegmentCollision { .. })
    ));
    let mid_loop_record =
        read_audio_import_record(&mid_loop_request.journal_root, &mid_loop_request.import_id)
            .unwrap()
            .unwrap();
    assert_eq!(mid_loop_record.created_segments.len(), 1);
    let mid_loop_abort = mid_loop_record.abort.as_ref().unwrap();
    assert_eq!(mid_loop_abort.chunk_index, Some(1));
    assert_eq!(mid_loop_abort.start_offset_seconds, Some(300.0));
    assert_eq!(mid_loop_abort.duration_seconds, Some(300.0));
    assert!(mid_loop_abort.reason.contains("audio segment collision"));
    assert!(
        mid_loop_record.created_segments[0].file_path.is_file(),
        "the already-created audio remains recorded rather than being deleted"
    );

    let midnight = TempDir::new().unwrap();
    let mut midnight_request = request(&midnight, "midnight");
    midnight_request.base_timestamp =
        NaiveDateTime::parse_from_str("2026-08-11T23:59:59", "%Y-%m-%dT%H:%M:%S").unwrap();
    let midnight_parent = midnight_request
        .journal_root
        .join("chronicle")
        .join(&midnight_request.day)
        .join(&midnight_request.stream);
    fs::create_dir_all(midnight_parent.join("235959_300")).unwrap();
    let overflow = fake_import(
        midnight_request,
        300.0,
        None,
        Rc::new(RefCell::new(Vec::new())),
    )
    .await;
    assert!(matches!(
        overflow,
        Err(ImportError::AudioSegmentDayOverflow { .. })
    ));
}

#[tokio::test]
async fn ac7_record_keeps_true_source_range_independent_of_the_key() {
    let temp = TempDir::new().unwrap();
    let request = request(&temp, "true-range");
    let parent = request
        .journal_root
        .join("chronicle")
        .join(&request.day)
        .join(&request.stream);
    fs::create_dir_all(parent.join("120000_300")).unwrap();
    let outcome = fake_import(
        request.clone(),
        601.482,
        None,
        Rc::new(RefCell::new(Vec::new())),
    )
    .await
    .unwrap();
    let record = read_audio_import_record(&request.journal_root, &request.import_id)
        .unwrap()
        .unwrap();
    let first = record.created_segments.first().unwrap();
    assert_eq!(first.start_offset_seconds, 0.0);
    assert_eq!(first.start_timestamp, "2026-08-11T12:00:00");
    assert_ne!(&first.key[..6], "120000");
    let last = record.created_segments.last().unwrap();
    assert_eq!(last.start_offset_seconds, 600.0);
    assert_eq!(last.start_timestamp, "2026-08-11T12:10:00");
    assert!((last.duration_seconds - 1.482).abs() < 0.000_001);
    assert!(last.key.ends_with("_2"));
    assert_ne!(last.key.rsplit('_').next().unwrap(), "1");
    assert_eq!(created(&outcome).segments.last().unwrap().segment, last.key);
}

#[tokio::test]
async fn ac8_emit_seam_receives_one_complete_observing_event_per_segment() {
    let temp = TempDir::new().unwrap();
    let mut import_request = request(&temp, "with-meta");
    import_request.facet = Some("work".to_owned());
    import_request.setting = Some("desk".to_owned());
    let emitted = Rc::new(RefCell::new(Vec::new()));
    let outcome = fake_import(import_request.clone(), 600.0, None, emitted.clone())
        .await
        .unwrap();
    {
        let events = emitted.borrow();
        assert_eq!(events.len(), created(&outcome).segments.len());
        for (event, segment) in events.iter().zip(&created(&outcome).segments) {
            assert_eq!(event.segment, segment.segment);
            assert_eq!(event.day, segment.day);
            assert_eq!(event.stream, segment.stream);
            assert_eq!(event.files, vec!["imported_audio.m4a".to_owned()]);
            assert_eq!(event.meta.import_id, import_request.import_id);
            assert_eq!(event.meta.stream, import_request.stream);
            assert_eq!(event.meta.facet.as_deref(), Some("work"));
            assert_eq!(event.meta.setting.as_deref(), Some("desk"));
            let fields = observing_fields(event);
            assert_eq!(fields["batch"], true);
            assert_eq!(fields["files"], json!(["imported_audio.m4a"]));
            assert_eq!(fields["meta"]["facet"], "work");
            assert_eq!(fields["meta"]["setting"], "desk");
        }
    }

    let without_meta = TempDir::new().unwrap();
    let emitted_without_meta = Rc::new(RefCell::new(Vec::new()));
    fake_import(
        request(&without_meta, "without-meta"),
        120.0,
        None,
        emitted_without_meta.clone(),
    )
    .await
    .unwrap();
    assert!(emitted_without_meta.borrow()[0].meta.facet.is_none());
    assert!(emitted_without_meta.borrow()[0].meta.setting.is_none());
    let fields = observing_fields(&emitted_without_meta.borrow()[0]);
    assert_eq!(fields["files"], json!(["imported_audio.m4a"]));
    assert!(fields["meta"].get("facet").is_none());
    assert!(fields["meta"].get("setting").is_none());
}

#[tokio::test]
async fn ac9_wait_false_returns_after_emission_without_processing_wait() {
    let temp = TempDir::new().unwrap();
    let emitted = Rc::new(RefCell::new(Vec::new()));
    let outcome = fake_import(request(&temp, "no-wait"), 600.0, None, emitted.clone())
        .await
        .unwrap();
    assert_eq!(emitted.borrow().len(), 2);
    assert!(!created(&outcome).processing.requested);
    let record = read_audio_import_record(&request(&temp, "no-wait").journal_root, "no-wait")
        .unwrap()
        .unwrap();
    assert!(matches!(record.wait, AudioWaitRecord::NotRequested));
    assert!(
        record
            .created_segments
            .iter()
            .all(|segment| segment.processing == AudioProcessingState::NotRequested)
    );
}

#[tokio::test]
async fn ac10_wait_reconciles_disk_and_reports_failures_without_partial() {
    let success_temp = TempDir::new().unwrap();
    let mut success_request = request(&success_temp, "dropped-event");
    success_request.wait_for_processing = true;
    success_request.stall_timeout = Duration::from_millis(20);
    success_request.poll_interval = Duration::from_millis(1);
    let success = import_audio_with_seams(
        success_request.clone(),
        AudioImportSeams {
            duration_probe: |_: &Path| Ok(120.0),
            slice: |_: &Path, output: &Path, _: f64, _: f64| {
                fs::write(output, b"audio").unwrap();
                fs::write(
                    output.with_extension("jsonl"),
                    format!(
                        "{{\"_solstone_processing\":{}}}\n",
                        json!({
                            "schema": "solstone.processing.v1",
                            "state": "analyzed",
                            "handler": "transcribe",
                            "input_size": 5,
                        })
                    ),
                )
                .unwrap();
                Ok(())
            },
            emit_observing: |_: &ObservingSegment| {},
            wait: native_processing_wait,
        },
    )
    .await
    .unwrap();
    assert!(matches!(success, AudioImportOutcome::Complete(_)));
    assert!(created(&success).processing.failed_segments.is_empty());
    assert!(created(&success).processing.stalled_segments.is_empty());
    let success_record =
        read_audio_import_record(&success_request.journal_root, &success_request.import_id)
            .unwrap()
            .unwrap();
    assert_eq!(
        success_record.created_segments[0].processing,
        AudioProcessingState::Succeeded
    );

    let during_temp = TempDir::new().unwrap();
    let mut during_request = request(&during_temp, "during-loop");
    during_request.wait_for_processing = true;
    during_request.stall_timeout = Duration::from_millis(50);
    during_request.poll_interval = Duration::from_millis(1);
    let during_wait_request = during_request.clone();
    let during_sidecar = during_request
        .journal_root
        .join("chronicle/20260811/import.audio/120500_300/imported_audio.jsonl");
    let during_server =
        CallosumSocketServer::bind(during_request.journal_root.join("health/callosum.sock"))
            .await
            .unwrap();
    let send_during_event = async {
        while during_server.client_count() == 0 {
            tokio::task::yield_now().await;
        }
        assert!(during_server.broadcast(CallosumEnvelope {
            tract: "observe".to_owned(),
            event: "observed".to_owned(),
            ts: None,
            extra: Map::from_iter([(String::from("segment"), json!("120000_300"))]),
        }));
        wait_for_segment_state(
            &during_wait_request,
            "120000_300",
            AudioProcessingState::Succeeded,
        )
        .await;
        write_analyzed_processing_record(&during_sidecar);
    };
    let during_import = import_audio_with_seams(
        during_request.clone(),
        AudioImportSeams {
            duration_probe: |_: &Path| Ok(600.0),
            slice: |_: &Path, output: &Path, _: f64, _: f64| {
                fs::write(output, b"audio").unwrap();
                Ok(())
            },
            emit_observing: |_: &ObservingSegment| {},
            wait: native_processing_wait,
        },
    );
    let (during, ()) = tokio::join!(during_import, send_during_event);
    during_server.stop().await;
    let during = during.unwrap();
    assert!(created(&during).processing.failed_segments.is_empty());
    assert!(created(&during).processing.stalled_segments.is_empty());
    let during_record =
        read_audio_import_record(&during_request.journal_root, &during_request.import_id)
            .unwrap()
            .unwrap();
    assert!(
        during_record
            .created_segments
            .iter()
            .all(|segment| segment.processing == AudioProcessingState::Succeeded)
    );

    let after_temp = TempDir::new().unwrap();
    let mut after_request = request(&after_temp, "after-loop");
    after_request.wait_for_processing = true;
    // This arm asserts the ABSENCE of stalls, so its budget only needs to be longer than a
    // loaded machine's reconcile. At 20ms it was a concurrency detector rather than a stall
    // detector: enough parallel work in this binary and a healthy segment reads as stalled.
    // The arm that must actually observe a stall (failure_request, below) keeps its short one.
    after_request.stall_timeout = Duration::from_secs(10);
    after_request.poll_interval = Duration::from_millis(20);
    let after_wait_request = after_request.clone();
    let after_sidecar = after_request
        .journal_root
        .join("chronicle/20260811/import.audio/120500_300/imported_audio.jsonl");
    let after_server =
        CallosumSocketServer::bind(after_request.journal_root.join("health/callosum.sock"))
            .await
            .unwrap();
    let send_last_event = async {
        while after_server.client_count() == 0 {
            tokio::task::yield_now().await;
        }
        assert!(after_server.broadcast(CallosumEnvelope {
            tract: "observe".to_owned(),
            event: "observed".to_owned(),
            ts: None,
            extra: Map::from_iter([(String::from("segment"), json!("120000_300"))]),
        }));
        wait_for_segment_state(
            &after_wait_request,
            "120000_300",
            AudioProcessingState::Succeeded,
        )
        .await;
        write_analyzed_processing_record(&after_sidecar);
    };
    let after_import = import_audio_with_seams(
        after_request.clone(),
        AudioImportSeams {
            duration_probe: |_: &Path| Ok(600.0),
            slice: |_: &Path, output: &Path, _: f64, _: f64| {
                fs::write(output, b"audio").unwrap();
                Ok(())
            },
            emit_observing: |_: &ObservingSegment| {},
            wait: native_processing_wait,
        },
    );
    let (after, ()) = tokio::join!(after_import, send_last_event);
    after_server.stop().await;
    let after = after.unwrap();
    assert!(created(&after).processing.failed_segments.is_empty());
    assert!(created(&after).processing.stalled_segments.is_empty());
    let after_record =
        read_audio_import_record(&after_request.journal_root, &after_request.import_id)
            .unwrap()
            .unwrap();
    assert!(
        after_record
            .created_segments
            .iter()
            .all(|segment| segment.processing == AudioProcessingState::Succeeded)
    );

    let failure_temp = TempDir::new().unwrap();
    let mut failure_request = request(&failure_temp, "processing-failure");
    failure_request.wait_for_processing = true;
    failure_request.stall_timeout = Duration::from_millis(20);
    failure_request.poll_interval = Duration::from_millis(1);
    let failure = import_audio_with_seams(
        failure_request.clone(),
        AudioImportSeams {
            duration_probe: |_: &Path| Ok(120.0),
            slice: |_: &Path, output: &Path, _: f64, _: f64| {
                fs::write(output, b"audio").unwrap();
                fs::write(
                    output.with_extension("jsonl"),
                    format!(
                        "{{\"_solstone_processing\":{}}}\n",
                        json!({
                            "schema": "solstone.processing.v1",
                            "state": "failed",
                            "attempts": 3,
                        })
                    ),
                )
                .unwrap();
                Ok(())
            },
            emit_observing: |_: &ObservingSegment| {},
            wait: native_processing_wait,
        },
    )
    .await
    .unwrap();
    assert!(matches!(failure, AudioImportOutcome::Complete(_)));
    assert_eq!(created(&failure).processing.failed_segments.len(), 1);
    let failure_record =
        read_audio_import_record(&failure_request.journal_root, &failure_request.import_id)
            .unwrap()
            .unwrap();
    assert_eq!(
        failure_record.created_segments[0].processing,
        AudioProcessingState::Failed
    );

    let stalled_temp = TempDir::new().unwrap();
    let mut stalled_request = request(&stalled_temp, "stall");
    stalled_request.wait_for_processing = true;
    stalled_request.stall_timeout = Duration::from_millis(2);
    stalled_request.poll_interval = Duration::from_millis(1);
    let stalled = fake_import(
        stalled_request,
        120.0,
        None,
        Rc::new(RefCell::new(Vec::new())),
    )
    .await
    .unwrap();
    assert_eq!(created(&stalled).processing.stalled_segments.len(), 1);
    let stalled_record = read_audio_import_record(&stalled_temp.path().join("journal"), "stall")
        .unwrap()
        .unwrap();
    assert_eq!(
        stalled_record.created_segments[0].processing,
        AudioProcessingState::Stalled
    );

    let event_temp = TempDir::new().unwrap();
    let mut event_request = request(&event_temp, "event-without-sidecar");
    event_request.wait_for_processing = true;
    event_request.stall_timeout = Duration::from_millis(100);
    event_request.poll_interval = Duration::from_millis(1);
    let server =
        CallosumSocketServer::bind(event_request.journal_root.join("health/callosum.sock"))
            .await
            .unwrap();
    let wait_for_client = async {
        while server.client_count() == 0 {
            tokio::task::yield_now().await;
        }
        assert!(server.broadcast(CallosumEnvelope {
            tract: "observe".to_owned(),
            event: "observed".to_owned(),
            ts: None,
            extra: Map::from_iter([(String::from("segment"), json!("120000_120"))]),
        }));
    };
    let import = import_audio_with_seams(
        event_request.clone(),
        AudioImportSeams {
            duration_probe: |_: &Path| Ok(120.0),
            slice: |_: &Path, output: &Path, _: f64, _: f64| {
                fs::write(output, b"audio").unwrap();
                Ok(())
            },
            emit_observing: |_: &ObservingSegment| {},
            wait: native_processing_wait,
        },
    );
    let (event_outcome, ()) = tokio::join!(import, wait_for_client);
    server.stop().await;
    let event_outcome = event_outcome.unwrap();
    assert!(
        created(&event_outcome)
            .processing
            .failed_segments
            .is_empty()
    );
    assert!(
        created(&event_outcome)
            .processing
            .stalled_segments
            .is_empty()
    );
    let event_record =
        read_audio_import_record(&event_request.journal_root, &event_request.import_id)
            .unwrap()
            .unwrap();
    assert_eq!(
        event_record.created_segments[0].processing,
        AudioProcessingState::Succeeded
    );
}

#[cfg(feature = "native-remux-corpus")]
#[test]
fn native_remux_preserves_long_form_audio_packets_without_unbounded_buffers() {
    solstone_core_import_host::audio::native_long_audio_remux_corpus::run();
}

// --- Half A: the import row reports its true outcome -------------------------------
//
// Before this, a generic audio import wrote no attempt facts, so the web reader fell to
// its legacy rule and reported `Failed("Import never completed", "timeout")` an hour
// after upload over content sitting safely on disk. These pin the real outcomes.

use solstone_core_import::{
    AttemptState, ProjectionStatus, admit_running_attempt, get_attempt_facts,
    project_import_result, read_import_metadata,
};
use solstone_core_import_host::audio_publication::finish_audio_attempt;

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// Admit an attempt the way `run_audio` does, returning its generation.
///
/// The start stamp must be a real recent clock, not a small constant. At `started_at_ms = 1000`
/// every admitted attempt is dated 1970, so it instantly exceeds the one-hour Running bound and
/// the projection reports `Unconfirmed` no matter what the terminal write did -- which silently
/// turned the stalled-segment test into a pass that survived deleting the code under test.
fn admit(request: &AudioImportRequest) -> u64 {
    admit_running_attempt(&request.journal_root, &request.import_id, now_ms(), None)
        .unwrap()
        .generation
}

fn stream_record_seq(journal_root: &Path, stream: &str) -> Option<u64> {
    let path = journal_root.join("streams").join(format!("{stream}.json"));
    let value: Value = serde_json::from_slice(&fs::read(path).ok()?).ok()?;
    value.get("seq").and_then(Value::as_u64)
}

fn imported_json_bytes(request: &AudioImportRequest) -> Option<Vec<u8>> {
    fs::read(
        request
            .journal_root
            .join("imports")
            .join(&request.import_id)
            .join("imported.json"),
    )
    .ok()
}

/// A clean import reads `success`, not the legacy timeout failure.
#[tokio::test]
async fn a_clean_audio_import_projects_success() {
    let temp = TempDir::new().unwrap();
    fs::write(temp.path().join("source.m4a"), b"source").unwrap();
    let request = request(&temp, "20260811_120000");
    let generation = admit(&request);

    let outcome = fake_import(
        request.clone(),
        120.0,
        None,
        Rc::new(RefCell::new(Vec::new())),
    )
    .await;
    finish_audio_attempt(
        &request.journal_root,
        &request.import_id,
        generation,
        &outcome,
    );

    let projection = project_import_result(&request.journal_root, &request.import_id);
    assert_eq!(projection.status, ProjectionStatus::Success);
    assert!(!projection.has_gaps, "a clean import has no gaps");
    // The metrics generalization: a successful row renders a real count, not null.
    assert_eq!(projection.entries_written, Some(1));
    // source_type comes from the publication's stream prefix, never from a source hint.
    assert_eq!(projection.source_type, "audio");
}

/// A dropped chunk keeps the import a success and marks it as carrying gaps.
#[tokio::test]
async fn a_dropped_chunk_projects_success_with_gaps() {
    let temp = TempDir::new().unwrap();
    fs::write(temp.path().join("source.m4a"), b"source").unwrap();
    let request = request(&temp, "20260811_130000");
    let generation = admit(&request);

    // 900s of audio is three 300s chunks; drop the middle one.
    let outcome = fake_import(
        request.clone(),
        900.0,
        Some(1),
        Rc::new(RefCell::new(Vec::new())),
    )
    .await;
    finish_audio_attempt(
        &request.journal_root,
        &request.import_id,
        generation,
        &outcome,
    );

    let projection = project_import_result(&request.journal_root, &request.import_id);
    assert_eq!(projection.status, ProjectionStatus::Success);
    assert!(
        projection.has_gaps,
        "a dropped chunk is a gap, not a silent success"
    );
}

/// N created segments advance the stream record by exactly N.
///
/// No "a replay advances by zero" twin: `replayable_unbound_advance` matches only when the
/// segment is the record head, so replaying N>1 segments advances by N. That is the tree's
/// real behaviour and pinning the opposite would have been pinning a wish.
#[tokio::test]
async fn publishing_n_segments_advances_the_stream_record_by_n() {
    let temp = TempDir::new().unwrap();
    fs::write(temp.path().join("source.m4a"), b"source").unwrap();
    let request = request(&temp, "20260811_140000");
    let generation = admit(&request);

    let outcome = fake_import(
        request.clone(),
        900.0,
        None,
        Rc::new(RefCell::new(Vec::new())),
    )
    .await;
    let created_count = created(outcome.as_ref().unwrap()).segments.len() as u64;
    assert_eq!(created_count, 3, "900s of audio is three chunks");

    finish_audio_attempt(
        &request.journal_root,
        &request.import_id,
        generation,
        &outcome,
    );

    assert_eq!(
        stream_record_seq(&request.journal_root, "import.audio"),
        Some(created_count),
        "each created segment advances the chain exactly once"
    );
}

/// The stream record an audio publication creates is labelled as an import.
///
/// `Kind::Imported(_)` flattens to the compat label `"import"` -- the `Named("audio")`
/// payload is discarded by the record writer -- so `"import"` is the observable that
/// exists. What matters is that it is never `"unknown"`, which is what re-deriving the
/// segments instead of using the producer's own `CreatedSegment` list would have minted.
#[tokio::test]
async fn an_audio_publication_labels_its_stream_record_as_an_import() {
    let temp = TempDir::new().unwrap();
    fs::write(temp.path().join("source.m4a"), b"source").unwrap();
    let request = request(&temp, "20260811_150000");
    let generation = admit(&request);

    let outcome = fake_import(
        request.clone(),
        120.0,
        None,
        Rc::new(RefCell::new(Vec::new())),
    )
    .await;
    finish_audio_attempt(
        &request.journal_root,
        &request.import_id,
        generation,
        &outcome,
    );

    let path = request.journal_root.join("streams/import.audio.json");
    let record: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    assert_eq!(record.get("kind").and_then(Value::as_str), Some("import"));
}

/// A superseded generation writes nothing, advances nothing and emits nothing.
///
/// The control in the same binary is the point: every assertion here is an absence, and
/// absences are all trivially true on a tree where audio never published at all. The
/// control fails unless the new path is live.
#[tokio::test]
async fn a_superseded_generation_touches_nothing_while_a_live_one_publishes() {
    let temp = TempDir::new().unwrap();
    fs::write(temp.path().join("source.m4a"), b"source").unwrap();

    // Control: a live generation does publish and does advance the chain.
    let live = request(&temp, "20260811_160000");
    let live_generation = admit(&live);
    let live_outcome =
        fake_import(live.clone(), 120.0, None, Rc::new(RefCell::new(Vec::new()))).await;
    finish_audio_attempt(
        &live.journal_root,
        &live.import_id,
        live_generation,
        &live_outcome,
    );
    assert!(
        imported_json_bytes(&live).is_some(),
        "control: a live generation writes a publication record"
    );
    let seq_after_live = stream_record_seq(&live.journal_root, "import.audio");
    assert_eq!(seq_after_live, Some(1), "control: the chain advanced");

    // Superseded: generation 1 finishes after generation 2 has been admitted.
    let stale = request(&temp, "20260811_170000");
    let stale_generation = admit(&stale);
    let stale_outcome = fake_import(
        stale.clone(),
        120.0,
        None,
        Rc::new(RefCell::new(Vec::new())),
    )
    .await;
    let successor = admit_running_attempt(&stale.journal_root, &stale.import_id, now_ms(), None)
        .unwrap()
        .generation;
    assert!(successor > stale_generation, "a successor was admitted");

    let metadata_before = read_import_metadata(&stale.journal_root, &stale.import_id).unwrap();
    let seq_before = stream_record_seq(&stale.journal_root, "import.audio");

    finish_audio_attempt(
        &stale.journal_root,
        &stale.import_id,
        stale_generation,
        &stale_outcome,
    );

    assert!(
        imported_json_bytes(&stale).is_none(),
        "a superseded child must not write a publication record"
    );
    assert_eq!(
        stream_record_seq(&stale.journal_root, "import.audio"),
        seq_before,
        "a superseded child must not advance the stream record"
    );
    let metadata_after = read_import_metadata(&stale.journal_root, &stale.import_id).unwrap();
    assert_eq!(
        serde_json::to_value(&metadata_before).unwrap(),
        serde_json::to_value(&metadata_after).unwrap(),
        "a superseded child must not rewrite import.json"
    );
    // The successor is still live and untouched.
    let facts = get_attempt_facts(&metadata_after).unwrap();
    assert_eq!(facts.generation, successor);
    assert_eq!(facts.state, AttemptState::Running);
}

/// An abort records a real failure and publishes nothing.
///
/// Every abort path returns `Err` and so carries no created-segment list; segments made
/// before the abort stay unpublished, which is a named residual of this part rather than
/// an oversight. The control is the same shape as the superseded test's: "no publication
/// record" is trivially true wherever audio never published.
#[tokio::test]
async fn an_aborted_import_reads_failed_and_publishes_nothing() {
    let temp = TempDir::new().unwrap();
    fs::write(temp.path().join("source.m4a"), b"source").unwrap();

    let control = request(&temp, "20260811_180000");
    let control_generation = admit(&control);
    let control_outcome = fake_import(
        control.clone(),
        120.0,
        None,
        Rc::new(RefCell::new(Vec::new())),
    )
    .await;
    finish_audio_attempt(
        &control.journal_root,
        &control.import_id,
        control_generation,
        &control_outcome,
    );
    assert!(
        imported_json_bytes(&control).is_some(),
        "control: a successful import does write a publication record"
    );

    // Total loss: the only chunk fails to remux, so the import aborts.
    let aborted = request(&temp, "20260811_190000");
    let aborted_generation = admit(&aborted);
    let aborted_outcome = fake_import(
        aborted.clone(),
        120.0,
        Some(0),
        Rc::new(RefCell::new(Vec::new())),
    )
    .await;
    assert!(aborted_outcome.is_err(), "a total loss aborts");
    finish_audio_attempt(
        &aborted.journal_root,
        &aborted.import_id,
        aborted_generation,
        &aborted_outcome,
    );

    assert!(
        imported_json_bytes(&aborted).is_none(),
        "an abort publishes nothing"
    );
    let projection = project_import_result(&aborted.journal_root, &aborted.import_id);
    assert_eq!(
        projection.status,
        ProjectionStatus::Failed,
        "an abort is a definitive failure, not an unconfirmed one"
    );
}

/// A row with no attempt keeps the landed legacy rule, unchanged.
///
/// This is the symptom the whole part exists to remove, and the memo requires that rows
/// written by earlier releases keep their base classification. Asserted in the same binary
/// as an admitted row, so it only passes while the new path is live.
#[tokio::test]
async fn a_row_with_no_attempt_keeps_the_landed_legacy_rule() {
    let temp = TempDir::new().unwrap();
    fs::write(temp.path().join("source.m4a"), b"source").unwrap();

    // The admitted row, so this guard cannot be green on a tree where nothing was built.
    let admitted = request(&temp, "20260811_200000");
    let admitted_generation = admit(&admitted);
    let admitted_outcome = fake_import(
        admitted.clone(),
        120.0,
        None,
        Rc::new(RefCell::new(Vec::new())),
    )
    .await;
    finish_audio_attempt(
        &admitted.journal_root,
        &admitted.import_id,
        admitted_generation,
        &admitted_outcome,
    );
    assert_eq!(
        project_import_result(&admitted.journal_root, &admitted.import_id).status,
        ProjectionStatus::Success
    );

    // The legacy row: a web-started import that never finalized, older than the bound.
    let legacy_id = "20260811_210000";
    let legacy_dir = admitted.journal_root.join("imports").join(legacy_id);
    fs::create_dir_all(&legacy_dir).unwrap();
    let two_hours_ago_ms = (std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64)
        .saturating_sub(7_200_000);
    let mut legacy = Map::new();
    legacy.insert("task_id".to_owned(), json!("1755000000000"));
    legacy.insert("upload_timestamp".to_owned(), json!(two_hours_ago_ms));
    fs::write(
        legacy_dir.join("import.json"),
        serde_json::to_vec(&Value::Object(legacy)).unwrap(),
    )
    .unwrap();

    let projection = project_import_result(&admitted.journal_root, legacy_id);
    assert_eq!(projection.status, ProjectionStatus::Failed);
    assert_eq!(
        projection.error.as_deref(),
        Some("Import never completed"),
        "the landed legacy rule is preserved verbatim for rows with no attempt"
    );
    assert_eq!(projection.error_stage.as_deref(), Some("timeout"));
}

/// A segment whose processing failed reads `failed`.
///
/// The shared `request()` helper leaves the processing wait off, so without this the
/// `failed_segments` arm of the terminal classification never executed at all.
#[tokio::test]
async fn a_failed_segment_projects_failed() {
    let temp = TempDir::new().unwrap();
    fs::write(temp.path().join("source.m4a"), b"source").unwrap();
    let mut req = request(&temp, "20260811_220000");
    req.wait_for_processing = true;
    req.stall_timeout = Duration::from_secs(10);
    req.poll_interval = Duration::from_millis(1);
    let generation = admit(&req);

    let outcome = import_audio_with_seams(
        req.clone(),
        AudioImportSeams {
            duration_probe: |_: &Path| Ok(120.0),
            slice: |_: &Path, output: &Path, _: f64, _: f64| {
                fs::write(output, b"audio").unwrap();
                fs::write(
                    output.with_extension("jsonl"),
                    format!(
                        "{{\"_solstone_processing\":{}}}\n",
                        json!({"schema": "solstone.processing.v1", "state": "failed", "attempts": 3})
                    ),
                )
                .unwrap();
                Ok(())
            },
            emit_observing: |_: &ObservingSegment| {},
            wait: native_processing_wait,
        },
    )
    .await;
    assert_eq!(
        created(outcome.as_ref().unwrap())
            .processing
            .failed_segments
            .len(),
        1
    );

    finish_audio_attempt(&req.journal_root, &req.import_id, generation, &outcome);
    let projection = project_import_result(&req.journal_root, &req.import_id);
    assert_eq!(projection.status, ProjectionStatus::Failed);
}

/// A stalled segment reads `unconfirmed`, not `failed`.
///
/// Thirty seconds of inactivity is not a verdict -- these segments usually complete later,
/// and calling that a failure would tell an owner their audio was lost when it was not.
/// This deliberately diverges from the CLI, which exits non-zero for a stall.
#[tokio::test]
async fn a_stalled_segment_projects_unconfirmed_rather_than_failed() {
    let temp = TempDir::new().unwrap();
    fs::write(temp.path().join("source.m4a"), b"source").unwrap();
    let mut req = request(&temp, "20260811_230000");
    req.wait_for_processing = true;
    req.stall_timeout = Duration::from_millis(2);
    req.poll_interval = Duration::from_millis(1);
    let generation = admit(&req);

    let outcome = fake_import(req.clone(), 120.0, None, Rc::new(RefCell::new(Vec::new()))).await;
    assert_eq!(
        created(outcome.as_ref().unwrap())
            .processing
            .stalled_segments
            .len(),
        1
    );

    finish_audio_attempt(&req.journal_root, &req.import_id, generation, &outcome);
    let projection = project_import_result(&req.journal_root, &req.import_id);
    assert_eq!(
        projection.status,
        ProjectionStatus::Unconfirmed,
        "a stall is not final, so it must not read as a definitive failure"
    );
}
