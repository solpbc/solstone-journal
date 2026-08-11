// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Real-asset parity between the native ced.cpp binding and Python's tagger.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;
use solstone_core_sound_tags::tag_audio;

const SAMPLE_RATE: usize = 16_000;

#[test]
fn native_sound_tags_match_python_at_window_boundaries() {
    let root = repository_root();
    let journal = tempfile::tempdir().expect("scratch journal");
    install_assets(&root, journal.path());

    for (name, audio) in [
        ("short", tone(SAMPLE_RATE / 2)),
        ("proper_tail", tone(11 * SAMPLE_RATE)),
        ("short_tail", tone(10 * SAMPLE_RATE + SAMPLE_RATE / 2)),
    ] {
        let input = journal.path().join(format!("{name}.json"));
        fs::write(&input, serde_json::to_vec(&audio).expect("audio JSON"))
            .expect("write audio input");
        let expected = python_tags(&root, journal.path(), &input);
        let actual = tag_audio(&audio, journal.path());
        assert_eq!(actual, expected, "sound-tag mismatch for {name}");
    }
}

fn install_assets(root: &Path, journal: &Path) {
    let python = root.join(".venv/bin/python3");
    assert!(
        python.is_file(),
        "make check-differentials must provision .venv"
    );
    let script = concat!(
        "from solstone.think.providers.ced_install import install_ced_assets\n",
        "record = install_ced_assets(journal_path=__import__('os').environ['SOLSTONE_JOURNAL'])\n",
        "assert record is not None\n",
    );
    let output = Command::new(python)
        .args(["-c", script])
        .current_dir(root)
        .env("SOLSTONE_JOURNAL", journal)
        .output()
        .expect("start CED asset installer");
    assert!(
        output.status.success(),
        "CED test asset provisioning failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn python_tags(root: &Path, journal: &Path, input: &Path) -> Option<Value> {
    let script = concat!(
        "import json, os, sys\n",
        "import numpy as np\n",
        "from solstone.observe.transcribe.sound_tags import tag_audio\n",
        "with open(os.environ['SOLSTONE_SOUND_TAG_AUDIO'], encoding='utf-8') as handle:\n",
        "    audio = np.asarray(json.load(handle), dtype=np.float32)\n",
        "json.dump(tag_audio(audio, 16000), sys.stdout, sort_keys=True)\n",
    );
    let output = Command::new(root.join(".venv/bin/python3"))
        .args(["-c", script])
        .current_dir(root)
        .env("SOLSTONE_JOURNAL", journal)
        .env("SOLSTONE_SOUND_TAG_AUDIO", input)
        .output()
        .expect("start Python sound tagger");
    assert!(
        output.status.success(),
        "Python sound tagger failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("Python sound-tags JSON")
}

fn tone(samples: usize) -> Vec<f32> {
    (0..samples)
        .map(|index| ((index as f32 / SAMPLE_RATE as f32) * 440.0).sin() * 0.2)
        .collect()
}

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("repository root")
        .to_path_buf()
}
