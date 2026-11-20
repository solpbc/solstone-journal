// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! A picture from one already-admitted screen entry or segment.
use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::path::Path;
use std::sync::Arc;

use crate::dispatch::DispatchError;
use crate::photo_picture::{Picture, bounded_picture, encode_picture};
use crate::references::SegmentAddress;
use crate::tools::ToolError;
use serde_json::{Value, json};
use solstone_core_format::screenshots::{SCREENSHOT_STEM, screen_media_stem};
use solstone_core_journal_io::{
    JournalRoot, journal_root::JournalEntryKind, media::MediaDirectory,
};

type Reply = (Value, Option<Vec<u8>>);

pub(crate) fn import_folder(path: &str) -> Option<&str> {
    let folder = path
        .strip_prefix("chronicle/")
        .unwrap_or(path)
        .strip_suffix("/screenshot_transcript.md")?;
    let mut parts = folder.split('/');
    let day = parts.next()?;
    if !solstone_core_format::segment::is_date_key(day) || parts.next()? != "import.image" {
        return None;
    }
    let segment = parts.next()?;
    (solstone_core_format::segment::segment_parse(segment).is_some() && parts.next().is_none())
        .then_some(folder)
}

fn missing(message: &str) -> Reply {
    (json!({"state":"not_kept","message":message}), None)
}

fn unreadable() -> Reply {
    (
        json!({"state":"unreadable","message":"the journal could not read this picture"}),
        None,
    )
}

fn attached(picture: Picture) -> Reply {
    (
        json!({"state":"attached","width":picture.width,"height":picture.height}),
        Some(picture.jpeg),
    )
}

fn open(root: &Path, relative: &Path) -> Option<Arc<MediaDirectory>> {
    MediaDirectory::open(Arc::new(JournalRoot::open(root).ok()?), relative).ok()
}

fn still(dir: &MediaDirectory, names: impl IntoIterator<Item = String>) -> Reply {
    for name in names {
        match dir.read_file_bounded(OsStr::new(&name), 64 * 1024 * 1024) {
            Ok(Some(file)) => {
                return bounded_picture(&file.bytes)
                    .map(attached)
                    .unwrap_or_else(unreadable);
            }
            Ok(None) => (),
            Err(_) => return unreadable(),
        }
    }
    missing("the original screenshot is no longer kept in this journal")
}

pub(crate) fn imported(root: &Path, folder: &str) -> Reply {
    let Some(dir) = open(root, &Path::new("chronicle").join(folder)) else {
        return unreadable();
    };
    still(
        &dir,
        ["png", "jpg", "jpeg", "webp", "gif"].map(|ext| format!("original.screenshot.{ext}")),
    )
}

// Return only the selected display's description as prose. Headers, processing
// diagnostics, and other metadata from the sidecar are never agent results.
fn description(bytes: &[u8], offset: u32) -> (Option<(f64, String)>, Option<String>) {
    let mut best = None;
    let mut raw = None;
    for (i, line) in bytes.split(|b| *b == b'\n').enumerate() {
        let Ok(row) = serde_json::from_slice::<Value>(line) else {
            continue;
        };
        if i == 0 {
            raw = row.get("raw").and_then(Value::as_str).map(str::to_owned);
        }
        if let Some(seconds) = row["timestamp"].as_f64()
            && seconds >= 0.0
            && seconds * 1000.0 <= f64::from(offset)
            && let Some(words) = row["analysis"]["visual_description"].as_str()
            && !words.is_empty()
            && best
                .as_ref()
                .is_none_or(|(previous, _)| seconds >= *previous)
        {
            best = Some((seconds, words.chars().take(8192).collect::<String>()));
        }
    }
    (best, raw)
}

