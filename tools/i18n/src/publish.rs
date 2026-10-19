// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::fault::Fault;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

pub fn atomic_write_file(dest: &Path, bytes: &[u8]) -> Result<(), Fault> {
    let parent = dest.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).map_err(|_| Fault::catalog("io-error"))?;

    let file_name = dest.file_name().and_then(|n| n.to_str()).unwrap_or("file");
    let pid = std::process::id();

    let mut attempts = 0;
    let (mut file, temp_path) = loop {
        let count = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let temp_name = format!("{}.tmp.{}.{}", file_name, pid, count);
        let p = parent.join(temp_name);
        match OpenOptions::new().write(true).create_new(true).open(&p) {
            Ok(f) => break (f, p),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                attempts += 1;
                if attempts > 1000 {
                    return Err(Fault::catalog("io-error"));
                }
                continue;
            }
            Err(_) => return Err(Fault::catalog("io-error")),
        }
    };

    let write_res = (|| -> Result<(), std::io::Error> {
        file.write_all(bytes)?;
        file.sync_all()?;
        Ok(())
    })();

    if let Err(_e) = write_res {
        let _ = fs::remove_file(&temp_path);
        return Err(Fault::catalog("io-error"));
    }

    drop(file);

    if let Err(_e) = fs::rename(&temp_path, dest) {
        let _ = fs::remove_file(&temp_path);
        return Err(Fault::catalog("io-error"));
    }

    Ok(())
}
