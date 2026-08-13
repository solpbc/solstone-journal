// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HandlerSpec {
    pub name: &'static str,
    pub patterns: &'static [&'static str],
    pub command: Vec<String>,
}

pub fn default_registry(describe_jobs: usize) -> Vec<HandlerSpec> {
    vec![
        HandlerSpec {
            name: "transcribe",
            patterns: &[".flac", ".wav", ".mp3", ".m4a"],
            command: vec!["journal".into(), "transcribe".into(), "{file}".into()],
        },
        HandlerSpec {
            name: "describe",
            patterns: &[".webm", ".mp4", ".mov"],
            command: vec![
                "journal".into(),
                "describe".into(),
                "{file}".into(),
                "-j".into(),
                describe_jobs.to_string(),
            ],
        },
        HandlerSpec {
            name: "depict",
            patterns: &[".png", ".jpg", ".jpeg", ".heic", ".webp", ".tiff"],
            command: vec!["journal".into(), "depict".into(), "{file}".into()],
        },
    ]
}

/// Replace the executable while keeping the production argv shape. This is a
/// process seam for native integration tests; the service never sets it.
pub fn with_program(registry: &mut [HandlerSpec], program: &Path) {
    for spec in registry {
        spec.command[0] = program.display().to_string();
    }
}

pub fn match_handler(journal: &Path, path: &Path, registry: &[HandlerSpec]) -> Option<HandlerSpec> {
    if path.file_name()?.to_str()?.starts_with('.') {
        return None;
    }
    let root = journal.join("chronicle");
    let relative = path
        .strip_prefix(&root)
        .or_else(|_| path.strip_prefix(journal))
        .ok()?;
    let parts: Vec<_> = relative.components().collect();
    if parts.len() != 4 {
        return None;
    }
    let day = parts[0].as_os_str().to_str()?;
    if day.len() != 8 || !day.bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let suffix = path.extension()?.to_str()?.to_ascii_lowercase();
    registry
        .iter()
        .find(|spec| {
            spec.patterns
                .iter()
                .any(|pattern| *pattern == format!(".{suffix}"))
        })
        .cloned()
}

pub fn command_for(spec: &HandlerSpec, file: &Path, verbose: bool, debug: bool) -> Vec<String> {
    let mut command = spec
        .command
        .iter()
        .map(|arg| {
            if arg == "{file}" {
                file.display().to_string()
            } else {
                arg.clone()
            }
        })
        .collect::<Vec<_>>();
    if debug {
        command.push("-d".into());
    } else if verbose {
        command.push("-v".into());
    }
    command
}

pub fn segment_dir(journal: &Path, day: &str, stream: Option<&str>, segment: &str) -> PathBuf {
    match stream {
        Some(stream) => journal
            .join("chronicle")
            .join(day)
            .join(stream)
            .join(segment),
        None => journal.join("chronicle").join(day).join(segment),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dot_prefixed_recording_is_rejected_before_pattern_matching() {
        let root = Path::new("/j");
        let registry = default_registry(10);
        assert!(
            match_handler(
                root,
                Path::new("/j/chronicle/20260101/default/120000_3/.live.webm"),
                &registry
            )
            .is_none()
        );
    }
    #[test]
    fn four_part_dated_segment_shape_is_required() {
        let root = Path::new("/j");
        let registry = default_registry(10);
        assert!(
            match_handler(
                root,
                Path::new("/j/chronicle/no/default/120000_3/a.webm"),
                &registry
            )
            .is_none()
        );
        assert!(
            match_handler(
                root,
                Path::new("/j/chronicle/20260101/default/a.webm"),
                &registry
            )
            .is_none()
        );
    }
    #[test]
    fn debug_wins() {
        let spec = default_registry(4).remove(1);
        let cmd = command_for(&spec, Path::new("/a.webm"), true, true);
        assert!(cmd.ends_with(&["-d".to_string()]));
        assert!(!cmd.contains(&"-v".to_string()));
    }
}
