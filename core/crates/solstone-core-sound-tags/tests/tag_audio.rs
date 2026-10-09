// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Classification runs out of process. These tests exercise
//! the request/response wiring and readiness dispatch with a stub script
//! standing in for the compiled `solstone-core-ced-analyze` sibling.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::json;
use solstone_core_installed_payload::{
    COMPILED_VERSION, PRODUCT, TARGET_LINUX_X86_64, code, guidance, render_installed_payload,
};
use solstone_core_local::install::capability_status::CapabilityStatus;
use solstone_core_local::install::ced_readiness::{
    CedVerdict, LINUX_CED_LIBRARY, POSIX_CED_MODEL, evaluate_ced_readiness_in_package_with_probe,
    fresh_model_check_in_package,
};
use solstone_core_local::install::ced_runtime::CedAnalyzeProgram;
use solstone_core_sound_tags::{
    CLASSIFY_SAMPLE_RATE, WINDOW_S, tag_audio, tag_audio_in_package,
    tag_audio_with_readiness_and_program,
};

fn enough_audio() -> Vec<f32> {
    vec![0.0; WINDOW_S * CLASSIFY_SAMPLE_RATE as usize]
}

fn one_second() -> Vec<f32> {
    vec![0.0; 16_000]
}

fn write_rendered_manifest(root: &Path, target: &str) {
    let manifest_bytes =
        render_installed_payload(root, PRODUCT, COMPILED_VERSION, target, "commit_aaa").unwrap();
    let manifest_path = root.join(solstone_core_installed_payload::INSTALLED_PAYLOAD_MANIFEST);
    fs::create_dir_all(manifest_path.parent().unwrap()).unwrap();
    fs::write(manifest_path, manifest_bytes).unwrap();
}

fn stub_program(body: &str) -> (tempfile::TempDir, CedAnalyzeProgram) {
    let root = tempfile::tempdir().expect("stub dir");
    let path = root.path().join("solstone-core-ced-analyze");
    fs::write(&path, body).expect("write stub");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("chmod stub");
    }
    (
        root,
        CedAnalyzeProgram::Explicit {
            executable: path,
            args: Vec::new(),
        },
    )
}

#[test]
fn audio_too_short_yields_neither_tags_nor_status() {
    let journal = tempfile::tempdir().expect("temporary journal");
    let (tags, status) = tag_audio(&one_second(), journal.path());
    assert!(tags.is_none());
    assert!(status.is_none());
}

#[test]
fn missing_member_in_package_yields_absent_status() {
    let root = tempfile::tempdir().unwrap();
    let lib_path = root.path().join(LINUX_CED_LIBRARY);
    let model_path = root.path().join(POSIX_CED_MODEL);
    fs::create_dir_all(lib_path.parent().unwrap()).unwrap();
    fs::create_dir_all(model_path.parent().unwrap()).unwrap();
    fs::write(&lib_path, b"lib_bytes").unwrap();
    fs::write(&model_path, b"model_bytes").unwrap();

    write_rendered_manifest(root.path(), TARGET_LINUX_X86_64);

    fs::remove_file(&model_path).unwrap();

    let (_stub_dir, program) = stub_program("#!/bin/sh\nexit 0\n");

    let (tags, status) = tag_audio_in_package(&enough_audio(), root.path(), &program);
    assert!(tags.is_none());
    match status {
        Some(CapabilityStatus::Absent { detail, .. }) => {
            assert!(
                detail.contains(code::MEMBER_MISSING),
                "detail must contain member-missing: {detail}"
            );
            assert!(detail.contains(guidance::PACKAGE_MISMATCH));
        }
        other => panic!("expected Absent status, got {other:?}"),
    }
}

