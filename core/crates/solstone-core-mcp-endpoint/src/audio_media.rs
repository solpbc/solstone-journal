// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Bounded audio clip retrieval from one already-admitted transcript segment.

use std::collections::{BTreeMap, HashMap};
use std::ffi::OsStr;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use serde::Deserialize;
use serde_json::{Value, json};
use solstone_core_journal_io::{
    JournalRoot, journal_root::JournalEntryKind, media::MediaDirectory,
};
use solstone_core_observe_audio::{AudioError, decode_clip_wav_from_file};
use solstone_core_processing_record::media::audio_extensions;

use crate::dispatch::DispatchError;
use crate::references::SegmentAddress;
use crate::tools::ToolError;
use crate::tools::transcripts::{AudioClipRequest, AudioEncoding};

/// Maximum bytes read from a segment's `.original-audio.json` playback sidecar.
const MAX_ORIGINAL_SIDECAR_BYTES: usize = 16 * 1024;
const ORIGINAL_AUDIO_SIDECAR_SUFFIX: &str = ".original-audio.json";

type Reply = (Value, Option<Vec<u8>>);

enum AudioTrackSource {
    SegmentFile {
        file_name: String,
    },
    RetainedOriginal {
        parent_rel: PathBuf,
        file_name: String,
        base_offset_ms: u64,
    },
}

#[derive(Deserialize)]
struct OriginalPlaybackSidecar {
    import_id: String,
    relative: PathBuf,
    offset_ms: i64,
}

fn is_audio_extension(name: &str) -> bool {
    let Some(ext) = Path::new(name).extension().and_then(OsStr::to_str) else {
        return false;
    };
    audio_extensions().any(|allowed| ext.eq_ignore_ascii_case(allowed))
}

fn track_stem(name: &str) -> Option<&str> {
    Path::new(name)
        .file_stem()
        .and_then(OsStr::to_str)
        .filter(|stem| !stem.is_empty())
}

fn is_valid_import_id(value: &str) -> bool {
    value.len() == 15
        && value.is_ascii()
        && value.as_bytes()[8] == b'_'
        && jiff::civil::DateTime::strptime("%Y%m%d_%H%M%S", value).is_ok()
}

fn parse_retained_original(
    root: &Arc<JournalRoot>,
    dir: &MediaDirectory,
    sidecar_name: &str,
) -> Option<AudioTrackSource> {
    let data = dir
        .read_file_bounded(OsStr::new(sidecar_name), MAX_ORIGINAL_SIDECAR_BYTES)
        .ok()??;
    let sidecar: OriginalPlaybackSidecar = serde_json::from_slice(&data.bytes).ok()?;
    if !is_valid_import_id(&sidecar.import_id) {
        return None;
    }
    let base_offset_ms = u64::try_from(sidecar.offset_ms).ok()?;
    if sidecar.relative.as_os_str().is_empty()
        || !sidecar
            .relative
            .components()
            .all(|part| matches!(part, Component::Normal(_)))
    {
        return None;
    }
    let full_rel = Path::new("imports")
        .join(&sidecar.import_id)
        .join("originals")
        .join(&sidecar.relative);
    let parent_rel = full_rel.parent()?.to_path_buf();
    let file_name = full_rel.file_name()?.to_str()?.to_owned();
    let orig_dir = MediaDirectory::open(Arc::clone(root), &parent_rel).ok()?;
    // Confirm the retained target exists as a contained regular file before
    // advertising it as an available track.
    orig_dir
        .with_read_file(OsStr::new(&file_name), |_| ())
        .ok()??;
    Some(AudioTrackSource::RetainedOriginal {
        parent_rel,
        file_name,
        base_offset_ms,
    })
}

