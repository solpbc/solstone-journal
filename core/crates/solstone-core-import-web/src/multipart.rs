// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use axum::extract::multipart::Field;

pub(crate) const MAX_BODY_BYTES: usize = 128 * 1024 * 1024;
pub(crate) const MAX_SAVE_FILE_BYTES: u64 = 4 * 1024 * 1024 * 1024;
pub(crate) const MAX_PART_BYTES: usize = 64 * 1024 * 1024;
pub(crate) const MAX_PARTS: usize = 12;
pub(crate) const MAX_HEADERS: usize = 16;
pub(crate) const MAX_FILENAME_BYTES: usize = 128;

pub(crate) async fn bounded(mut field: Field<'_>) -> Result<Vec<u8>, &'static str> {
    let mut bytes = Vec::new();
    while let Some(chunk) = field
        .chunk()
        .await
        .map_err(|_| "cannot read multipart part")?
    {
        if bytes
            .len()
            .checked_add(chunk.len())
            .is_none_or(|len| len > MAX_PART_BYTES)
        {
            return Err("multipart part exceeds 64 MiB");
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}
