// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Exact import-produced audio bytes admitted to the mobile shared worker.
//! This is an import receipt, separate from device network-ingest receipts.

#[cfg(unix)]
mod native {
    use std::ffi::OsStr;
    use std::io::Read;
    use std::path::Path;

    use serde::{Deserialize, Serialize};
    use sha2::{Digest, Sha256};
    use solstone_core_journal_io::{
        FlatDirectory, JournalRoot, read_observed_file_bounded,
        write_bytes_exclusive_bound_detailed_checked,
    };

    const NAME: &str = "imported-audio-custody.json";
    const SCHEMA: &str = "solstone.import.audio-input.v1";

    #[derive(Deserialize, Serialize)]
    #[serde(deny_unknown_fields)]
    struct Receipt {
        schema: String,
        import_id: String,
        filename: String,
        size: u64,
        sha256: String,
    }

    /// Retain the segment allocated by the importer through hashing and durable
    /// create-only publication. No receipt is inferred from current bytes later.
    pub fn publish(segment: &JournalRoot, import_id: &str, filename: &OsStr) -> Result<(), String> {
        let directory = FlatDirectory::from_root(segment).map_err(|e| e.to_string())?;
        let observed = directory
            .observe(filename)
            .map_err(|e| e.to_string())?
            .ok_or("imported audio is missing")?;
        let mut file = directory
            .open_regular_file(&observed)
            .map_err(|e| e.to_string())?;
        segment
            .revalidate_canonical_binding()
            .map_err(|e| e.to_string())?;
        let mut digest = Sha256::new();
        let mut size = 0u64;
        let mut buffer = [0u8; 65536];
        loop {
            // This loop owns the original full WorkStop installed by the callable.
            if !solstone_core_import::work_continues() {
                return Err("import custody verification cancelled".into());
            }
            let count = file.read(&mut buffer).map_err(|e| e.to_string())?;
            if count == 0 {
                break;
            }
            size = size
                .checked_add(count as u64)
                .ok_or("audio size overflow")?;
            digest.update(&buffer[..count]);
        }
        let checkpoint = || -> Result<(), String> {
            if !solstone_core_import::work_continues() {
                return Err("import custody verification cancelled".into());
            }
            segment
                .revalidate_canonical_binding()
                .map_err(|e| e.to_string())?;
            directory
                .verify_regular_file(&file, &observed)
                .map_err(|e| e.to_string())
        };
        checkpoint()?;
        let filename = filename.to_str().ok_or("audio filename is not UTF-8")?;
        let receipt = Receipt {
            schema: SCHEMA.into(),
            import_id: import_id.into(),
            filename: filename.into(),
            size,
            sha256: format!("{:x}", digest.finalize()),
        };
        let bytes = serde_json::to_vec(&receipt).map_err(|e| e.to_string())?;
        let published = write_bytes_exclusive_bound_detailed_checked(
            &directory,
            OsStr::new(NAME),
            &bytes,
            0o600,
            &checkpoint,
        )
        .map_err(|e| e.to_string())?;
        if !published.is_fully_confirmed() {
            return Err("audio custody publication was not fully confirmed".into());
        }
        checkpoint()
    }

    /// Read only a complete receipt beneath the original retained journal root.
    /// The consumer still hashes its retained input and compares this witness.
    pub fn read(root: &JournalRoot, input: &Path) -> Result<Option<(u64, String)>, String> {
        read_checked(root, input, &|| !solstone_core_import::work_continues())
    }

    pub fn read_checked(
        root: &JournalRoot,
        input: &Path,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<Option<(u64, String)>, String> {
        if cancelled() {
            return Err("custody verification cancelled".into());
        }
        root.revalidate_canonical_binding()
            .map_err(|e| e.to_string())?;
        let relative = input
            .strip_prefix(root.canonical_path().join("chronicle"))
            .map_err(|e| e.to_string())?;
        if relative.components().count() != 4 {
            return Ok(None);
        }
        let parent = input.parent().ok_or("input parent missing")?;
        let relative_parent = parent
            .strip_prefix(root.canonical_path())
            .map_err(|e| e.to_string())?;
        let directory = FlatDirectory::open(root, relative_parent).map_err(|e| e.to_string())?;
        let receipt = read_observed_file_bounded(&directory, OsStr::new(NAME), 16384)
            .map_err(|e| e.to_string())?;
        root.revalidate_canonical_binding()
            .map_err(|e| e.to_string())?;
        let current = FlatDirectory::open(root, relative_parent).map_err(|e| e.to_string())?;
        if current.identity() != directory.identity() {
            return Err("imported audio parent changed".into());
        }
        let Some(receipt) = receipt else {
            return solstone_core_import::audio_archive::read_published_part_receipt_bound(
                root, input, cancelled,
            );
        };
        let receipt: Receipt = serde_json::from_slice(&receipt.bytes)
            .map_err(|_| "unreadable imported audio custody")?;
        if receipt.schema != SCHEMA
            || receipt.import_id.is_empty()
            || Some(OsStr::new(&receipt.filename)) != input.file_name()
            || receipt.sha256.len() != 64
            || !receipt.sha256.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Err("invalid imported audio custody".into());
        }
        Ok(Some((receipt.size, receipt.sha256)))
    }
}

#[cfg(unix)]
pub use native::{publish, read, read_checked};
