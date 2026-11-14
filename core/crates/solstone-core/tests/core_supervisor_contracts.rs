// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

// Every included test module is unix-only; the support modules serve only them.
#![cfg(unix)]

#[path = "support/installation_binding.rs"]
mod installation_binding;
#[path = "support/speakers_analyze_stub.rs"]
mod speakers_analyze_stub;
#[path = "supervisor_boot.rs"]
mod supervisor_boot;
#[path = "support/supervisor_guard.rs"]
mod supervisor_guard;
#[path = "supervisor_providers.rs"]
mod supervisor_providers;
#[path = "support/temporary_root.rs"]
mod temporary_root;
