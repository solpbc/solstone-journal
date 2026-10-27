// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use super::*;

#[path = "../tests/support/mod.rs"]
mod reference;

fn request(audio_path: &str, model_path: &str) -> Value {
    json!({
        "schema": REQUEST_SCHEMA,
        "audio_f32le_path": audio_path,
        "models": {"firered_vad_onnx_path": model_path},
        "min_speech_seconds": 1.0,
    })
}

#[cfg(not(feature = "full-tests"))]
mod routine {
    use super::*;

    #[test]
    fn request_validates_schema_model_and_options_without_aliases() {
        let valid = request("audio.f32", "model.onnx");
        let parsed = parse_request(&valid.to_string()).expect("request");
        assert_eq!(parsed.options, VadOptions::default());
        assert_eq!(parsed.firered_vad_onnx_path, "model.onnx");
        for schema in ["solstone-vad-request-v1", "invalid-schema"] {
            let mut value = valid.clone();
            value["schema"] = json!(schema);
            let error = parse_request(&value.to_string()).unwrap_err();
            assert_eq!(error.reason(), "unknown-schema");
            assert_eq!(error.exit_code(), 64);
        }
        for value in [
            json!([]),
            {
                let mut value = valid.clone();
                value["models"] = json!({});
                value
            },
            {
                let mut value = valid.clone();
                value["audio_f32le_path"] = json!("");
                value
            },
            {
                let mut value = valid.clone();
                value["min_speech_seconds"] = json!(-1);
                value
            },
            {
                let mut value = valid.clone();
                value["options"] = json!({"unexpected": 1});
                value
            },
            {
                let mut value = valid.clone();
                value["options"] = json!({"speech_pad_ms": -1});
                value
            },
        ] {
            assert_eq!(
                parse_request(&value.to_string()).unwrap_err().reason(),
                "malformed-request"
            );
        }
        assert_eq!(
            parse_request("not json").unwrap_err().reason(),
            "malformed-request"
        );
        let mut override_request = valid;
        override_request["options"] = json!({"threshold": 0.5, "speech_pad_ms": 0});
        let options = parse_request(&override_request.to_string())
            .expect("request")
            .options;
        assert_eq!(options.threshold, 0.5);
        assert_eq!(options.speech_pad_ms, 0);
        assert_eq!(options.min_silence_duration_ms, 1000);
    }

    #[test]
    fn frontend_matches_independent_canonical_features() {
        let metadata = reference::reference();
        let mut maximum = 0.0_f32;
        let mut ordinary_cases = 0;
        for case in metadata["cases"].as_array().expect("cases") {
            if case["long"].as_bool().expect("long") {
                continue;
            }
            let audio = reference::audio(case);
            let actual = features::prepare_features(&audio).expect("features");
            assert_eq!(
                actual.frames(),
                case["frames"].as_u64().expect("frames") as usize
            );
            let expected = reference::floats(&case["features"]);
            let delta = reference::max_abs_diff(actual.data(), &expected);
            reference::check_feature_delta(case, delta);
            if !reference::is_spectral_null_control(case) {
                ordinary_cases += 1;
                maximum = maximum.max(delta);
            }
        }
        assert!(
            ordinary_cases > 0,
            "no ordinary feature controls were exercised"
        );
        eprintln!("canonical feature maximum absolute difference: {maximum}");
    }

    #[test]
    fn feature_blocks_own_every_frame_once_with_full_context() {
        for frames in [0, 1, 29999, 30000, 30001, 60163] {
            let mut next = 0;
            for block in feature_blocks(frames) {
                assert_eq!(block.owned.start, next);
                assert!(block.owned.len() <= 30000);
                assert_eq!(block.context.start, block.owned.start.saturating_sub(152));
                assert_eq!(block.context.end, (block.owned.end + 160).min(frames));
                next = block.owned.end;
            }
            assert_eq!(next, frames);
        }
    }

