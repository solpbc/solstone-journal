// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Strict retained-input admission. Normal and legacy fingerprint policy stays
//! in the parent; this collector supplies only complete, uniquely resolved input.

use std::path::{Component, Path};

use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
#[cfg(unix)]
use solstone_core_journal_io::{
    FlatDirectory as Directory, list_flat_directory as list, open_flat_directory_bound as open,
};
use solstone_core_journal_io::{JournalEntryKind, JournalRoot, read_relative_file_bounded};
#[cfg(windows)]
use solstone_core_journal_io::{
    WindowsFlatDirectory as Directory, list_windows_flat_directory as list,
    open_windows_flat_directory_bound as open,
};

/// The fingerprint and Sense values consume the same admitted byte snapshots.
pub struct ActivityInputSnapshot {
    pub hash: String,
    pub sense: Vec<Value>,
}

fn component(value: &str) -> Result<(), String> {
    if value.is_empty()
        || value.contains(['/', '\\', '\0'])
        || !matches!(
            Path::new(value).components().next(),
            Some(Component::Normal(_))
        )
    {
        return Err("retained activity source identity is malformed".into());
    }
    Ok(())
}

fn day_directory(root: &JournalRoot, day: &str) -> Result<Directory, String> {
    let chronicle = open(root, "chronicle".as_ref(), root.canonical_path())
        .map_err(|e| e.to_string())?
        .ok_or("retained activity source is unavailable")?;
    open(
        &chronicle,
        day.as_ref(),
        &root.canonical_path().join("chronicle"),
    )
    .map_err(|e| e.to_string())?
    .ok_or("retained activity source is unavailable".into())
}

/// Bound enumeration, read regular descendants without following aliases, and
/// retain the existing layout-aware fingerprint's exact identity construction.
pub fn activity_input_bounded(
    journal: &Path,
    day: &str,
    record: &Map<String, Value>,
    checkpoint: impl Fn() -> Result<(), String>,
) -> Result<ActivityInputSnapshot, String> {
    chrono::NaiveDate::parse_from_str(day, "%Y%m%d").map_err(|e| e.to_string())?;
    let spans = record
        .get("segments")
        .and_then(Value::as_array)
        .filter(|spans| !spans.is_empty() && spans.len() <= 1024)
        .ok_or("retained activity span is unavailable or exceeds recovery budget")?
        .iter()
        .map(|value| {
            let segment = value
                .as_str()
                .ok_or("retained activity span is malformed")?;
            component(segment)?;
            Ok(segment.to_owned())
        })
        .collect::<Result<Vec<_>, String>>()?;
    let stream = match record.get("stream") {
        None | Some(Value::Null) => None,
        Some(Value::String(stream)) => {
            component(stream)?;
            Some(stream.as_str())
        }
        _ => return Err("retained activity stream is malformed".into()),
    };
    checkpoint()?;
    let root = JournalRoot::open(journal).map_err(|e| e.to_string())?;
    let directory = day_directory(&root, day)?;
    let entries = list(&directory, 1024)
        .map_err(|e| e.to_string())?
        .ok_or("retained activity source exceeds recovery entry budget")?;
    let relative_day = Path::new("chronicle").join(day);
    let mut inputs = Vec::new();
    let mut senses = Vec::new();
    let mut observations = Vec::new();
    let mut bytes_left = 16 * 1024 * 1024;
    for segment in &spans {
        let mut selected = Vec::new();
        for entry in &entries {
            checkpoint()?;
            let name = entry
                .name
                .to_str()
                .ok_or("retained activity source identity is malformed")?;
            let direct = name == segment;
            if direct {
                if stream.is_some_and(|s| s != solstone_core_journal_io::DEFAULT_STREAM) {
                    continue;
                }
            } else if stream.is_some_and(|s| s != name) {
                continue;
            }
            match entry.kind {
                JournalEntryKind::RegularFile => continue,
                JournalEntryKind::Directory => {}
                _ => return Err("retained activity source alias or special file refused".into()),
            }
            let child = open(
                &directory,
                &entry.name,
                &root.canonical_path().join(&relative_day),
            )
            .map_err(|e| e.to_string())?
            .ok_or("retained activity source disappeared")?;
            if !child.matches_entry(entry) {
                return Err("retained activity source changed during admission".into());
            }
            let relative = if direct {
                relative_day.join(&entry.name)
            } else {
                if open(
                    &child,
                    segment.as_ref(),
                    &root.canonical_path().join(&relative_day).join(&entry.name),
                )
                .map_err(|e| e.to_string())?
                .is_none()
                {
                    continue;
                }
                relative_day.join(&entry.name).join(segment)
            };
            selected.push(relative.join("talents/sense.json"));
            if selected.len() > 1 {
                return Err("retained activity source is ambiguous".into());
            }
        }
        let relative = selected
            .pop()
            .ok_or("retained activity source is unavailable")?;
        checkpoint()?;
        let observed =
            read_relative_file_bounded(&root, &relative, bytes_left.min(4 * 1024 * 1024))
                .map_err(|e| e.to_string())?
                .ok_or("retained activity Sense is unavailable")?;
        let value = serde_json::from_slice(&observed.bytes)
            .map_err(|_| "retained activity Sense is malformed")?;
        bytes_left -= observed.bytes.len();
        senses.push(value);
        inputs.push(serde_json::json!({"segment":segment,"sense":[{
            "path":relative.display().to_string(),"sha256":format!("{:x}", Sha256::digest(&observed.bytes)),"size":observed.bytes.len()
        }]}));
        observations.push((relative, observed));
    }
    let hash = super::activity_identity_hash(record, &spans, inputs)
        .ok_or("cannot fingerprint retained activity inputs")?;
    for (relative, before) in observations {
        checkpoint()?;
        let after = read_relative_file_bounded(&root, &relative, 4 * 1024 * 1024)
            .map_err(|e| e.to_string())?
            .ok_or("retained activity Sense is unavailable")?;
        if before != after {
            return Err("retained activity input changed during admission".into());
        }
    }
    checkpoint()?;
    if list(&directory, 1024).map_err(|e| e.to_string())?.as_ref() != Some(&entries)
        || identity(&day_directory(&root, day)?) != identity(&directory)
    {
        return Err("retained activity source changed during admission".into());
    }
    root.revalidate().map_err(|e| e.to_string())?;
    Ok(ActivityInputSnapshot {
        hash,
        sense: senses,
    })
}

