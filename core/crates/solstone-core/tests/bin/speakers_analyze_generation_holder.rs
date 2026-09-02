// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Real speakers-analyze generation entry point for supervisor process-tree
//! tests. Unlike the plain fixture children, this binary genuinely calls
//! [`enter_speakers_analyze_generation`] so a process-tree test can prove an
//! actual inherited descriptor capability rather than an environment-only
//! mock. It always declares [`SpeakersAnalyzeOwnerRole::Transcribe`], the
//! role a real Sense-spawned transcribe child would use; whether it borrows
//! or acquires depends only on whether it inherited a live generation, never
//! on an argv/env mode flag.

use std::env;
use std::ffi::OsStr;
use std::fs;
use std::path::PathBuf;
use std::thread;
use std::time::Duration;

use solstone_core_transcribe::{SpeakersAnalyzeOwnerRole, enter_speakers_analyze_generation};

const GENERATION_ID_ENV: &str = "SOL_SPEAKERS_ANALYZE_INSTALL_GENERATION_ID";

fn main() {
    let mut args = env::args().skip(1);
    let journal = PathBuf::from(args.next().expect("journal path"));
    let marker = PathBuf::from(args.next().expect("marker path"));
    let had_inherited_env = env::var_os(GENERATION_ID_ENV).is_some();

    let record =
        match enter_speakers_analyze_generation(&journal, SpeakersAnalyzeOwnerRole::Transcribe) {
            Ok(generation) => {
                let environment = generation.inheritance_environment();
                let install_generation_id = environment
                    .get(OsStr::new(GENERATION_ID_ENV))
                    .map(|value| value.to_string_lossy().into_owned());
                serde_json::json!({
                    "ok": true,
                    "borrowed": had_inherited_env,
                    "install_generation_id": install_generation_id,
                    "pid": std::process::id(),
                })
            }
            Err(error) => serde_json::json!({
                "ok": false,
                "borrowed": had_inherited_env,
                "message": error.message().unwrap_or_default(),
                "exit_code": error.exit_code(),
            }),
        };
    fs::write(
        &marker,
        serde_json::to_vec(&record).expect("serialize marker"),
    )
    .expect("write marker");

    // Park like the other supervised app fixtures so process-tree and
    // shutdown tests can observe and terminate this process normally.
    loop {
        thread::sleep(Duration::from_secs(3600));
    }
}