    #[test]
    fn returned_probabilities_reject_missing_extra_nonfinite_or_nonposterior_values() {
        assert!(validate_probabilities(&[0.0, 0.5, 1.0], 3).is_ok());
        assert!(validate_probabilities(&[], 1).is_err());
        assert!(validate_probabilities(&[0.5; 2], 1).is_err());
        for probability in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -0.01, 1.01] {
            assert!(validate_probabilities(&[probability], 1).is_err());
        }
    }

    fn bare_options() -> VadOptions {
        VadOptions {
            speech_pad_ms: 0,
            ..VadOptions::default()
        }
    }

    #[test]
    fn threshold_equality_starts_and_hysteresis_preserves_an_open_segment() {
        let options = VadOptions {
            threshold: 0.5,
            ..bare_options()
        };
        assert_eq!(
            find_speech_timestamps(&[0.0, 0.5, 0.4, 0.4], 640, &options),
            vec![SpeechChunk {
                start: 160,
                end: 640
            }]
        );
    }

    #[test]
    fn silence_hold_splits_only_after_the_full_duration() {
        for (gap, expected) in [
            (
                100,
                vec![SpeechChunk {
                    start: 0,
                    end: 120 * 160,
                }],
            ),
            (
                101,
                vec![
                    SpeechChunk {
                        start: 0,
                        end: 10 * 160,
                    },
                    SpeechChunk {
                        start: 111 * 160,
                        end: 121 * 160,
                    },
                ],
            ),
        ] {
            let probs = [vec![0.9; 10], vec![0.0; gap], vec![0.9; 10]].concat();
            assert_eq!(
                find_speech_timestamps(&probs, probs.len() * 160, &bare_options()),
                expected
            );
        }
    }

    #[test]
    fn overlapping_padding_shares_the_gap_without_double_counting_duration() {
        let probs = [vec![0.9; 10], vec![0.0; 10], vec![0.9; 10], vec![0.0]].concat();
        let options = VadOptions {
            min_silence_duration_ms: 0,
            ..VadOptions::default()
        };
        let speech = find_speech_timestamps(&probs, probs.len() * 160, &options);
        assert_eq!(
            speech,
            vec![
                SpeechChunk {
                    start: 0,
                    end: 2400
                },
                SpeechChunk {
                    start: 2400,
                    end: 4960
                }
            ]
        );
        let response = response_value(
            &parse_request(&request("audio.f32", "model.onnx").to_string()).expect("request"),
            4960,
            &speech,
        );
        assert_eq!(response["speech_duration"], json!(4960.0 / 16000.0));
    }

    #[test]
    fn padding_and_duration_options_keep_ranges_sorted_disjoint_and_clamped() {
        let probs = (0..220)
            .map(|index| if index % 37 < 9 { 0.9 } else { 0.0 })
            .collect::<Vec<_>>();
        let samples = probs.len() * 160 + 139;
        for silence in [0, 10, 100, 1000] {
            for padding in [0, 1, 10, 400, 5000] {
                for minimum in [0, 10, 100] {
                    let options = VadOptions {
                        min_silence_duration_ms: silence,
                        speech_pad_ms: padding,
                        min_speech_duration_ms: minimum,
                        ..VadOptions::default()
                    };
                    let speech = find_speech_timestamps(&probs, samples, &options);
                    assert!(
                        speech
                            .iter()
                            .all(|span| span.start < span.end && span.end <= samples)
                    );
                    assert!(speech.windows(2).all(|pair| pair[0].end <= pair[1].start));
                    assert!(
                        speech
                            .iter()
                            .map(|span| span.end - span.start)
                            .sum::<usize>()
                            <= samples
                    );
                }
            }
        }
    }

    #[test]
    fn minimum_speech_and_eof_use_original_samples() {
        let options = VadOptions {
            min_speech_duration_ms: 100,
            ..bare_options()
        };
        assert!(find_speech_timestamps(&[0.9; 10], 1600, &options).is_empty());
        assert_eq!(
            find_speech_timestamps(&[0.9; 10], 1601, &options),
            vec![SpeechChunk {
                start: 0,
                end: 1601
            }]
        );
        assert!(find_speech_timestamps(&[], 399, &VadOptions::default()).is_empty());
        assert!(find_speech_timestamps(&[0.0; 20], 3400, &VadOptions::default()).is_empty());
        assert_eq!(
            find_speech_timestamps(&[0.9; 10], 1759, &VadOptions::default()),
            vec![SpeechChunk {
                start: 0,
                end: 1759
            }]
        );
    }

    #[test]
    fn response_and_error_envelopes_preserve_the_detector_neutral_contract() {
        let parsed =
            parse_request(&request("audio.f32", "model.onnx").to_string()).expect("request");
        let response = response_value(
            &parsed,
            48000,
            &[SpeechChunk {
                start: 0,
                end: 16000,
            }],
        );
        assert_eq!(response["schema"], json!(RESPONSE_SCHEMA));
        assert_eq!(response["duration"], json!(3.0));
        assert_eq!(response["speech_duration"], json!(1.0));
        assert_eq!(response["has_speech"], json!(true));
        assert_eq!(
            response_value(
                &parsed,
                48000,
                &[SpeechChunk {
                    start: 0,
                    end: 15999
                }]
            )["has_speech"],
            json!(false)
        );
        let error: Value = serde_json::from_str(&error_line_for_vad_error(&VadError::Internal {
            detail: "cause".into(),
        }))
        .expect("error");
        assert_eq!(error["schema"], json!(ERROR_SCHEMA));
        assert_eq!(error["reason"], json!("internal-error"));
        assert_eq!(error["detail"], json!("cause"));
        assert!(evaluate_args(&[]).is_ok());
        assert!(evaluate_args(&[OsString::from("--unexpected")]).is_err());
    }

    #[test]
    fn dotted_release_version_rejects_soname_suffix_and_junk() {
        assert!(is_dotted_release_version("1.25.0"));
        for invalid in ["1", "1.25", "1.25.0.1", "1.25.0-rc1", "1..0"] {
            assert!(!is_dotted_release_version(invalid));
        }
    }
}