fn discover_tracks(
    root: &Arc<JournalRoot>,
    dir: &MediaDirectory,
) -> Result<BTreeMap<String, AudioTrackSource>, DispatchError> {
    let entries = dir
        .entries(4096)
        .ok()
        .flatten()
        .ok_or(DispatchError::Tool(ToolError::FileUnreadable))?;

    let mut direct_files: Vec<String> = Vec::new();
    let mut sidecar_files: Vec<(String, String)> = Vec::new();
    for entry in &entries {
        if entry.kind != JournalEntryKind::RegularFile {
            continue;
        }
        let Some(name) = entry.name.to_str() else {
            continue;
        };
        if is_audio_extension(name) {
            direct_files.push(name.to_owned());
        } else if let Some(stem) = name.strip_suffix(ORIGINAL_AUDIO_SIDECAR_SUFFIX)
            && !stem.is_empty()
        {
            sidecar_files.push((stem.to_owned(), name.to_owned()));
        }
    }

    let mut stem_counts: HashMap<String, usize> = HashMap::new();
    for name in &direct_files {
        if let Some(stem) = track_stem(name) {
            *stem_counts.entry(stem.to_owned()).or_insert(0) += 1;
        }
    }

    let mut tracks = BTreeMap::new();
    for name in direct_files {
        let key = match track_stem(&name) {
            Some(stem) if stem_counts.get(stem).copied().unwrap_or(0) == 1 => stem.to_owned(),
            _ => name.clone(),
        };
        tracks.insert(key, AudioTrackSource::SegmentFile { file_name: name });
    }

    for (stem, sidecar_name) in sidecar_files {
        if stem_counts.contains_key(&stem) || tracks.contains_key(&stem) {
            continue;
        }
        if let Some(source) = parse_retained_original(root, dir, &sidecar_name) {
            tracks.insert(stem, source);
        }
    }

    Ok(tracks)
}

fn open_segment(
    journal_root: &Path,
    segment: &Path,
) -> Option<(Arc<JournalRoot>, Arc<MediaDirectory>)> {
    let rel = segment.strip_prefix(journal_root).ok()?;
    let root = Arc::new(JournalRoot::open(journal_root).ok()?);
    let dir = MediaDirectory::open(Arc::clone(&root), rel).ok()?;
    Some((root, dir))
}

/// Whether this already-admitted segment currently holds at least one readable
/// audio file or retained original on disk. Callers must gate this behind
/// `AdmittedCategory::AudioMedia` and the segment release checks.
pub(crate) fn has_audio(journal_root: &Path, segment: &Path) -> bool {
    let Some((root, dir)) = open_segment(journal_root, segment) else {
        return false;
    };
    discover_tracks(&root, &dir).is_ok_and(|tracks| !tracks.is_empty())
}

fn segment_duration_ms(address: &SegmentAddress) -> Option<u64> {
    address
        .segment
        .split('_')
        .nth(1)
        .and_then(|seconds| seconds.parse::<u64>().ok())
        .filter(|seconds| *seconds > 0)
        .map(|seconds| seconds.saturating_mul(1_000))
}

fn missing() -> Reply {
    (
        json!({
            "state": "not_kept",
            "message": "no audio recording is kept for this segment in this journal",
        }),
        None,
    )
}

