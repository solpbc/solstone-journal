// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

// The native integration runner executes the binary's actual portable modules.
#[allow(dead_code)]
#[path = "../src/prefs.rs"]
mod prefs;
#[allow(dead_code)]
#[path = "../src/update.rs"]
mod update;

// Compile the production Windows journal bridge used by startup/apply. The
// updater cases inject ApplyHooks, so these imports do not start a journal.
#[cfg(windows)]
#[allow(dead_code)]
#[path = "../src/journal.rs"]
mod journal;
#[cfg(windows)]
#[allow(dead_code)]
#[path = "../src/setup_events.rs"]
mod setup_events;
#[cfg(windows)]
#[allow(dead_code)]
#[path = "../src/status.rs"]
mod status;
