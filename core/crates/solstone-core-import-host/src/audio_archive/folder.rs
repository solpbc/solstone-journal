// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Folder scanning and filename evidence parsing for audio archives.

use std::fs;
use std::path::Path;

use solstone_core_import::audio_archive::{
    AudioArchiveError, AudioArchiveMemberInput, MemberStartEvidence, MemberTimeAuthority,
};

use super::manifest::probe_member_file;

pub fn scan_folder_members(
    folder_path: &Path,
    source_zone: &str,
) -> Result<Vec<AudioArchiveMemberInput>, AudioArchiveError> {
    let mut files = Vec::new();
    collect_files_recursive(folder_path, &mut files).map_err(AudioArchiveError::Io)?;
    files.sort();

    let mut members = Vec::with_capacity(files.len());
    for file_path in files {
        let relative_name = match file_path.strip_prefix(folder_path) {
            Ok(rel) => rel.to_string_lossy().replace('\\', "/"),
            Err(_) => file_path.to_string_lossy().into_owned(),
        };

        let file_name = file_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();

        let civil_time = parse_limitless_civil_time(file_name);
        let start = match civil_time {
            Some(civil) => MemberStartEvidence {
                instant: None,
                local_civil: Some(civil),
                source_zone: Some(source_zone.to_owned()),
                authority: MemberTimeAuthority::Filename,
            },
            None => MemberStartEvidence {
                instant: None,
                local_civil: None,
                source_zone: None,
                authority: MemberTimeAuthority::Filename,
            },
        };

        if !solstone_core_import::work_continues() {
            return Err(AudioArchiveError::Other("import stopped".to_owned()));
        }
        let (byte_len, sha256, duration) = probe_member_file(&file_path);

        members.push(AudioArchiveMemberInput {
            original_relative_name: relative_name,
            byte_len,
            sha256,
            duration,
            start,
            run_note: None,
        });
    }

    Ok(members)
}

fn collect_files_recursive(
    dir: &Path,
    collected: &mut Vec<std::path::PathBuf>,
) -> std::io::Result<()> {
    if !dir.is_dir() {
        return Ok(());
    }
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let kind = entry.file_type()?;
        if kind.is_dir() {
            collect_files_recursive(&path, collected)?;
        } else if kind.is_file() {
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            // Accept audio files; ignore hidden files and metadata files like .json
            if !path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with('.'))
                && matches!(
                    ext.as_str(),
                    "opus" | "ogg" | "flac" | "wav" | "mp3" | "m4a" | "aac"
                )
            {
                collected.push(path);
            }
        }
    }
    Ok(())
}

pub fn parse_limitless_civil_time(filename: &str) -> Option<String> {
    let start = filename.find("limitless_pendant_")?;
    let rest = &filename[start + "limitless_pendant_".len()..];
    let to_pos = rest.find("_to_")?;
    let stamp_part = &rest[..to_pos]; // e.g. "2026-04-01T12-00-00"
    let (date_part, time_part) = stamp_part.split_once('T')?;

    let date_parts: Vec<&str> = date_part.split('-').collect();
    if date_parts.len() != 3 {
        return None;
    }
    let year: u32 = date_parts[0].parse().ok()?;
    let month: u32 = date_parts[1].parse().ok()?;
    let day: u32 = date_parts[2].parse().ok()?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) || year < 1900 {
        return None;
    }

    let time_parts: Vec<&str> = time_part.split('-').collect();
    if time_parts.len() != 3 {
        return None;
    }
    let hour: u32 = time_parts[0].parse().ok()?;
    let min: u32 = time_parts[1].parse().ok()?;
    let sec: u32 = time_parts[2].parse().ok()?;
    if hour > 23 || min > 59 || sec > 59 {
        return None;
    }

    Some(format!(
        "{year:04}-{month:02}-{day:02}T{hour:02}:{min:02}:{sec:02}"
    ))
}