#[test]
fn model_changed_after_admission_yields_integrity_invalid_and_does_not_invoke_worker() {
    let root = tempfile::tempdir().unwrap();
    let lib_path = root.path().join(LINUX_CED_LIBRARY);
    let model_path = root.path().join(POSIX_CED_MODEL);
    fs::create_dir_all(lib_path.parent().unwrap()).unwrap();
    fs::create_dir_all(model_path.parent().unwrap()).unwrap();
    fs::write(&lib_path, b"lib_bytes").unwrap();
    fs::write(&model_path, b"initial_model_bytes").unwrap();

    // 1. Render the package with the model bytes that admission will accept.
    write_rendered_manifest(root.path(), TARGET_LINUX_X86_64);

    // 2. evaluate_ced_readiness_in_package_with_probe(..., |_, _| Ok(())) and assert Ready.
    let readiness =
        evaluate_ced_readiness_in_package_with_probe(root.path(), "linux", "x86_64", |_, _| Ok(()));
    assert!(matches!(readiness, CedVerdict::Ready { .. }));

    // 3. Overwrite the model file.
    fs::write(&model_path, b"altered_model_bytes").unwrap();

    // 4. fresh_model_check_in_package returns IntegrityInvalid whose detail contains code::MEMBER_CHANGED and guidance::PACKAGE_MISMATCH, and does not contain tamper.
    let check = fresh_model_check_in_package(root.path(), "linux", "x86_64");
    match check {
        Err(CapabilityStatus::IntegrityInvalid { detail, .. }) => {
            assert!(
                detail.contains(code::MEMBER_CHANGED),
                "detail must contain member-changed: {detail}"
            );
            assert!(detail.contains(guidance::PACKAGE_MISMATCH));
            assert!(!detail.contains("tamper"));
        }
        other => {
            panic!("expected IntegrityInvalid from fresh_model_check_in_package, got {other:?}")
        }
    }

    // 5. tag_audio_in_package with a stub that touches a marker on any invocation does not create that marker.
    let marker = root.path().join("invoked.marker");
    let script = format!(
        "#!/bin/sh\ntouch '{}'\nprintf '%s\\n' '{{\"schema\":\"solstone-ced-response-v1\",\"windows\":[{{\"ok\":true,\"tags\":{{\"Music\":0.9}}}}]}}'\n",
        marker.display()
    );
    let (_stub_dir, program) = stub_program(&script);

    let (tags, status) = tag_audio_in_package(&enough_audio(), root.path(), &program);
    assert!(tags.is_none());
    match status {
        Some(CapabilityStatus::IntegrityInvalid { detail, .. }) => {
            assert!(detail.contains(code::MEMBER_CHANGED));
            assert!(detail.contains(guidance::PACKAGE_MISMATCH));
            assert!(!detail.contains("tamper"));
        }
        other => panic!("expected IntegrityInvalid status, got {other:?}"),
    }
    assert!(
        !marker.exists(),
        "worker must not be invoked when model is changed"
    );
}

#[test]
fn successful_package_classification_returns_tags_and_no_status() {
    let root = tempfile::tempdir().unwrap();
    let lib_path = root.path().join(LINUX_CED_LIBRARY);
    let model_path = root.path().join(POSIX_CED_MODEL);
    fs::create_dir_all(lib_path.parent().unwrap()).unwrap();
    fs::create_dir_all(model_path.parent().unwrap()).unwrap();
    fs::write(&lib_path, b"lib_bytes").unwrap();
    fs::write(&model_path, b"model_bytes").unwrap();

    write_rendered_manifest(root.path(), TARGET_LINUX_X86_64);

    let (_stub_dir, program) = stub_program(
        "#!/bin/sh\ninput=$(cat)\ncase \"$input\" in\n  *solstone-ced-probe-request-v1*)\n    printf '%s\\n' '{\"schema\":\"solstone-ced-probe-response-v1\",\"ok\":true}'\n    ;;\n  *)\n    printf '%s\\n' '{\"schema\":\"solstone-ced-response-v1\",\"windows\":[{\"ok\":true,\"tags\":{\"Music\":0.9,\"Above\":0.11}}]}'\n    ;;\nesac\n",
    );

    let (tags, status) = tag_audio_in_package(&enough_audio(), root.path(), &program);
    assert!(status.is_none());
    assert_eq!(
        tags.unwrap(),
        json!({
            "engine": "ced.cpp v0.1.0",
            "model": "ced-tiny-q8_0",
            "threshold": 0.1,
            "window_s": 10,
            "agg": "max",
            "windows": 1,
            "tags": {"Music": 0.9, "Above": 0.11},
        })
    );
}

