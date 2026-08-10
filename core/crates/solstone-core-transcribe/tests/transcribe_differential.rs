// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Differential harness preconditions for the non-ORT transcribe path.
//!
//! The AC16 CLI test owns the composed stub-helper execution because Cargo can
//! expose that package's executable paths there.  This feature-gated target
//! establishes the Python oracle and committed input fixture that the broader
//! header/statement differential consumes in an installed environment.

use std::path::PathBuf;
use std::process::Command;

#[test]
fn python_reference_and_parakeet_fixture_are_available() {
    let root = repository_root();
    assert!(
        root.join("solstone/observe/transcribe/_fixtures/parakeet_sample.wav")
            .is_file(),
        "the shared parakeet differential fixture is missing"
    );
    let python = root.join(".venv/bin/python3");
    assert!(
        python.is_file(),
        "make check-differentials must provision .venv"
    );
    let output = Command::new(python)
        .args(["-c", "import solstone.observe.transcribe.main"])
        .current_dir(&root)
        .output()
        .expect("start Python transcribe reference import");
    assert!(
        output.status.success(),
        "Python transcribe reference is unavailable: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("repository root")
        .to_path_buf()
}
