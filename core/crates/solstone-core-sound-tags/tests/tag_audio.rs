// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::json;
use solstone_core_sound_tags::tag_audio;

const MODEL_SIZE_BYTES: u64 = 6_211_616;

#[test]
fn missing_assets_degrade_to_none() {
    let journal = tempfile::tempdir().expect("temporary journal");

    assert_eq!(tag_audio(&one_second(), journal.path()), None);
}

#[test]
fn invalid_assets_degrade_to_none() {
    let Some(stub_assets) = assets(1) else {
        return;
    };
    fs::remove_file(&stub_assets.library).expect("remove library");
    assert_eq!(tag_audio(&one_second(), stub_assets.journal()), None);

    let Some(stub_assets) = assets(1) else {
        return;
    };
    fs::remove_file(&stub_assets.model).expect("remove model");
    assert_eq!(tag_audio(&one_second(), stub_assets.journal()), None);

    let Some(stub_assets) = assets(2) else {
        return;
    };
    write_model(&stub_assets.model, None);
    assert_eq!(tag_audio(&one_second(), stub_assets.journal()), None);

    let Some(stub_assets) = assets(1) else {
        return;
    };
    write_model(&stub_assets.model, Some("NULL_LOAD"));
    assert_eq!(tag_audio(&one_second(), stub_assets.journal()), None);
}

#[test]
fn successful_tags_match_the_python_contract_and_free_every_result() {
    let Some(assets) = assets(1) else {
        return;
    };
    write_model(&assets.model, None);

    let tags = tag_audio(&one_second(), assets.journal()).expect("stub tags");
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
    write_model(&assets.model, None);
    let mut audio = vec![-1.0; 160_000];
    audio.extend(one_second());

    let tags = tag_audio(&audio, assets.journal()).expect("one successful tail window");
    assert_eq!(tags["windows"], 1);
    assert_eq!(tags["tags"]["Music"], 0.9);
    let counters = counters(&assets.model);
    assert_eq!(counters["classify"], 2);
    assert_eq!(counters["free_string"], 1);
    assert_eq!(counters["context_free"], 1);
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
    let root = journal.path().join("cache/providers/ced/v0.1.0");
    let Some(artifact) = artifact_key() else {
        eprintln!("skipping CED stub test: unsupported host platform");
        return None;
    };
    let Some(library_name) = library_name() else {
        eprintln!("skipping CED stub test: unsupported host platform");
        return None;
    };
    let engine = root.join("engine").join(artifact);
    fs::create_dir_all(&engine).expect("engine directory");
    let model = root
        .join("models/mudler__ced-gguf")
        .join("b5e9a4aad6438763c8da16079d77563fbed35c65")
        .join("ced-tiny-q8_0.gguf");
    fs::create_dir_all(model.parent().expect("model parent")).expect("model directory");
    write_model(&model, None);
    let library = engine.join(library_name);
    if !compile_stub(&library, abi) {
        eprintln!("skipping CED stub test: no usable C compiler");
        return None;
    }
    fs::write(engine.join("ced_capi.h"), b"/* test header */\n").expect("stub header");
    Some(Assets {
        journal,
        library,
        model,
    })
}

fn write_model(path: &Path, marker: Option<&str>) {
    let file = File::create(path).expect("model file");
    file.set_len(MODEL_SIZE_BYTES).expect("model size");
    if let Some(marker) = marker {
        use std::io::Write;
        let mut file = file;
        file.write_all(marker.as_bytes()).expect("model marker");
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

fn artifact_key() -> Option<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => Some("linux-cpu-x64"),
        ("linux", "aarch64") => Some("linux-cpu-arm64"),
        ("macos", "aarch64") => Some("macos-metal-arm64"),
        _ => None,
    }
}

fn library_name() -> Option<&'static str> {
    match std::env::consts::OS {
        "linux" => Some("libced.so"),
        "macos" => Some("libced.dylib"),
        _ => None,
    }
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
