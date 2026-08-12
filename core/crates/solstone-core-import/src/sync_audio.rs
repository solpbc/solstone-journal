// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Local audio folder catalogue sync with a bounded M4A container reader.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};

use crate::dedupe::hash_source;
use crate::sync_state::{
    FileSyncBackend, FileSyncState, SyncActionFailure, SyncActionRequest, SyncActionSeams,
    SyncReport, load_sync_state, run_actions, write_sync_state,
};
use crate::{AutoTimestamp, ImportError};

const MIN_DURATION_SECONDS: f64 = 30.0;
const AUDIO_EXTENSIONS: &[&str] = &["m4a", "mp4"];
const MAX_MOOV_BYTES: u64 = 16 * 1024 * 1024;

/// Audio folder catalogue options, matching the reference keyword shape.
pub struct AudioSyncOptions<'a> {
    pub journal: &'a Path,
    pub save: bool,
    pub source_path: Option<&'a Path>,
    pub force: bool,
    pub auto: AutoTimestamp,
}

pub type AudioSyncState = FileSyncState;
pub type AudioFileState = Map<String, Value>;

pub fn sync_audio<A>(
    options: &AudioSyncOptions<'_>,
    seams: &mut SyncActionSeams<A>,
) -> Result<SyncReport, ImportError>
where
    A: for<'a> FnMut(SyncActionRequest<'a>) -> Result<(), SyncActionFailure>,
{
    let source = options.source_path.ok_or_else(|| {
        catalog_error("Audio sync requires --path pointing to an audio folder".to_owned())
    })?;
    if !source.is_dir() {
        return Err(catalog_error(format!(
            "Audio sync path is not a directory: {}",
            source.display()
        )));
    }
    let source = source
        .canonicalize()
        .map_err(|error| catalog_error(error.to_string()))?;
    let mut state =
        load_sync_state(options.journal, FileSyncBackend::Audio)?.unwrap_or_else(|| {
            FileSyncState::empty(FileSyncBackend::Audio, Some(source.display().to_string()))
        });
    state.source_path = Some(source.display().to_string());
    if options.force {
        state.files.clear();
    }

    let mut paths = BTreeMap::new();
    let mut current = BTreeSet::new();
    let mut audio_count = 0;
    for path in audio_files(&source)? {
        audio_count += 1;
        let relative = path
            .strip_prefix(&source)
            .expect("discovered below source")
            .to_string_lossy()
            .replace('\\', "/");
        current.insert(relative.clone());
        let hash = hash_source(&path)?.into_inner();
        let filesize = fs::metadata(&path)
            .map_err(|error| catalog_error(error.to_string()))?
            .len();
        let mut entry = state
            .files
            .get(&relative)
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        entry.insert(
            "filename".to_owned(),
            Value::String(
                path.file_name()
                    .and_then(|value| value.to_str())
                    .unwrap_or("")
                    .to_owned(),
            ),
        );
        entry.insert("filesize".to_owned(), json!(filesize));
        entry.insert("hash".to_owned(), Value::String(hash));
        match m4a_duration_seconds(&path) {
            Ok(duration) if duration < MIN_DURATION_SECONDS => {
                entry.insert("status".to_owned(), Value::String("skipped".to_owned()));
                entry.insert(
                    "skip_reason".to_owned(),
                    Value::String("too_short".to_owned()),
                );
                entry.insert("duration".to_owned(), json!(duration));
                entry.remove("last_error");
            }
            Ok(duration) => {
                if entry.get("status").and_then(Value::as_str) != Some("imported") {
                    entry.insert("status".to_owned(), Value::String("available".to_owned()));
                }
                entry.insert("duration".to_owned(), json!(duration));
                entry.remove("skip_reason");
                entry.remove("last_error");
            }
            Err(_) => {
                entry.insert("status".to_owned(), Value::String("unreadable".to_owned()));
                entry.remove("duration");
                entry.remove("skip_reason");
                entry.remove("last_error");
            }
        }
        state.files.insert(relative.clone(), Value::Object(entry));
        paths.insert(relative, path);
    }
    if audio_count == 0 {
        return Err(catalog_error(
            "Audio sync path contains no audio files".to_owned(),
        ));
    }
    for (relative, entry) in &mut state.files {
        if !current.contains(relative) {
            entry["status"] = Value::String("removed".to_owned());
        }
    }
    state.stamp();
    write_sync_state(options.journal, &state)?;
    run_actions(&mut state, options.save, options.journal, seams, &paths)
}

fn audio_files(root: &Path) -> Result<Vec<PathBuf>, ImportError> {
    let mut files = Vec::new();
    walk_audio(root, &mut files)?;
    files.sort();
    Ok(files)
}

