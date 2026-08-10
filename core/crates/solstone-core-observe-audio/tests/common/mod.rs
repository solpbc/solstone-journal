// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![allow(clippy::disallowed_methods, clippy::disallowed_types, dead_code)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

pub struct TempDir {
    path: PathBuf,
}

impl TempDir {
    pub fn new(name: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "solstone-observe-audio-{name}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).expect("create temporary test directory");
        Self { path }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

pub fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../..")
        .canonicalize()
        .expect("canonical repository root")
}

pub fn fixture_source() -> PathBuf {
    repo_root().join("solstone/observe/transcribe/_fixtures/parakeet_sample.wav")
}

pub fn fixture_m4a_multi_track() -> PathBuf {
    repo_root().join("tests/fixtures/audio/aac_multi_track.m4a")
}

pub fn python(script: &str, arguments: &[&Path]) {
    let python = repo_root().join(".venv/bin/python");
    let status = Command::new(python)
        .arg("-c")
        .arg(script)
        .args(arguments)
        .current_dir(repo_root())
        .status()
        .expect("run Python oracle");
    assert!(status.success(), "Python oracle failed");
}

pub fn render_fixture(output: &Path, arguments: &[&str]) {
    let status = Command::new("ffmpeg")
        .arg("-y")
        .arg("-v")
        .arg("error")
        .args(arguments)
        .arg(output)
        .status()
        .expect("run ffmpeg fixture renderer");
    assert!(
        status.success(),
        "ffmpeg fixture renderer failed for {output:?}"
    );
}

pub fn generated_decode_corpus(temp: &TempDir) -> Vec<PathBuf> {
    let source = fixture_source();
    let source = source.to_str().expect("UTF-8 source path");
    let mut outputs = Vec::new();
    for (name, rate) in [
        ("stereo-44100.wav", "44100"),
        ("stereo-44100.flac", "44100"),
        ("stereo-48000.wav", "48000"),
        ("stereo-48000.flac", "48000"),
    ] {
        let output = temp.path().join(name);
        render_fixture(&output, &["-i", source, "-ar", rate, "-ac", "2"]);
        outputs.push(output);
    }
    for (name, codec) in [
        ("render.flac", None),
        ("render.opus", Some("libopus")),
        ("render.ogg", Some("libvorbis")),
        ("render.mp3", Some("libmp3lame")),
        ("render.wav", None),
    ] {
        let output = temp.path().join(name);
        let args = match codec {
            Some(codec) => vec!["-i", source, "-ac", "2", "-c:a", codec],
            None => vec!["-i", source, "-ac", "2"],
        };
        render_fixture(&output, &args);
        outputs.push(output);
    }
    outputs.push(fixture_m4a_multi_track());
    outputs
}

pub fn read_f32le(path: &Path) -> Vec<f32> {
    fs::read(path)
        .expect("read f32le output")
        .chunks_exact(4)
        .map(|bytes| f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
        .collect()
}
