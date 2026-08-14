// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Default)]
pub(crate) struct SegmentMedia {
    pub(crate) audio_file: Option<String>,
    pub(crate) video_files: BTreeMap<String, String>,
    pub(crate) image_files: BTreeMap<String, String>,
    pub(crate) media_sizes: BTreeMap<String, u64>,
    pub(crate) has_raw_present: BTreeMap<String, bool>,
    pub(crate) has_raw_reference: BTreeMap<String, bool>,
    pub(crate) has_raw_file: BTreeMap<String, bool>,
    counted: BTreeSet<PathBuf>,
}

pub(crate) fn discover(dir: &Path, markdown_only: bool) -> SegmentMedia {
    let mut media = SegmentMedia {
        media_sizes: BTreeMap::from([("audio".into(), 0), ("screen".into(), 0)]),
        has_raw_present: BTreeMap::from([("audio".into(), false), ("screen".into(), false)]),
        has_raw_reference: BTreeMap::from([("audio".into(), false), ("screen".into(), false)]),
        has_raw_file: BTreeMap::from([("audio".into(), false), ("screen".into(), false)]),
        ..Default::default()
    };
    if markdown_only {
        return media;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return media;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        if let Some(modality) = modality(&path) {
            media.has_raw_present.insert(modality.into(), true);
            media.count(modality, &path);
        }
    }
    media
}

impl SegmentMedia {
    pub(crate) fn register_audio(
        &mut self,
        day: &str,
        stream: &str,
        key: &str,
        dir: &Path,
        raw: &str,
    ) {
        if !is_audio(raw) {
            return;
        }
        self.has_raw_reference.insert("audio".into(), true);
        let path = dir.join(raw);
        if !path.is_file() {
            return;
        }
        self.has_raw_present.insert("audio".into(), true);
        self.has_raw_file.insert("audio".into(), true);
        self.audio_file = Some(url(day, stream, key, raw));
        self.count("audio", &path);
    }
    pub(crate) fn register_screen(
        &mut self,
        day: &str,
        stream: &str,
        key: &str,
        dir: &Path,
        raw: &str,
        source_file: &str,
    ) -> Option<&'static str> {
        let kind = screen_kind(raw)?;
        self.has_raw_reference.insert("screen".into(), true);
        let path = dir.join(raw);
        if !path.is_file() {
            return None;
        }
        self.has_raw_present.insert("screen".into(), true);
        self.has_raw_file.insert("screen".into(), true);
        let value = url(day, stream, key, raw);
        if kind == "video" {
            self.video_files.insert(source_file.into(), value);
        } else {
            self.image_files.insert(raw.into(), value);
        }
        self.count("screen", &path);
        Some(kind)
    }
    pub(crate) fn purged(&self, modality: &str) -> bool {
        self.has_raw_reference[modality] && !self.has_raw_file[modality]
    }
    fn count(&mut self, modality: &str, path: &Path) {
        let resolved = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        if self.counted.insert(resolved) {
            *self.media_sizes.entry(modality.into()).or_default() +=
                path.metadata().map(|metadata| metadata.len()).unwrap_or(0);
        }
    }
}

pub(crate) fn markdown_only(dir: &Path, stream: &str) -> bool {
    if !stream.starts_with("import.") {
        return false;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return false;
    };
    let names = entries
        .flatten()
        .filter_map(|entry| entry.file_name().into_string().ok())
        .collect::<Vec<_>>();
    names
        .iter()
        .any(|name| name == "imported.md" || name.ends_with("_transcript.md"))
        && !names.iter().any(|name| {
            name.ends_with("audio.jsonl")
                || name.ends_with("screen.jsonl")
                || name.ends_with("_transcript.jsonl")
        })
}
pub(crate) fn markdown_files(dir: &Path) -> Vec<PathBuf> {
    let mut values = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file()
                && path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name == "imported.md" || name.ends_with("_transcript.md"))
        })
        .collect::<Vec<_>>();
    values.sort();
    values
}
fn url(day: &str, stream: &str, key: &str, raw: &str) -> String {
    format!("/app/transcripts/api/serve_file/{day}/{stream}/{key}/{raw}")
}
fn modality(path: &Path) -> Option<&'static str> {
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    if is_audio_extension(&extension) {
        Some("audio")
    } else if screen_kind_extension(&extension).is_some() {
        Some("screen")
    } else {
        None
    }
}
fn is_audio(raw: &str) -> bool {
    raw.rsplit('.')
        .next()
        .is_some_and(|value| is_audio_extension(&value.to_ascii_lowercase()))
}
fn is_audio_extension(value: &str) -> bool {
    matches!(value, "flac" | "opus" | "ogg" | "m4a" | "mp3" | "wav")
}
fn screen_kind(raw: &str) -> Option<&'static str> {
    screen_kind_extension(&raw.rsplit('.').next()?.to_ascii_lowercase())
}
fn screen_kind_extension(value: &str) -> Option<&'static str> {
    match value {
        "webm" | "mp4" | "mov" => Some("video"),
        "png" | "jpg" | "jpeg" | "heic" | "heif" | "gif" | "webp" | "tiff" => Some("image"),
        _ => None,
    }
}
