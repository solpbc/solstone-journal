// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::PathBuf;

pub(super) fn temporary_root() -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        PathBuf::from("/var/tmp")
    }
    #[cfg(not(target_os = "macos"))]
    {
        std::env::temp_dir()
    }
}
