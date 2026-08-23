// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Test-only hooks for external integration tests. Not part of the public API contract.

use std::path::Path;

use crate::PeerExportAreaResult;
use crate::peer::PeerLoopbackClient;
use crate::peer_export::{area_result, export_segments};

pub fn export_segments_result(
    journal: &Path,
    base_url: &str,
    days: &[String],
    dry_run: bool,
) -> PeerExportAreaResult {
    let loopback = PeerLoopbackClient::for_test(base_url);
    area_result(
        "segments",
        export_segments(journal, &loopback, "test", days, dry_run),
    )
}
