// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use solstone_core_processing_record::{MediaKind, media_kind};

#[derive(Default)]
pub(crate) struct SegmentMedia {
    pub(crate) audio_file: Option<String>,
    pub(crate) video_files: BTreeMap<String, String>,
    pub(crate) image_files: BTreeMap<String, String>,
    pub(crate) media_sizes: BTreeMap<String, u64>,
    pub(crate) has_raw_present: BTreeMap<String, bool>,
    pub(crate) has_raw_reference: BTreeMap<String, bool>,
    pub(crate) has_raw_file: BTreeMap<String, bool>,
    pub(crate) missing_referenced_files: BTreeMap<String, BTreeSet<String>>,
    counted: BTreeSet<PathBuf>,
}

pub(crate) fn discover(
    dir: &Path,
    markdown_only: bool,
    unclaimed_images: &BTreeSet<String>,
) -> SegmentMedia {
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
        if let Some(modality) = modality(&path, unclaimed_images) {
            media.has_raw_present.insert(modality.into(), true);
            media.count(modality, &path);
        }
    }
    media
}

impl SegmentMedia {
    pub(crate) fn register_image_url(&mut self, day: &str, stream: &str, key: &str, raw: &str) {
        self.image_files
            .insert(raw.into(), url(day, stream, key, raw));
    }
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
            self.missing_referenced_files
                .entry("audio".into())
                .or_default()
                .insert(raw.to_owned());
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
            self.missing_referenced_files
                .entry("screen".into())
                .or_default()
                .insert(raw.to_owned());
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
    pub(crate) fn media_removal(&self, dir: &Path) -> crate::media_removal::MediaRemoval {
        let purged = BTreeMap::from([
            ("audio".to_owned(), self.purged("audio")),
            ("screen".to_owned(), self.purged("screen")),
        ]);
        crate::media_removal::compute_media_removal(dir, &purged, &self.missing_referenced_files)
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
    format!(
        "/app/transcripts/api/serve_file/{}/{raw}",
        segment_rel(day, stream, key)
    )
}
/// A segment's path under `chronicle/`. A direct-layout segment sits under its
/// day, and its stream is the `_default` sentinel, never a folder.
pub(crate) fn segment_rel(day: &str, stream: &str, key: &str) -> String {
    if stream == solstone_core_journal_io::DEFAULT_STREAM {
        format!("{day}/{key}")
    } else {
        format!("{day}/{stream}/{key}")
    }
}

pub(crate) fn physical_segment_dir(
    journal_root: &Path,
    day: &str,
    stream: &str,
    key: &str,
) -> PathBuf {
    journal_root
        .join("chronicle")
        .join(segment_rel(day, stream, key))
}

pub(crate) fn physical_staged_dir(
    journal_root: &Path,
    day: &str,
    stream: &str,
    key: &str,
) -> PathBuf {
    let mut path = physical_segment_dir(journal_root, day, stream, key);
    path.pop();
    path.push(solstone_core_retention::staged_name(key));
    path
}
pub(crate) fn modality(path: &Path, unclaimed_images: &BTreeSet<String>) -> Option<&'static str> {
    let extension = path.extension()?.to_str()?;
    match media_kind(extension)? {
        MediaKind::Audio => Some("audio"),
        MediaKind::Video => Some("screen"),
        MediaKind::Image => {
            let name = path.file_name()?.to_str()?;
            if unclaimed_images.contains(name) {
                None
            } else {
                Some("screen")
            }
        }
    }
}
fn is_audio(raw: &str) -> bool {
    raw.rsplit('.').next().and_then(media_kind) == Some(MediaKind::Audio)
}
fn screen_kind(raw: &str) -> Option<&'static str> {
    match media_kind(raw.rsplit('.').next()?)? {
        MediaKind::Video => Some("video"),
        MediaKind::Image => Some("image"),
        MediaKind::Audio => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use solstone_core_retention::layout;

    #[test]
    fn path_builders_agree_with_retention_layout() {
        let day = "20260805";
        let key = "070000_17";
        let named_stream = "field.audio";
        let default_stream = solstone_core_journal_io::DEFAULT_STREAM;
        let root = Path::new("/mock/journal");

        // Direct stream agreement
        let direct_rel = format!("chronicle/{}", segment_rel(day, default_stream, key));
        assert_eq!(direct_rel, layout::segment_rel(day, default_stream, key));
        assert_eq!(
            physical_segment_dir(root, day, default_stream, key),
            root.join(layout::segment_rel(day, default_stream, key))
        );
        assert_eq!(
            physical_staged_dir(root, day, default_stream, key),
            root.join(format!(
                "{}/{}",
                layout::stream_rel(day, default_stream),
                solstone_core_retention::staged_name(key)
            ))
        );

        // Named stream agreement
        let named_rel = format!("chronicle/{}", segment_rel(day, named_stream, key));
        assert_eq!(named_rel, layout::segment_rel(day, named_stream, key));
        assert_eq!(
            physical_segment_dir(root, day, named_stream, key),
            root.join(layout::segment_rel(day, named_stream, key))
        );
        assert_eq!(
            physical_staged_dir(root, day, named_stream, key),
            root.join(format!(
                "{}/{}",
                layout::stream_rel(day, named_stream),
                solstone_core_retention::staged_name(key)
            ))
        );
    }
}
