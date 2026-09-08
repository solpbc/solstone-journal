// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Operator-selected proof against a complete, isolated, test-signed payload.
#![cfg(windows)]

use std::io::Cursor;
use std::path::PathBuf;

use serde_json::Value;
use sha2::{Digest, Sha256};
use solstone_core_depict::{Detector, SystemDetector};
use solstone_core_distribution::manifest_verify::install_test_fixture_pin;
use solstone_core_distribution::windows_payload::{
    WINDOWS_RFDETR_MODEL, WINDOWS_RFDETR_WORKER, verify_windows_payload,
};

fn input_path(name: &str) -> PathBuf {
    let path = PathBuf::from(std::env::var_os(name).unwrap_or_else(|| panic!("missing {name}")));
    assert!(path.is_absolute(), "{name} must be absolute");
    path
}

fn assert_person(result: &Value) {
    assert_eq!(result["image"]["width"], 480);
    assert_eq!(result["image"]["height"], 320);
    assert!(
        result["detections"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["class_name"] == "person")
    );
}

#[test]
#[ignore = "requires an isolated complete test-signed payload and exact RF fixture"]
fn installed_rfdetr_production_paths_refuse_tamper_and_ignore_overrides() {
    install_test_fixture_pin(&input_path("SOLSTONE_WINDOWS_TEST_PIN")).unwrap();
    let executable = std::env::current_exe().unwrap();
    let bin = executable.parent().unwrap();
    assert_eq!(bin.file_name().unwrap(), "bin");
    let root = bin.parent().unwrap();
    let payload = verify_windows_payload(root).unwrap();
    let marker = payload
        .declared_path("share/windows-native-test-only")
        .expect("isolated fixture marker");
    assert_eq!(
        std::fs::read(marker).unwrap(),
        b"windows-rfdetr-consumer-fixture-v1"
    );
    let poison = input_path("SOLSTONE_WINDOWS_RF_POISON");
    assert_eq!(
        std::fs::read(&poison).unwrap(),
        b"not an executable or model"
    );
    assert_eq!(
        std::env::var_os("SOLSTONE_DESCRIBE_DETECT_BINARY").unwrap(),
        poison.as_os_str()
    );
    assert_eq!(
        std::env::var("SOLSTONE_DESCRIBE_DETECT_TIMEOUT_MS").unwrap(),
        "1"
    );
    assert_eq!(
        std::env::var_os("PATH").unwrap(),
        poison.parent().unwrap().as_os_str()
    );
    let bytes = std::fs::read(input_path("SOLSTONE_WINDOWS_RF_INPUT")).unwrap();
    assert_eq!(
        format!("{:x}", Sha256::digest(&bytes)),
        "ff3a2092bf7a3a572903482e4d73a25576f0f1c13dd53f2f6b7bb3c20fb0119d"
    );
    let mut png = Cursor::new(Vec::new());
    image::load_from_memory(&bytes)
        .unwrap()
        .write_to(&mut png, image::ImageFormat::Png)
        .unwrap();
    let journal = tempfile::tempdir().unwrap();
    let describe = solstone_core_describe::detect::detect(png.get_ref(), journal.path()).unwrap();
    let depict = SystemDetector
        .detect(png.get_ref())
        .unwrap()
        .expect("required detector result");
    assert_person(&describe);
    assert_person(&depict);
    println!(
        "{}",
        serde_json::json!({"describe": describe, "depict": depict})
    );

    // Each failure uses the actual production resolver/launch call chain. Restore
    // fixture bytes before asserting so a failed assertion leaves a usable tree.
    let model = root.join(WINDOWS_RFDETR_MODEL);
    let original = std::fs::read(&model).unwrap();
    std::fs::write(&model, b"poisoned model").unwrap();
    let describe_bad = solstone_core_describe::detect::detect(png.get_ref(), journal.path());
    let depict_bad = SystemDetector.detect(png.get_ref());
    std::fs::write(&model, original).unwrap();
    assert!(describe_bad.is_err());
    assert!(depict_bad.is_err());
    verify_windows_payload(root).unwrap();

    let worker = root.join(WINDOWS_RFDETR_WORKER);
    let held = tempfile::tempdir_in(root.parent().unwrap()).unwrap();
    let held_worker = held.path().join("rfdetr-cli.exe");
    std::fs::rename(&worker, &held_worker).unwrap();
    let describe_missing = solstone_core_describe::detect::detect(png.get_ref(), journal.path());
    let depict_missing = SystemDetector.detect(png.get_ref());
    std::fs::rename(&held_worker, &worker).unwrap();
    assert!(describe_missing.is_err());
    assert!(depict_missing.is_err());
    verify_windows_payload(root).unwrap();
    assert_eq!(std::fs::read_dir(journal.path()).unwrap().count(), 0);
}
