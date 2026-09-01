// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Classification runs out of process now (Brief D). These tests exercise
//! the request/response wiring and readiness dispatch with a stub script
//! standing in for the compiled `solstone-core-ced-analyze` sibling -- there
//! is no compiled cross-lane `zig-gnu-2.27` binary available in a dev
//! `cargo test` run. Genuine `dlopen`/model-load/classify proof against a
//! real compiled `libced.so` lives in `solstone-core-ced-analyze`'s own
//! tests, which own that boundary now.

use std::fs;
use std::path::PathBuf;

use serde_json::json;
use solstone_core_assets::canonical_host_pair;
use solstone_core_local::install::ced_fixture::{
    write_ced_model_bytes, write_complete_ced_install,
};
use solstone_core_local::install::ced_install::{
    ced_artifact_key, ced_library_path, ced_model_path,
};
use solstone_core_local::install::ced_readiness::{
    CedDegradedCause, CedReadiness, evaluate_ced_readiness,
    evaluate_ced_readiness_against_with_probe,
};
use solstone_core_local::install::ced_runtime::CedAnalyzeProgram;
use solstone_core_sound_tags::{tag_audio, tag_audio_with_readiness_and_program};

#[test]
fn missing_assets_degrade_to_none() {
    let journal = tempfile::tempdir().expect("temporary journal");
    assert_eq!(tag_audio(&one_second(), journal.path()), None);
}

#[test]
fn integrity_invalid_model_degrades() {
    let Some(key) = host_key() else {
        return;
    };
    let journal = tempfile::tempdir().expect("temporary journal");
    write_complete_ced_install(journal.path(), key).expect("complete install");
    match evaluate_ced_readiness(journal.path(), host_os(), host_arch()) {
        CedReadiness::Degraded {
            cause: CedDegradedCause::IntegrityInvalid,
            ..
        } => {}
        other => panic!("expected integrity-invalid, got {other:?}"),
    }
    assert_eq!(tag_audio(&one_second(), journal.path()), None);
}

#[test]
fn unloadable_ready_verdict_degrades_tag_audio_to_none() {
    let readiness = CedReadiness::Degraded {
        cause: CedDegradedCause::Unloadable,
        detail: "stub: engine refused to load".to_owned(),
    };
    assert_eq!(
        tag_audio_with_readiness_and_program(
            &one_second(),
            readiness,
            &CedAnalyzeProgram::Explicit {
                executable: PathBuf::from("/should/never/run"),
                args: Vec::new(),
            },
        ),
        None,
        "a Degraded verdict must never invoke the helper at all"
    );
}

