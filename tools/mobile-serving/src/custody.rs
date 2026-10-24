// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use sha2::{Digest, Sha256};
use std::{fs::File, io::Read, path::Path};

/// Publication before receipt persistence is not a completed network input.
/// Match the accepted custody receipt before giving these bytes to a processor.
pub(crate) fn accepted_input(root: &Path, input: &Path) -> Result<bool, String> {
    let segment = input.parent().ok_or("input segment missing")?;
    let relative = segment
        .strip_prefix(root.join("chronicle"))
        .map_err(|e| e.to_string())?;
    let parts = relative
        .iter()
        .map(|p| p.to_string_lossy())
        .collect::<Vec<_>>();
    if parts.len() != 3 {
        return Ok(false);
    }
    let report =
        solstone_core_callosum::read_device_ingest_events(segment).map_err(|e| e.to_string())?;
    if report.unparseable != 0 {
        return Err(format!("unreadable custody receipt: {}", segment.display()));
    }
    let name = input
        .file_name()
        .ok_or("input filename missing")?
        .to_string_lossy();
    let files = report
        .records
        .iter()
        .filter(|r| {
            r.outcome == "accepted"
                && r.day == parts[0]
                && r.stream == parts[1]
                && r.segment == parts[2]
        })
        .flat_map(|r| &r.files)
        .filter(|f| f.written == name)
        .collect::<Vec<_>>();
    if files.is_empty() {
        return Ok(false);
    }
    let mut file = File::open(input).map_err(|e| e.to_string())?;
    let size = file.metadata().map_err(|e| e.to_string())?.len();
    // The processing owner records execution-budget refusals durably. Custody
    // still verifies the entire accepted receipt with a constant-size buffer.
    if !file.metadata().map_err(|e| e.to_string())?.is_file() {
        return Err("input is not a regular file".into());
    }
    let mut hash = Sha256::new();
    let mut bytes = [0u8; 65536];
    loop {
        let n = file.read(&mut bytes).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        hash.update(&bytes[..n]);
    }
    let hash = format!("{:x}", hash.finalize());
    Ok(files.iter().any(|f| f.size == size && f.sha256 == hash))
}
