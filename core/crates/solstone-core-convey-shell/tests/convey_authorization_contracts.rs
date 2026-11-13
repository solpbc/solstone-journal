// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

// The door and authorization-gate suites exercise unix inodes, FIFOs and sockets;
// ci/suites.toml runs this target on linux and macos only.
#![cfg(unix)]

#[path = "door_support.rs"]
mod door_support;
#[path = "support/warn_capture.rs"]
mod warn_capture;

#[path = "authorization_gate.rs"]
mod authorization_gate;
#[path = "door_e2e.rs"]
mod door_e2e;