pub(crate) fn moment(
    root: &Path,
    segment: &Path,
    address: &SegmentAddress,
    offset: u32,
    display: Option<&str>,
) -> Result<Reply, DispatchError> {
    let not_found = DispatchError::Tool(ToolError::ReferenceNotFound);
    let length = address
        .segment
        .split_once('_')
        .and_then(|(_, s)| s.parse::<u64>().ok())
        .ok_or(not_found)?;
    if u64::from(offset) >= length.saturating_mul(1000) {
        return Err(DispatchError::InvalidInput);
    }
    let dir = open(root, segment.strip_prefix(root).map_err(|_| not_found)?).ok_or(not_found)?;
    let origin = solstone_core_journal_io::paths::segment_origin(segment);
    let entries = dir
        .entries(4096)
        .ok()
        .flatten()
        .ok_or(DispatchError::Tool(ToolError::FileUnreadable))?;
    let names: BTreeSet<_> = entries
        .iter()
        .filter(|e| e.kind == JournalEntryKind::RegularFile)
        .filter_map(|e| e.name.to_str())
        .collect();
    let displays: BTreeSet<_> = names
        .iter()
        .filter_map(|name| screen_media_stem(&origin, name))
        .collect();
    if displays.is_empty() {
        let text = segment_description(&dir).ok_or(not_found)?;
        if display.is_some() {
            return Err(not_found);
        }
        let (mut info, jpeg) =
            missing("the original screen image or recording is no longer kept in this journal");
        info["segment_description"] = json!(text);
        return Ok((info, jpeg));
    }
    let selected = match display {
        Some(name) if displays.contains(name) => name,
        Some(_) => return Err(not_found),
        None if displays.len() > 1 => {
            return Ok((
                json!({"state":"choose_display","message":"pass one of these displays with the same offset_ms","displays":displays}),
                None,
            ));
        }
        None => displays.first().copied().ok_or(not_found)?,
    };
    let sidecar_name = format!("{selected}.jsonl");
    let sidecar = if screen_media_stem(&origin, &sidecar_name) == Some(selected) {
        dir.read_prefix(OsStr::new(&sidecar_name), 16 * 1024 * 1024)
            .map_err(|_| DispatchError::Tool(ToolError::FileUnreadable))?
    } else {
        None
    };
    let (words, raw) = sidecar
        .as_ref()
        .map(|data| description(&data.bytes, offset))
        .unwrap_or_default();
    let (mut reply, jpeg) = if selected == SCREENSHOT_STEM
        && solstone_core_segment::is_screenshots_stream(root, &address.stream)
    {
        still(
            &dir,
            solstone_core_format::screenshots::SCREENSHOT_EXTENSIONS
                .map(|ext| format!("{selected}.{ext}")),
        )
    } else {
        // Some screen headers name a WebM whose basename differs from the
        // sidecar. Only a bare file in this admitted directory may match it.
        let name = ["webm", "mp4", "mov"]
            .into_iter()
            .map(|ext| format!("{selected}.{ext}"))
            .find(|name| {
                names.contains(name.as_str()) && screen_media_stem(&origin, name) == Some(selected)
            })
            .or_else(|| {
                raw.filter(|name| {
                    name.ends_with(".webm")
                        && !name.contains(['/', '\\'])
                        && !name.trim_end_matches(".webm").ends_with("_screen")
                        && screen_media_stem(&origin, name).is_some()
                        && names.contains(name.as_str())
                })
            });
        name.map_or_else(
            || missing("the original recording is no longer kept in this journal"),
            |name| recording(&dir, &name, offset),
        )
    };
    reply["display"] = json!(selected);
    reply["offset_ms"] = json!(offset);
    if let Some((seconds, words)) = words {
        reply["description"] = json!(words);
        reply["description_offset_ms"] = json!((seconds * 1000.0).round() as u64);
    } else if let Some(text) = segment_description(&dir) {
        reply["segment_description"] = json!(text);
    } else {
        reply["description_state"] = json!("unavailable");
    }
    Ok((reply, jpeg))
}

fn recording(dir: &MediaDirectory, name: &str, offset: u32) -> Reply {
    match dir.with_read_file(OsStr::new(name), |file| {
        solstone_core_grab::frame_at(file, offset)
    }) {
        Ok(Some(Ok(Some((at, rgb))))) => {
            let (mut info, image) = encode_picture(image::DynamicImage::ImageRgb8(rgb))
                .map(attached)
                .unwrap_or_else(unreadable);
            info["frame_offset_ms"] = json!(at);
            (info, image)
        }
        Ok(Some(Ok(None))) => (
            json!({"state":"no_frame","message":"there is no frame at that time in this recording"}),
            None,
        ),
        Ok(None) => missing("the original recording is no longer kept in this journal"),
        Ok(Some(Err(_))) | Err(_) => unreadable(),
    }
}

fn segment_description(dir: &MediaDirectory) -> Option<String> {
    let talents = dir.open_child(OsStr::new("talents")).ok()??;
    ["screen.md", "screen_text.md"]
        .into_iter()
        .find_map(|file| {
            let data = talents.read_prefix(OsStr::new(file), 32 * 1024).ok()??;
            Some(String::from_utf8_lossy(&data.bytes).into_owned())
        })
}
