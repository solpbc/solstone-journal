// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::json;
use solstone_core_assets::canonical_host_pair;
use solstone_core_local::install::capability_status::CapabilityStatus;
use solstone_core_local::install::ced_fixture::{
    ced_model_digest, write_ced_model_bytes, write_complete_ced_install,
};
use solstone_core_local::install::ced_install::{
    ced_artifact_key, ced_library_path, ced_model_path,
};
use solstone_core_local::install::ced_readiness::{
    CedVerdict, evaluate_ced_readiness, evaluate_ced_readiness_against,
};
use solstone_core_sound_tags::{tag_audio, tag_audio_with_readiness};

#[test]
fn missing_assets_degrade_to_none() {
    let journal = tempfile::tempdir().expect("temporary journal");
    assert_eq!(tag_audio(&one_second(), journal.path()), None);
}

#[test]
fn unsupported_platform_returns_none() {
    // Exercises CedVerdict::Unsupported in tag_audio_with_readiness
    // (distinct warn: "sound tagger disabled: ced assets unsupported on {os}/{arch}").
    let verdict = CedVerdict::Unsupported {
        os: "windows".to_owned(),
        arch: "x86_64".to_owned(),
    };
    assert_eq!(tag_audio_with_readiness(&one_second(), verdict), None);
}

#[test]
fn integrity_invalid_model_degrades() {
    let Some(key) = host_key() else {
        return;
    };
    let journal = tempfile::tempdir().expect("temporary journal");
    write_complete_ced_install(journal.path(), key).expect("complete install");
    match evaluate_ced_readiness(journal.path(), host_os(), host_arch()) {
        CedVerdict::Degraded(CapabilityStatus::IntegrityInvalid { .. }) => {}
        other => panic!("expected integrity-invalid, got {other:?}"),
    }
    assert_eq!(tag_audio(&one_second(), journal.path()), None);
}

#[test]
fn unloadable_null_load_degrades() {
    let Some(assets) = assets(1) else {
        return;
    };
    write_ced_model_bytes(assets.journal(), b"NULL_LOAD").expect("null-load marker");
    let digest = ced_model_digest(assets.journal()).expect("fixture digest");
    let (os, arch) = (host_os(), host_arch());
    match evaluate_ced_readiness_against(assets.journal(), os, arch, &digest) {
        CedVerdict::Degraded(CapabilityStatus::UnloadableOrUnrunnable { .. }) => {}
        other => panic!("expected unloadable, got {other:?}"),
    }
    assert_eq!(tag_audio(&one_second(), assets.journal()), None);
}

#[test]
fn successful_tags_match_the_python_contract_and_free_every_result() {
    let Some(assets) = assets(1) else {
        return;
    };
    let tags = tag_audio_with_readiness(&one_second(), ready_verdict(&assets)).expect("stub tags");
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
    let counters = counters(&assets.model);
    assert_eq!(counters["classify"], 1);
    assert_eq!(counters["free_string"], counters["classify"]);
    assert_eq!(counters["context_free"], 1);
}

#[test]
fn a_failed_window_keeps_successful_windows_without_freeing_null() {
    let Some(assets) = assets(1) else {
        return;
    };
    let mut audio = vec![-1.0; 160_000];
    audio.extend(one_second());

    let tags = tag_audio_with_readiness(&audio, ready_verdict(&assets))
        .expect("one successful tail window");
    assert_eq!(tags["windows"], 1);
    assert_eq!(tags["tags"]["Music"], 0.9);
    let counters = counters(&assets.model);
    assert_eq!(counters["classify"], 2);
    assert_eq!(counters["free_string"], 1);
    assert_eq!(counters["context_free"], 1);
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
        if !compile_stub(&library, 1) {
            return;
        }
        let digest = ced_model_digest(journal.path()).expect("fixture digest");
        match evaluate_ced_readiness_against(journal.path(), os, arch, &digest) {
            CedVerdict::Ready {
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
    journal: tempfile::TempDir,
    library: PathBuf,
    model: PathBuf,
}

impl Assets {
    fn journal(&self) -> &Path {
        self.journal.path()
    }
}

fn assets(abi: i32) -> Option<Assets> {
    let journal = tempfile::tempdir().expect("temporary journal");
    let Some(key) = host_key() else {
        eprintln!("skipping CED stub test: unsupported host platform");
        return None;
    };
    write_complete_ced_install(journal.path(), key).expect("complete install");
    let library = ced_library_path(journal.path(), key);
    if !compile_stub(&library, abi) {
        eprintln!("skipping CED stub test: no usable C compiler");
        return None;
    }
    let model = ced_model_path(journal.path());
    Some(Assets {
        journal,
        library,
        model,
    })
}

fn ready_verdict(assets: &Assets) -> CedVerdict {
    CedVerdict::Ready {
        library: assets.library.clone(),
        model: assets.model.clone(),
    }
}

fn compile_stub(output: &Path, abi: i32) -> bool {
    let compiler = std::env::var("CC").unwrap_or_else(|_| "cc".to_owned());
    if Command::new(&compiler).arg("--version").output().is_err() {
        return false;
    }
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/ced_stub.c");
    let mut command = Command::new(compiler);
    if std::env::consts::OS == "macos" {
        command.arg("-dynamiclib");
    } else {
        command.args(["-shared", "-fPIC"]);
    }
    let output = command
        .arg(format!("-DCED_TEST_ABI={abi}"))
        .arg(source)
        .arg("-o")
        .arg(output)
        .output()
        .expect("start C compiler");
    assert!(
        output.status.success(),
        "compile CED stub failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    true
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

fn counters(model: &Path) -> BTreeMap<String, usize> {
    fs::read_to_string(model.with_extension("gguf.counts"))
        .expect("stub counters")
        .lines()
        .fold(BTreeMap::new(), |mut counts, event| {
            *counts.entry(event.to_owned()).or_default() += 1;
            counts
        })
}