#[test]
fn successful_tags_match_the_stub_contract() {
    let Some(assets) = assets() else {
        return;
    };
    let (_root, program) = stub_program(
        "#!/bin/sh\ncat >/dev/null\nprintf '%s\\n' '{\"schema\":\"solstone-ced-response-v1\",\"windows\":[{\"ok\":true,\"tags\":{\"Music\":0.9,\"Above\":0.11}}]}'\n",
    );
    let tags =
        tag_audio_with_readiness_and_program(&one_second(), ready_verdict(&assets), &program)
            .expect("stub tags");
    assert_eq!(
        tags,
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
    let Some(assets) = assets() else {
        return;
    };
    let (_root, program) = stub_program(
        "#!/bin/sh\ncat >/dev/null\nprintf '%s\\n' '{\"schema\":\"solstone-ced-response-v1\",\"windows\":[{\"ok\":false,\"reason\":\"classify-failed\",\"detail\":\"stub failure\"},{\"ok\":true,\"tags\":{\"Music\":0.9}}]}'\n",
    );
    let mut audio = vec![-1.0; 160_000];
    audio.extend(one_second());

    let tags = tag_audio_with_readiness_and_program(&audio, ready_verdict(&assets), &program)
        .expect("one successful window");
    assert_eq!(tags["windows"], 1);
    assert_eq!(tags["tags"]["Music"], 0.9);
}

#[test]
fn helper_process_failure_degrades_to_none() {
    let Some(assets) = assets() else {
        return;
    };
    let (_root, program) = stub_program(
        "#!/bin/sh\ncat >/dev/null\nprintf '%s\\n' '{\"schema\":\"solstone-ced-error-v1\",\"reason\":\"library-unloadable\",\"detail\":\"boom\"}' >&2\nexit 69\n",
    );
    assert_eq!(
        tag_audio_with_readiness_and_program(&one_second(), ready_verdict(&assets), &program),
        None
    );
}

#[test]
fn malformed_helper_response_degrades_to_none() {
    let Some(assets) = assets() else {
        return;
    };
    let (_root, program) = stub_program("#!/bin/sh\ncat >/dev/null\nprintf 'not json'\n");
    assert_eq!(
        tag_audio_with_readiness_and_program(&one_second(), ready_verdict(&assets), &program),
        None
    );
}

#[test]
fn window_count_mismatch_degrades_to_none() {
    let Some(assets) = assets() else {
        return;
    };
    // One window worth of audio requested; the stub reports two.
    let (_root, program) = stub_program(
        "#!/bin/sh\ncat >/dev/null\nprintf '%s\\n' '{\"schema\":\"solstone-ced-response-v1\",\"windows\":[{\"ok\":true,\"tags\":{}},{\"ok\":true,\"tags\":{}}]}'\n",
    );
    assert_eq!(
        tag_audio_with_readiness_and_program(&one_second(), ready_verdict(&assets), &program),
        None
    );
}

#[test]
fn ready_layout_maps_linux_x64_linux_arm64_and_macos_metal() {
    let Some(_) = host_key() else {
        return;
    };
    for (os, arch, key) in [
        ("linux", "x86_64", "linux-cpu-x64"),
        ("linux", "arm64", "linux-cpu-arm64"),
        ("darwin", "arm64", "macos-metal-arm64"),
    ] {
        let journal = tempfile::tempdir().expect("temporary journal");
        write_complete_ced_install(journal.path(), key).expect("complete install");
        let library = ced_library_path(journal.path(), key);
        let digest = solstone_core_local::install::ced_fixture::ced_model_digest(journal.path())
            .expect("fixture digest");
        match evaluate_ced_readiness_against_with_probe(
            journal.path(),
            os,
            arch,
            &digest,
            |_library, _model| Ok(()),
        ) {
            CedReadiness::Ready {
                library: ready_library,
                model,
            } => {
                assert_eq!(ready_library, library);
                assert_eq!(model, ced_model_path(journal.path()));
            }
            other => panic!("{os}/{arch} expected ready, got {other:?}"),
        }
        assert_eq!(ced_artifact_key(os, arch), Some(key));
    }
}

struct Assets {
    _journal: tempfile::TempDir,
    library: PathBuf,
    model: PathBuf,
}

fn assets() -> Option<Assets> {
    let journal = tempfile::tempdir().expect("temporary journal");
    let key = host_key()?;
    write_complete_ced_install(journal.path(), key).expect("complete install");
    // Real bytes never matter here -- the stub program answers every
    // classify request without touching them -- but a real path on disk
    // keeps the request payload honest.
    write_ced_model_bytes(journal.path(), b"stub model bytes").expect("model bytes");
    let library = ced_library_path(journal.path(), key);
    let model = ced_model_path(journal.path());
    Some(Assets {
        _journal: journal,
        library,
        model,
    })
}

fn ready_verdict(assets: &Assets) -> CedReadiness {
    CedReadiness::Ready {
        library: assets.library.clone(),
        model: assets.model.clone(),
    }
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

fn host_key() -> Option<&'static str> {
    let (os, arch) = canonical_host_pair(std::env::consts::OS, std::env::consts::ARCH);
    ced_artifact_key(os, arch)
}

fn host_os() -> &'static str {
    canonical_host_pair(std::env::consts::OS, std::env::consts::ARCH).0
}

fn host_arch() -> &'static str {
    canonical_host_pair(std::env::consts::OS, std::env::consts::ARCH).1
}

fn one_second() -> Vec<f32> {
    vec![0.0; 16_000]
}