#[test]
fn unloadable_ready_verdict_degrades_tag_audio_to_none() {
    let readiness = CedVerdict::Degraded(CapabilityStatus::UnloadableOrUnrunnable {
        capability: "ced".to_owned(),
        detail: "stub: engine refused to load".to_owned(),
    });
    let (tags, status) = tag_audio_with_readiness_and_program(
        &enough_audio(),
        readiness,
        &CedAnalyzeProgram::Explicit {
            executable: PathBuf::from("/should/never/run"),
            args: Vec::new(),
        },
    );
    assert!(tags.is_none());
    assert!(matches!(
        status,
        Some(CapabilityStatus::UnloadableOrUnrunnable { .. })
    ));
}

#[test]
fn successful_tags_match_the_stub_contract() {
    let (_root, program) = stub_program(
        "#!/bin/sh\ncat >/dev/null\nprintf '%s\\n' '{\"schema\":\"solstone-ced-response-v1\",\"windows\":[{\"ok\":true,\"tags\":{\"Music\":0.9,\"Above\":0.11}}]}'\n",
    );
    let readiness = CedVerdict::Ready {
        library: PathBuf::from("/fake/libced.so"),
        model: PathBuf::from("/fake/model.gguf"),
    };
    let (tags, status) = tag_audio_with_readiness_and_program(&enough_audio(), readiness, &program);
    assert!(status.is_none());
    assert_eq!(
        tags.expect("stub tags"),
        json!({
            "engine": "ced.cpp v0.1.0",
            "model": "ced-tiny-q8_0",
            "threshold": 0.1,
            "window_s": 10,
            "agg": "max",
            "windows": 1,
            "tags": {"Music": 0.9, "Above": 0.11},
        })
    );
}

#[test]
fn a_failed_window_keeps_successful_windows() {
    let (_root, program) = stub_program(
        "#!/bin/sh\ncat >/dev/null\nprintf '%s\\n' '{\"schema\":\"solstone-ced-response-v1\",\"windows\":[{\"ok\":false,\"reason\":\"classify-failed\",\"detail\":\"stub failure\"},{\"ok\":true,\"tags\":{\"Music\":0.9}}]}'\n",
    );
    let readiness = CedVerdict::Ready {
        library: PathBuf::from("/fake/libced.so"),
        model: PathBuf::from("/fake/model.gguf"),
    };
    let mut audio = vec![0.0; 160_000];
    audio.extend(vec![0.0; 160_000]);

    let (tags, status) = tag_audio_with_readiness_and_program(&audio, readiness, &program);
    assert!(status.is_none());
    let tags = tags.expect("one successful window");
    assert_eq!(tags["windows"], 1);
    assert_eq!(tags["tags"]["Music"], 0.9);
}

#[test]
fn helper_process_failure_degrades_to_none() {
    let (_root, program) = stub_program(
        "#!/bin/sh\ncat >/dev/null\nprintf '%s\\n' '{\"schema\":\"solstone-ced-error-v1\",\"reason\":\"library-unloadable\",\"detail\":\"boom\"}' >&2\nexit 69\n",
    );
    let readiness = CedVerdict::Ready {
        library: PathBuf::from("/fake/libced.so"),
        model: PathBuf::from("/fake/model.gguf"),
    };
    let (tags, status) = tag_audio_with_readiness_and_program(&enough_audio(), readiness, &program);
    assert!(tags.is_none());
    assert!(status.is_none());
}

#[test]
fn malformed_helper_response_degrades_to_none() {
    let (_root, program) = stub_program("#!/bin/sh\ncat >/dev/null\nprintf 'not json'\n");
    let readiness = CedVerdict::Ready {
        library: PathBuf::from("/fake/libced.so"),
        model: PathBuf::from("/fake/model.gguf"),
    };
    let (tags, status) = tag_audio_with_readiness_and_program(&enough_audio(), readiness, &program);
    assert!(tags.is_none());
    assert!(status.is_none());
}

#[test]
fn window_count_mismatch_degrades_to_none() {
    let (_root, program) = stub_program(
        "#!/bin/sh\ncat >/dev/null\nprintf '%s\\n' '{\"schema\":\"solstone-ced-response-v1\",\"windows\":[{\"ok\":true,\"tags\":{}},{\"ok\":true,\"tags\":{}}]}'\n",
    );
    let readiness = CedVerdict::Ready {
        library: PathBuf::from("/fake/libced.so"),
        model: PathBuf::from("/fake/model.gguf"),
    };
    let (tags, status) = tag_audio_with_readiness_and_program(&enough_audio(), readiness, &program);
    assert!(tags.is_none());
    assert!(status.is_none());
}