#[cfg(feature = "full-tests")]
mod full {
    use super::*;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn model() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../models/assets/firered_vad.onnx")
    }

    struct TempDir(PathBuf);
    impl TempDir {
        fn new() -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos();
            let path = std::env::temp_dir()
                .join(format!("solstone-vad-test-{}-{nonce}", std::process::id()));
            fs::create_dir(&path).expect("test directory");
            Self(path)
        }
        fn path(&self, name: &str) -> String {
            self.0.join(name).to_string_lossy().into_owned()
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn filesystem_boundaries_preserve_specific_input_errors() {
        let temp = TempDir::new();
        assert_eq!(
            read_audio_f32le(&temp.path("missing"))
                .unwrap_err()
                .reason(),
            "audio-unreadable"
        );
        assert_eq!(
            read_audio_f32le(&temp.0.to_string_lossy())
                .unwrap_err()
                .reason(),
            "audio-invalid"
        );
        for (name, bytes, reason) in [
            ("empty", Vec::new(), "audio-invalid"),
            ("partial", vec![1, 2, 3], "audio-invalid"),
            ("nan", f32::NAN.to_le_bytes().to_vec(), "audio-non-finite"),
        ] {
            let path = temp.path(name);
            fs::write(&path, bytes).expect("fixture");
            assert_eq!(read_audio_f32le(&path).unwrap_err().reason(), reason);
        }
    }

    #[test]
    fn missing_and_corrupt_graphs_fail_before_inference() {
        let temp = TempDir::new();
        let audio = temp.path("audio.f32");
        fs::write(&audio, vec![0; 16000 * 4]).expect("audio");
        assert_eq!(
            run_request(&request(&audio, &temp.path("missing.onnx")).to_string())
                .unwrap_err()
                .reason(),
            "model-unreadable"
        );
        let mut corrupted = fs::read(model()).expect("graph");
        corrupted[0] ^= 1;
        let path = temp.path("corrupt.onnx");
        fs::write(&path, corrupted).expect("graph fixture");
        let error = run_request(&request(&audio, &path).to_string()).unwrap_err();
        assert_eq!(error.reason(), "model-identity-mismatch");
        assert_eq!(error.exit_code(), 64);
    }

    #[test]
    fn actual_graph_io_and_wrong_metadata_are_checked() {
        let session = FireRedVadSession::open(&model()).expect("session");
        check_session_io(&session.session).expect("graph I/O");
        let expected = [ExpectedDim::Any, ExpectedDim::Any, ExpectedDim::Exact(80)];
        let original = session.session.inputs()[0].dtype().clone();
        assert!(check_tensor("input", "other", &original, INPUT_FEATURES_NAME, &expected).is_err());
        for change in 0..3 {
            let mut invalid = original.clone();
            let ValueType::Tensor { ty, shape, .. } = &mut invalid else {
                panic!("tensor");
            };
            match change {
                0 => *ty = TensorElementType::Int64,
                1 => *shape = vec![80_i64].into(),
                _ => *shape = vec![-1_i64, -1, 81].into(),
            }
            assert!(
                check_tensor(
                    "input",
                    INPUT_FEATURES_NAME,
                    &invalid,
                    INPUT_FEATURES_NAME,
                    &expected
                )
                .is_err()
            );
        }
    }

    #[test]
    fn short_audio_has_no_model_frames_and_repeated_requests_are_independent() {
        assert!(
            speech_probabilities(&vec![0.0; 399], &model())
                .expect("short input")
                .is_empty()
        );
        let metadata = reference::reference();
        let case = metadata["cases"]
            .as_array()
            .expect("cases")
            .iter()
            .find(|case| case["name"] == "speech_seed")
            .expect("speech fixture");
        let audio = reference::audio(case);
        let first = speech_probabilities(&audio, &model()).expect("first");
        speech_probabilities(&vec![0.0; 16000], &model()).expect("silence");
        let second = speech_probabilities(&audio, &model()).expect("second");
        assert_eq!(first, second);
    }

    #[test]
    fn bounded_native_inference_matches_independent_whole_graph_reference() {
        let metadata = reference::reference();
        let mut max_features = 0.0_f32;
        let mut max_probabilities = 0.0_f32;
        let mut context_control_difference = 0.0_f32;
        for case in metadata["cases"].as_array().expect("cases") {
            let audio = reference::audio(case);
            let features = features::prepare_features(&audio).expect("features");
            let expected = reference::floats(&case["features"]);
            let selected = case["feature_rows"]
                .as_array()
                .expect("rows")
                .iter()
                .flat_map(|row| {
                    features
                        .row(row.as_u64().expect("row") as usize)
                        .expect("feature row")
                        .iter()
                        .copied()
                })
                .collect::<Vec<_>>();
            let feature_delta = reference::max_abs_diff(&selected, &expected);
            reference::check_feature_delta(case, feature_delta);
            if !reference::is_spectral_null_control(case) {
                max_features = max_features.max(feature_delta);
            }
            let expected = reference::floats(&case["probabilities"]);
            let actual = speech_probabilities(&audio, &model()).expect("probabilities");
            let delta = reference::max_abs_diff(&actual, &expected);
            assert!(delta <= 1e-4, "{}: posterior delta {delta}", case["name"]);
            if reference::is_spectral_null_control(case) {
                eprintln!(
                    "{}: native posteriors {actual:?}; reference {expected:?}; max delta {delta}",
                    case["name"]
                );
            }
            max_probabilities = max_probabilities.max(delta);
            // The reference spans come from tests/generate_reference.py's policy, which fixes
            // threshold 0.3; parity is checked at that policy, not at the product default.
            let reference_policy = VadOptions {
                threshold: 0.3,
                ..VadOptions::default()
            };
            let speech = find_speech_timestamps(&actual, audio.len(), &reference_policy);
            assert_eq!(
                speech
                    .iter()
                    .map(|span| (span.start, span.end))
                    .collect::<Vec<_>>(),
                reference::spans(case),
                "{}: spans",
                case["name"]
            );
            if case["frames"].as_u64().expect("frames") > OWNED_FRAMES_PER_BLOCK as u64 {
                let mut session = FireRedVadSession::open(&model()).expect("session");
                let mut independent = Vec::new();
                for first in (0..features.frames()).step_by(OWNED_FRAMES_PER_BLOCK) {
                    let end = (first + OWNED_FRAMES_PER_BLOCK).min(features.frames());
                    independent.extend(
                        session
                            .infer(&features.data()[first * 80..end * 80], end - first)
                            .expect("independent block"),
                    );
                }
                context_control_difference =
                    context_control_difference.max(reference::max_abs_diff(&actual, &independent));
            }
        }
        assert!(
            context_control_difference > 1e-3,
            "no-context control did not expose the seam"
        );
        eprintln!(
            "canonical feature max={max_features}; whole-graph posterior max={max_probabilities}; no-context control max={context_control_difference}"
        );
    }

    #[cfg(any(target_os = "linux", windows))]
    #[test]
    fn loaded_runtime_version_comes_from_the_running_native_library() {
        FireRedVadSession::open(&model()).expect("session");
        assert!(is_dotted_release_version(
            &loaded_onnx_runtime_version().expect("runtime version")
        ));
    }
}
