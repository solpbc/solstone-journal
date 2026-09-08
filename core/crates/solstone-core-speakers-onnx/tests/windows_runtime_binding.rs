// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(windows)]

use std::path::PathBuf;

use solstone_core_distribution::manifest_verify::install_test_fixture_pin;
use solstone_core_distribution::windows_payload::verify_windows_payload;
use solstone_core_speakers_onnx::windows_runtime::bootstrap_windows_onnx;

#[test]
#[ignore = "requires an isolated signed payload and a second native ORT module"]
fn preinitialized_other_runtime_is_refused_before_inference() {
    let pin = PathBuf::from(std::env::var_os("SOLSTONE_WINDOWS_TEST_PIN").expect("fixture pin"));
    install_test_fixture_pin(&pin).unwrap();
    let executable = std::env::current_exe().unwrap();
    let root = executable.parent().unwrap().parent().unwrap();
    let payload = verify_windows_payload(root).unwrap();
    let marker = payload
        .declared_path("share/windows-native-test-only")
        .expect("isolated fixture marker");
    assert_eq!(
        std::fs::read(marker).unwrap(),
        b"windows-onnx-consumer-fixture-v1"
    );
    let other = PathBuf::from(std::env::var_os("ORT_DYLIB_PATH").expect("alternate ORT module"));
    assert!(other.is_absolute() && other.is_file());
    assert!(
        !other
            .canonicalize()
            .unwrap()
            .starts_with(root.canonicalize().unwrap())
    );
    let admitted = payload.onnxruntime_library_path().unwrap();
    assert_eq!(
        std::fs::read(&other).unwrap(),
        std::fs::read(admitted).unwrap(),
        "alternate module must be an exact copy of the admitted runtime"
    );
    // Deliberately initialize only the API. No environment is committed, so
    // the historical commit()-only guard would have accepted this binding.
    let before = std::ptr::from_ref(ort::api());
    let error = bootstrap_windows_onnx().unwrap_err();
    assert!(
        error.contains("API is bound to a different library"),
        "{error}"
    );
    assert_eq!(before, std::ptr::from_ref(ort::api()));
    println!("preinitialized other runtime refused before inference: {error}");
}
