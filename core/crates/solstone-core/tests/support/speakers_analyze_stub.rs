// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Test-only speakers-analyze stub wiring for supervisor subprocess fixtures.
//!
//! These supervisor race-test fixtures never run real transcription; they
//! only need `enter_speakers_analyze_generation`'s installation validation to
//! succeed so a real generation-owning boot proceeds far enough to spawn the
//! fixture app children. This points a spawned `solstone-core supervisor`
//! subprocess at the repository's real, checked-in model assets and an
//! already-executable (never invoked) stand-in helper binary, through the
//! same production override env vars `solstone-core-transcribe` already
//! supports for out-of-process tests (see
//! `solstone-core-transcribe-cli/tests/reachability.rs`).

use std::path::PathBuf;
use std::process::Command;

/// Apply the env vars a spawned `solstone-core supervisor` subprocess needs
/// so its speakers-analyze generation acquisition succeeds.
pub fn apply(command: &mut Command) {
    command
        .env(
            "SOLSTONE_SPEAKERS_ANALYZE_BINARY",
            env!("CARGO_BIN_EXE_solstone-core-system-test-child"),
        )
        .env("SOLSTONE_TRANSCRIBE_MODEL_ASSETS_DIR", model_assets_dir());
}

fn model_assets_dir() -> PathBuf {
    repository_root().join("core/models/assets")
}

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("repository root")
        .to_path_buf()
}
