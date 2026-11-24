// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

pub mod describe;
pub mod detection;
pub mod duplicates;
pub mod observer;
pub mod review;
pub mod suggest;

/// Entity outcome files carry source English for direct readers.
fn outcome_error<const N: usize>(id: &str, args: [(&str, &dyn std::fmt::Display); N]) -> String {
    solstone_core_cli_copy::display_for(solstone_core_cli_copy::ENGLISH, id, args)
}
