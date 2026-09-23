// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! solstone-core-generate is publishable and cannot depend on solstone-core-system,
//! so it keeps its own copy of the Windows launch-only environment names. This crate
//! depends on both; the two lists must stay equal.
#![cfg(windows)]

#[test]
fn generate_keeps_the_same_launch_only_names_as_the_launcher() {
    let mut system: Vec<_> =
        solstone_core_system::process::launch_only_environment_names().collect();
    let mut generate = solstone_core_generate::WINDOWS_LAUNCH_ONLY_ENVIRONMENT.to_vec();
    system.sort_unstable();
    generate.sort_unstable();
    assert_eq!(system, generate);
}
