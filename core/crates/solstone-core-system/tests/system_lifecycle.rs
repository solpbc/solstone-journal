// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#[path = "fixture_binary.rs"]
mod fixture_binary;
#[path = "lifecycle.rs"]
mod lifecycle;
#[cfg(target_os = "macos")]
#[path = "macos_process_census.rs"]
mod macos_process_census;
#[path = "managed_process.rs"]
mod managed_process;
