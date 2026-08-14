// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use axum::extract::{Multipart, multipart::Field};

pub(crate) const MAX_BODY_BYTES: usize = 128 * 1024 * 1024;
const MAX_PART_BYTES: usize = 64 * 1024 * 1024;
const MAX_PARTS: usize = 12;
const MAX_HEADERS: usize = 16;
const MAX_FILENAME_BYTES: usize = 128;

pub(crate) struct Part {
    pub(crate) name: String,
    pub(crate) filename: Option<String>,
    pub(crate) content_type: Option<String>,
    pub(crate) bytes: Vec<u8>,
}

pub(crate) async fn collect(mut multipart: Multipart) -> Result<Vec<Part>, &'static str> {
    let mut parts = Vec::new();
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|_| "invalid multipart body")?
    {
        if parts.len() == MAX_PARTS {
            return Err("too many multipart parts");
        }
        if field.headers().len() > MAX_HEADERS {
            return Err("too many multipart headers");
        }
        let filename = field.file_name().map(ToOwned::to_owned);
        if filename
            .as_ref()
            .is_some_and(|value| value.len() > MAX_FILENAME_BYTES)
        {
            return Err("multipart filename is too long");
        }
        let name = field.name().unwrap_or_default().to_owned();
        let content_type = field.content_type().map(ToOwned::to_owned);
        parts.push(Part {
            name,
            filename,
            content_type,
            bytes: bounded(field).await?,
        });
    }
    Ok(parts)
}

async fn bounded(mut field: Field<'_>) -> Result<Vec<u8>, &'static str> {
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
