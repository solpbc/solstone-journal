// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Direct staged libtest subjects. Each selector runs in a fresh process inside
//! bin of an isolated test-signed payload; the outer driver retains all fixtures
//! and its fence if helper cleanup is pending or the subject fails.

use super::*;
use crate::speakers_installation::{SpeakersAnalyzeOwnerRole, enter_speakers_analyze_generation};
use sha2::{Digest, Sha256};
use solstone_core_distribution::manifest_verify::install_test_fixture_pin;
use solstone_core_distribution::windows_payload::{
    WINDOWS_VAD_ANALYZE_WORKER, WindowsPayloadRefusal, verify_windows_payload,
};
use std::path::PathBuf;
use std::time::Duration;

fn absolute_input(name: &str) -> PathBuf {
    let path = PathBuf::from(std::env::var_os(name).unwrap_or_else(|| panic!("missing {name}")));
    assert!(path.is_absolute(), "{name} must be absolute");
    path
}

fn fixture_root() -> PathBuf {
    let executable = std::env::current_exe().unwrap();
    let bin = executable.parent().unwrap();
    assert_eq!(bin.file_name().unwrap(), "bin");
    bin.parent().unwrap().to_path_buf()
}

fn assert_generation_contended(journal: &Path) {
    let result =
        enter_speakers_analyze_generation(journal, SpeakersAnalyzeOwnerRole::Maintenance, None);
    assert!(result.is_err_and(|error| {
        error
            .message()
            .is_some_and(|message| message.starts_with("generation-lease-contended:"))
    }));
}

fn install_parent_pin() -> (PathBuf, Vec<u8>) {
    let pin = absolute_input("SOLSTONE_WINDOWS_TEST_PIN");
    let bytes = std::fs::read(&pin).unwrap();
    install_test_fixture_pin(&pin).unwrap();
    (pin, bytes)
}

#[test]
#[ignore = "requires real installed ONNX helpers, signed models and the committed speech seed"]
fn installed_onnx_callers_infer_with_real_generation() {
    let (pin, original_pin) = install_parent_pin();
    assert_eq!(absolute_input("SOLSTONE_JOURNAL_MINISIGN_PIN"), pin);
    let root = fixture_root();
    let payload = verify_windows_payload(&root).unwrap();
    let marker = payload
        .declared_path("share/windows-native-test-only")
        .unwrap();
    assert_eq!(
        std::fs::read(marker).unwrap(),
        b"windows-onnx-consumer-fixture-v1"
    );
    let poison = absolute_input("SOLSTONE_WINDOWS_ONNX_POISON");
    assert_eq!(std::fs::read(&poison).unwrap(), b"not a helper or runtime");
    for name in [
        "SOLSTONE_VAD_BINARY",
        "SOLSTONE_SPEAKERS_ANALYZE_BINARY",
        "ORT_DYLIB_PATH",
    ] {
        assert_eq!(
            absolute_input(name),
            poison,
            "outer override control missing"
        );
    }
    assert_eq!(
        std::env::var_os("PATH").unwrap(),
        poison.parent().unwrap().as_os_str()
    );
    let seed = std::fs::read(absolute_input("SOLSTONE_WINDOWS_ONNX_SPEECH_SEED")).unwrap();
    assert_eq!(seed.len(), 65_536);
    assert_eq!(
        format!("{:x}", Sha256::digest(&seed)),
        "93f09ab4b54cc294b89ae2815c2a66be9d746e849d138725841cbccce8bb24e1"
    );
    let audio: Vec<f32> = seed
        .chunks_exact(4)
        .map(|bytes| f32::from_le_bytes(bytes.try_into().unwrap()))
        .collect::<Vec<_>>()
        .repeat(12);
    assert!(audio.iter().all(|value| value.is_finite()));

    // Retain the journal on disk even if an assertion fails while the facade
    // owns pending cleanup. The outer receipt, not TempDir::drop, removes it.
    let journal = tempfile::Builder::new()
        .prefix("solstone-onnx-native-journal-")
        .tempdir()
        .unwrap()
        .keep();
    println!("ONNX_NATIVE_JOURNAL={}", journal.display());
    let generation =
        enter_speakers_analyze_generation(&journal, SpeakersAnalyzeOwnerRole::Transcribe, None)
            .unwrap();
    let context = generation.child_launch_context();
    assert_generation_contended(&journal);
    let vad = crate::audio::run_vad(&audio, 0.5, &context).unwrap();
    assert!(vad.has_speech && vad.speech_duration_s > 0.5);
    assert!(!vad.speech_segments.is_empty());
    let duration = audio.len() as f64 / 16_000.0;
    assert!((vad.duration_s - duration).abs() < 0.001);
    let statements = vec![
        serde_json::json!({"id": 1, "start": 0.0, "end": 4.0,
        "text": "Synthetic native fixture"})
        .as_object()
        .unwrap()
        .clone(),
    ];
    let speakers = crate::speakers::analyze_speakers(
        &journal.join("fixture.wav"),
        &audio,
        &audio,
        None,
        &statements,
        &statements,
        16_000,
        0.5,
        &context,
    )
    .unwrap();
    let embeddings = speakers
        .embedding_payload
        .as_ref()
        .expect("real statement embedding");
    assert_eq!(embeddings.statement_ids, [1]);
    assert_eq!(embeddings.encoder, "wespeaker-resnet34-256");
    assert_eq!(embeddings.payload.len(), 256 * 4);
    let values: Vec<f32> = embeddings
        .payload
        .chunks_exact(4)
        .map(|bytes| f32::from_le_bytes(bytes.try_into().unwrap()))
        .collect();
    assert!(values.iter().all(|value| value.is_finite()));
    assert!(values.iter().any(|value| value.abs() > 0.000_001));
    assert_generation_contended(&journal);
    drop(context);
    drop(generation);
    let reacquired =
        enter_speakers_analyze_generation(&journal, SpeakersAnalyzeOwnerRole::Maintenance, None)
            .unwrap();
    drop(reacquired);
    verify_windows_payload(&root).unwrap();
    assert_eq!(std::fs::read(pin).unwrap(), original_pin);
    println!(
        "{}",
        serde_json::json!({"speech_duration_s": vad.speech_duration_s,
        "speech_segments": vad.speech_segments, "embedding_sha256": format!("{:x}",
        Sha256::digest(&embeddings.payload)), "generation_reacquired": true})
    );
}