fn suggested_filename(address: &SegmentAddress, track: &str, start_ms: u64) -> String {
    let clock = address
        .segment
        .split('_')
        .next()
        .filter(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
        .unwrap_or("000000");
    let safe_track: String = track
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-'))
        .take(64)
        .collect();
    let safe_track = if safe_track.is_empty() {
        "audio"
    } else {
        safe_track.as_str()
    };
    if start_ms == 0 {
        format!("{}_{}_{}.wav", address.day, clock, safe_track)
    } else {
        format!(
            "{}_{}_{}_{}ms.wav",
            address.day, clock, safe_track, start_ms
        )
    }
}

fn unreadable(track: &str) -> Reply {
    (
        json!({
            "state": "unreadable",
            "track": track,
            "message": "the journal could not read this audio recording",
        }),
        None,
    )
}

fn empty(track: &str) -> Reply {
    (
        json!({
            "state": "empty",
            "track": track,
            "message": "this recording holds no audio",
        }),
        None,
    )
}

fn out_of_range(track: &str, start_ms: u64, total_duration_ms: Option<u64>) -> Reply {
    let mut info = json!({
        "state": "out_of_range",
        "track": track,
        "start_ms": start_ms,
        "message": "no audio exists at that offset in this recording",
    });
    if let Some(total) = total_duration_ms {
        info["total_duration_ms"] = json!(total);
    }
    (info, None)
}

/// Extract one bounded 16 kHz mono PCM-16 WAV clip from an already-admitted
/// transcript segment.
pub(crate) fn clip(
    journal_root: &Path,
    segment: &Path,
    address: &SegmentAddress,
    request: &AudioClipRequest,
) -> Result<Reply, DispatchError> {
    let not_found = DispatchError::Tool(ToolError::ReferenceNotFound);
    let (root, dir) = open_segment(journal_root, segment).ok_or(not_found)?;
    let tracks = discover_tracks(&root, &dir)?;

    if tracks.is_empty() {
        if request.track.is_some() {
            return Err(not_found);
        }
        return Ok(missing());
    }

    let (selected_name, source) = match request.track.as_deref() {
        Some(wanted) => {
            let source = tracks.get(wanted).ok_or(not_found)?;
            (wanted.to_owned(), source)
        }
        None if tracks.len() > 1 => {
            let names: Vec<&str> = tracks.keys().map(String::as_str).collect();
            return Ok((
                json!({
                    "state": "choose_track",
                    "message": "pass one of these tracks as audio_track",
                    "tracks": names,
                }),
                None,
            ));
        }
        None => {
            let (name, source) = tracks.iter().next().ok_or(not_found)?;
            (name.clone(), source)
        }
    };

    let (decode_result, base_offset_ms, segment_cap_ms) = match source {
        AudioTrackSource::SegmentFile { file_name } => {
            let result = dir.with_read_file(OsStr::new(file_name), |file| {
                decode_clip_wav_from_file(file, request.start_ms, request.duration_ms, None)
            });
            (result, 0_u64, None)
        }
        AudioTrackSource::RetainedOriginal {
            parent_rel,
            file_name,
            base_offset_ms,
        } => {
            let Some(segment_ms) = segment_duration_ms(address) else {
                return Ok(unreadable(&selected_name));
            };
            if request.start_ms >= segment_ms {
                return Ok(out_of_range(
                    &selected_name,
                    request.start_ms,
                    Some(segment_ms),
                ));
            }
            let Ok(orig_dir) = MediaDirectory::open(Arc::clone(&root), parent_rel) else {
                return Ok(missing());
            };
            let file_start_ms = base_offset_ms.saturating_add(request.start_ms);
            let max_end_ms = Some(base_offset_ms.saturating_add(segment_ms));
            let result = orig_dir.with_read_file(OsStr::new(file_name), |file| {
                decode_clip_wav_from_file(file, file_start_ms, request.duration_ms, max_end_ms)
            });
            (result, *base_offset_ms, Some(segment_ms))
        }
    };

    let reply = match decode_result {
        Ok(Some(Ok(Some(clip)))) => {
            let relative_total_ms = clip.total_duration_ms.map(|total| {
                let rel = total.saturating_sub(base_offset_ms);
                segment_cap_ms.map_or(rel, |cap| rel.min(cap))
            });
            let filename = suggested_filename(address, &selected_name, request.start_ms);
            let mut info = json!({
                "state": "attached",
                "track": selected_name,
                "filename": filename,
                "mime_type": "audio/wav",
                "sample_rate_hz": clip.sample_rate_hz,
                "channels": clip.channels,
                "start_ms": request.start_ms,
                "duration_ms": clip.duration_ms,
                "has_more": clip.has_more,
                "bytes": clip.wav.len(),
            });
            if let Some(total) = relative_total_ms {
                info["total_duration_ms"] = json!(total);
            }
            if clip.has_more {
                info["next_start_ms"] = json!(request.start_ms.saturating_add(clip.duration_ms));
            }
            match request.encoding {
                AudioEncoding::Content => {
                    info["audio_content_index"] = json!(1);
                    (info, Some(clip.wav))
                }
                AudioEncoding::Base64 => {
                    use base64::Engine as _;
                    info["data_base64"] =
                        Value::String(base64::engine::general_purpose::STANDARD.encode(&clip.wav));
                    (info, None)
                }
            }
        }
        Ok(Some(Ok(None))) => out_of_range(&selected_name, request.start_ms, segment_cap_ms),
        Ok(None) => missing(),
        Ok(Some(Err(AudioError::EmptyInput { .. } | AudioError::NoDecodedAudio { .. }))) => {
            empty(&selected_name)
        }
        Ok(Some(Err(_))) | Err(_) => unreadable(&selected_name),
    };

    Ok(reply)
}