fn identity(directory: &Directory) -> solstone_core_journal_io::ObjectIdentity {
    #[cfg(unix)]
    {
        directory.identity()
    }
    #[cfg(windows)]
    {
        directory.object_identity()
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    #[cfg(feature = "full-tests")]
    use std::cell::Cell;
    use std::fs;

    const DAY: &str = "20260101";
    const FIRST: &[u8] = b"{\"density\":\"high\",\"content_type\":\"text\"}\n";
    const SECOND: &[u8] = b"{\"density\":\"low\",\"content_type\":\"text\"}\n";

    fn fixture(root: &Path, layout: &str) -> (Map<String, Value>, std::path::PathBuf) {
        let stream = if layout == "direct" {
            "_default"
        } else {
            "recording"
        };
        let segments = if layout == "multi" {
            vec!["120000_300", "120500_300"]
        } else {
            vec!["120000_300"]
        };
        let mut first = None;
        for (index, segment) in segments.iter().enumerate() {
            let day = root.join("chronicle").join(DAY);
            let path = if layout == "direct" {
                day.join(segment)
            } else {
                day.join(stream).join(segment)
            }
            .join("talents/sense.json");
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, if index == 0 { FIRST } else { SECOND }).unwrap();
            first.get_or_insert(path);
        }
        (serde_json::json!({"id":"work_120000","activity":"work","facet":"work","stream":stream,"segments":segments}).as_object().unwrap().clone(), first.unwrap())
    }

    #[test]
    fn bounded_layouts_preserve_fixed_native_fingerprints() {
        // Fixed digests independently constructed from the pre-repair identity
        // schema, exact source bytes, relative layout paths and canonical JSON.
        for (layout, expected) in [
            (
                "direct",
                "ba0a1eeb1eb4fc0bc451f8376d3732281ffb95449d39a01f254107961fbe29f9",
            ),
            (
                "named",
                "e52877957c052a4032c9d0fc2373554a961ffec05c09a2429aa1a07c32463949",
            ),
            (
                "multi",
                "e9f1e5d09c222b2697cc792f0f7cd2ef74277458ff78e6c105902c07413c5100",
            ),
        ] {
            let root = tempfile::tempdir().unwrap();
            let (record, path) = fixture(root.path(), layout);
            assert_eq!(
                super::super::activity_hash(root.path(), DAY, &record).as_deref(),
                Some(expected)
            );
            let bounded = activity_input_bounded(root.path(), DAY, &record, || Ok(())).unwrap();
            assert_eq!(bounded.hash, expected);
            assert_eq!(
                bounded.sense.len(),
                record["segments"].as_array().unwrap().len()
            );
            // Raw transcripts are outside this Sense-only fingerprint. Never
            // traverse them to decide whether retained Sense is available.
            std::os::unix::fs::symlink(
                "/absent-retained-fixture",
                path.parent().unwrap().parent().unwrap().join("audio.jsonl"),
            )
            .unwrap();
            assert_eq!(
                activity_input_bounded(root.path(), DAY, &record, || Ok(()))
                    .unwrap()
                    .hash,
                expected
            );
        }
    }

    #[cfg(feature = "full-tests")]
    #[test]
    fn consuming_reads_refuse_replacement_growth_alias_and_fifo() {
        for shape in ["replace", "grow", "alias", "fifo"] {
            let root = tempfile::tempdir().unwrap();
            let (record, path) = fixture(root.path(), "named");
            let calls = Cell::new(0);
            let result = activity_input_bounded(root.path(), DAY, &record, || {
                calls.set(calls.get() + 1);
                // One directory: checkpoint 4 is after the first actual bounded
                // consumption, immediately before the second consuming read.
                if calls.get() == 4 {
                    if shape == "grow" {
                        fs::write(&path, vec![b' '; 4 * 1024 * 1024 + 1]).unwrap();
                    } else {
                        fs::rename(&path, path.with_extension("original")).unwrap();
                        match shape {
                            "replace" => fs::write(&path, FIRST).unwrap(),
                            "alias" => {
                                std::os::unix::fs::symlink(path.with_extension("original"), &path)
                                    .unwrap()
                            }
                            "fifo" => assert!(
                                std::process::Command::new("mkfifo")
                                    .arg(&path)
                                    .status()
                                    .unwrap()
                                    .success()
                            ),
                            _ => unreachable!(),
                        }
                    }
                }
                Ok(())
            });
            assert!(calls.get() >= 4, "{shape}");
            assert!(result.is_err(), "{shape}");
        }
    }

    #[test]
    fn missing_ambiguous_malformed_budget_and_stop_refuse() {
        for shape in [
            "missing",
            "ambiguous",
            "stream",
            "entries",
            "bytes",
            "aggregate",
            "stop",
            "directory_alias",
        ] {
            let root = tempfile::tempdir().unwrap();
            let (mut record, path) = fixture(root.path(), "named");
            match shape {
                "missing" => fs::remove_file(&path).unwrap(),
                "ambiguous" => {
                    record.remove("stream");
                    let second = root
                        .path()
                        .join("chronicle")
                        .join(DAY)
                        .join("other/120000_300/talents/sense.json");
                    fs::create_dir_all(second.parent().unwrap()).unwrap();
                    fs::write(second, FIRST).unwrap();
                }
                "stream" => {
                    record.insert("stream".into(), Value::Bool(false));
                }
                "entries" => {
                    for n in 0..1024 {
                        fs::write(
                            root.path()
                                .join("chronicle")
                                .join(DAY)
                                .join(format!("extra{n}")),
                            b"",
                        )
                        .unwrap();
                    }
                }
                "bytes" => fs::write(&path, vec![b' '; 4 * 1024 * 1024 + 1]).unwrap(),
                "aggregate" => {
                    let mut spans = Vec::new();
                    let mut bytes = FIRST.to_vec();
                    bytes.resize(4 * 1024 * 1024, b' ');
                    for n in 0..5 {
                        let key = format!("120{n}00_300");
                        spans.push(key.clone());
                        let p = root
                            .path()
                            .join("chronicle")
                            .join(DAY)
                            .join("recording")
                            .join(key)
                            .join("talents/sense.json");
                        fs::create_dir_all(p.parent().unwrap()).unwrap();
                        fs::write(p, &bytes).unwrap();
                    }
                    record.insert("segments".into(), serde_json::json!(spans));
                }
                "directory_alias" => {
                    let stream = root.path().join("chronicle").join(DAY).join("recording");
                    let moved = root.path().join("moved");
                    fs::rename(&stream, &moved).unwrap();
                    std::os::unix::fs::symlink(moved, stream).unwrap();
                }
                "stop" => {}
                _ => unreachable!(),
            }
            let result = activity_input_bounded(root.path(), DAY, &record, || {
                if shape == "stop" {
                    Err("stopped fixture".into())
                } else {
                    Ok(())
                }
            });
            assert!(result.is_err(), "{shape}");
        }
    }
}