#[test]
#[ignore = "one fresh process per missing, empty, relative or wrong-env-over-parent-pin case"]
fn installed_onnx_pin_refusal_precedes_helper_launch() {
    let (pin, original_pin) = install_parent_pin();
    let mode = std::env::var("SOLSTONE_WINDOWS_ONNX_PIN_CASE").unwrap();
    let selected = std::env::var_os("SOLSTONE_JOURNAL_MINISIGN_PIN");
    let expected_message = match mode.as_str() {
        "missing" => {
            assert!(selected.is_none());
            verify_windows_payload(&fixture_root()).unwrap();
            "test-signed ONNX launch requires an explicit UTF-8 fixture pin path".to_owned()
        }
        "empty" => {
            assert!(selected.as_ref().is_some_and(|value| value.is_empty()));
            verify_windows_payload(&fixture_root()).unwrap();
            "test-signed ONNX launch requires an absolute fixture pin path".to_owned()
        }
        "relative" => {
            assert!(
                selected
                    .as_ref()
                    .is_some_and(|value| !value.is_empty() && !Path::new(value).is_absolute())
            );
            "test-signed ONNX launch requires an absolute fixture pin path".to_owned()
        }
        "wrong-env-over-parent-pin" => {
            let wrong = absolute_input("SOLSTONE_JOURNAL_MINISIGN_PIN");
            assert_ne!(wrong, pin);
            assert_ne!(std::fs::read(wrong).unwrap(), original_pin);
            let refusal = verify_windows_payload(&fixture_root()).unwrap_err();
            assert_eq!(refusal.kind, WindowsPayloadRefusal::Signature);
            format!("could not verify the signed ONNX app payload: {refusal}")
        }
        _ => panic!("unknown exact pin control"),
    };
    // No generation is fabricated here: every case must return typed admission
    // failure before creating a helper. A launch or malformed-request result
    // makes this test fail, even if that child subsequently exits nonzero.
    let result = run_onnx_helper(
        OnnxHelper::Vad,
        &fixture_root().join(WINDOWS_VAD_ANALYZE_WORKER),
        b"",
        BoundedHelperBudget {
            timeout: Duration::from_secs(10),
            stdin_limit_bytes: 1,
            stdout_limit_bytes: 4096,
            stderr_limit_bytes: 4096,
        },
        BoundedHelperResources::new(),
    );
    match result {
        Err(OnnxHelperError::Admission(detail)) => assert_eq!(detail, expected_message),
        _ => panic!("exact pin refusal must precede helper launch"),
    }
    assert_eq!(std::fs::read(pin).unwrap(), original_pin);
}
