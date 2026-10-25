// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Owner actions supported by the journal's native host, independent of its browser.

#[derive(Debug, Clone, Copy, serde::Serialize)]
pub struct OwnerHostCapabilities {
    pub bundled_local_thinking: bool,
    pub desktop_capture: bool,
    pub terminal_speech_setup: bool,
    pub same_computer_pairing: bool,
}

pub const fn capabilities() -> OwnerHostCapabilities {
    let desktop = cfg!(any(
        target_os = "linux",
        target_os = "macos",
        target_os = "windows"
    ));
    OwnerHostCapabilities {
        bundled_local_thinking: desktop,
        desktop_capture: desktop,
        terminal_speech_setup: cfg!(any(target_os = "linux", target_os = "macos")),
        same_computer_pairing: desktop,
    }
}