fn walk_audio(directory: &Path, files: &mut Vec<PathBuf>) -> Result<(), ImportError> {
    for entry in fs::read_dir(directory).map_err(|error| catalog_error(error.to_string()))? {
        let entry = entry.map_err(|error| catalog_error(error.to_string()))?;
        let path = entry.path();
        let metadata = entry
            .metadata()
            .map_err(|error| catalog_error(error.to_string()))?;
        if metadata.is_dir() {
            walk_audio(&path, files)?;
        } else if metadata.is_file()
            && path
                .extension()
                .and_then(|value| value.to_str())
                .is_some_and(|extension| {
                    AUDIO_EXTENSIONS
                        .iter()
                        .any(|expected| extension.eq_ignore_ascii_case(expected))
                })
        {
            files.push(path);
        }
    }
    Ok(())
}

#[derive(Clone, Copy)]
struct Mp4Box {
    kind: [u8; 4],
    start: u64,
    end: u64,
}

fn m4a_duration_seconds(path: &Path) -> Result<f64, ()> {
    let mut file = File::open(path).map_err(|_| ())?;
    let length = file.metadata().map_err(|_| ())?.len();
    let mut cursor = 0;
    let mut saw_ftyp = false;
    while cursor < length {
        let container = read_box(&mut file, cursor, length)?;
        if container.kind == *b"ftyp" {
            saw_ftyp = true;
        }
        if container.kind == *b"moov" {
            if !saw_ftyp || container.end - container.start > MAX_MOOV_BYTES {
                return Err(());
            }
            return mvhd_duration(&mut file, container.start, container.end);
        }
        cursor = container.end;
    }
    Err(())
}

fn mvhd_duration(file: &mut File, start: u64, end: u64) -> Result<f64, ()> {
    let mut cursor = start;
    while cursor < end {
        let child = read_box(file, cursor, end)?;
        if child.kind == *b"mvhd" {
            let size = child.end.checked_sub(child.start).ok_or(())?;
            let read_size = usize::try_from(size.min(32)).map_err(|_| ())?;
            if read_size < 20 {
                return Err(());
            }
            let mut bytes = vec![0; read_size];
            file.seek(SeekFrom::Start(child.start)).map_err(|_| ())?;
            file.read_exact(&mut bytes).map_err(|_| ())?;
            return mvhd_value(&bytes);
        }
        cursor = child.end;
    }
    Err(())
}

fn read_box(file: &mut File, offset: u64, limit: u64) -> Result<Mp4Box, ()> {
    if limit.checked_sub(offset).ok_or(())? < 8 {
        return Err(());
    }
    file.seek(SeekFrom::Start(offset)).map_err(|_| ())?;
    let mut header = [0_u8; 8];
    file.read_exact(&mut header).map_err(|_| ())?;
    let size32 = u64::from(u32::from_be_bytes(header[..4].try_into().map_err(|_| ())?));
    let kind: [u8; 4] = header[4..].try_into().map_err(|_| ())?;
    let (size, header_size) = if size32 == 1 {
        if limit.checked_sub(offset).ok_or(())? < 16 {
            return Err(());
        }
        let mut extended = [0_u8; 8];
        file.read_exact(&mut extended).map_err(|_| ())?;
        (u64::from_be_bytes(extended), 16)
    } else if size32 == 0 {
        (limit.checked_sub(offset).ok_or(())?, 8)
    } else {
        (size32, 8)
    };
    if size < header_size {
        return Err(());
    }
    let end = offset.checked_add(size).ok_or(())?;
    if end > limit || end <= offset {
        return Err(());
    }
    Ok(Mp4Box {
        kind,
        start: offset + header_size,
        end,
    })
}

fn mvhd_value(bytes: &[u8]) -> Result<f64, ()> {
    let version = *bytes.first().ok_or(())?;
    let (timescale_offset, duration_offset, duration_bytes) = match version {
        0 => (12, 16, 4),
        1 => (20, 24, 8),
        _ => return Err(()),
    };
    let timescale = read_u32(bytes, timescale_offset)?;
    if timescale == 0 {
        return Err(());
    }
    let duration = if duration_bytes == 4 {
        u64::from(read_u32(bytes, duration_offset)?)
    } else {
        read_u64(bytes, duration_offset)?
    };
    Ok(duration as f64 / f64::from(timescale))
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, ()> {
    Ok(u32::from_be_bytes(
        bytes
            .get(offset..offset + 4)
            .ok_or(())?
            .try_into()
            .map_err(|_| ())?,
    ))
}

fn read_u64(bytes: &[u8], offset: usize) -> Result<u64, ()> {
    Ok(u64::from_be_bytes(
        bytes
            .get(offset..offset + 8)
            .ok_or(())?
            .try_into()
            .map_err(|_| ())?,
    ))
}

fn catalog_error(message: String) -> ImportError {
    ImportError::SyncCatalog {
        backend: "audio",
        message,
    }
}
