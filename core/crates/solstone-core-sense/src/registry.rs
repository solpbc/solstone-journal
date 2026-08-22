// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use solstone_core_format::segment::segment_key;
use thiserror::Error;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HandlerSpec {
    pub name: &'static str,
    pub patterns: &'static [&'static str],
    pub command: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub(crate) enum DispatcherResolveError {
    #[error("dispatcher missing: {path}")]
    Missing { path: PathBuf },
    #[error("dispatcher not a file: {path}")]
    NonRegular { path: PathBuf },
    #[error("dispatcher not executable: {path}")]
    NonExecutable { path: PathBuf },
    #[error("dispatcher current_exe failed: {message}")]
    CurrentExe { message: String },
}

pub fn default_registry(describe_jobs: usize) -> Vec<HandlerSpec> {
    vec![
        HandlerSpec {
            name: "transcribe",
            patterns: &[".flac", ".opus", ".ogg", ".m4a", ".mp3", ".wav"],
            command: vec!["transcribe".into(), "{file}".into()],
        },
        HandlerSpec {
            name: "describe",
            patterns: &[".webm", ".mp4", ".mov"],
            command: vec![
                "describe".into(),
                "{file}".into(),
                "-j".into(),
                describe_jobs.to_string(),
            ],
        },
        HandlerSpec {
            name: "depict",
            patterns: &[
                ".png", ".jpg", ".jpeg", ".heic", ".heif", ".gif", ".webp", ".tiff",
            ],
            command: vec!["depict".into(), "{file}".into()],
        },
    ]
}

pub(crate) fn resolve_dispatcher_in(dir: &Path) -> Result<PathBuf, DispatcherResolveError> {
    let candidate = dir.join("solstone-core-journal");
    match fs::metadata(&candidate) {
        Ok(metadata) if metadata.is_file() => {
            if dispatcher_is_executable(&metadata) {
                Ok(candidate)
            } else {
                Err(DispatcherResolveError::NonExecutable { path: candidate })
            }
        }
        Ok(_) => Err(DispatcherResolveError::NonRegular { path: candidate }),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            Err(DispatcherResolveError::Missing { path: candidate })
        }
        Err(_) => Err(DispatcherResolveError::NonExecutable { path: candidate }),
    }
}

fn dispatcher_is_executable(metadata: &fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        true
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
    let (day, _segment, _file) = match parts.as_slice() {
        [day, segment, file]
            if segment
                .as_os_str()
                .to_str()
                .is_some_and(|value| segment_key(value).as_deref() == Some(value)) =>
        {
            (day, segment, file)
        }
        [day, _stream, _segment, file] => (day, _segment, file),
        _ => return None,
    };
    let day = day.as_os_str().to_str()?;
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

pub fn command_for(
    spec: &HandlerSpec,
    program: &Path,
    file: &Path,
    verbose: bool,
    debug: bool,
) -> Vec<String> {
    let mut command = Vec::with_capacity(spec.command.len() + 2);
    command.push(program.display().to_string());
    for arg in &spec.command {
        command.push(if arg == "{file}" {
            file.display().to_string()
        } else {
            arg.clone()
        });
    }
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

    #[cfg(unix)]
    fn write_mode(path: &Path, mode: u32) {
        use std::os::unix::fs::PermissionsExt;
        fs::write(path, b"").expect("write candidate");
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).expect("set mode");
    }

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
    fn named_and_default_stream_paths_require_a_valid_day_and_segment() {
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
    fn three_part_default_stream_segment_path_is_accepted() {
        let root = Path::new("/j");
        let registry = default_registry(10);
        assert_eq!(
            match_handler(
                root,
                Path::new("/j/chronicle/20260101/120000_3/a.webm"),
                &registry
            )
            .expect("default-stream video"),
            default_registry(10)
                .into_iter()
                .find(|spec| spec.name == "describe")
                .expect("describe spec")
        );
    }
    #[test]
    fn debug_wins() {
        let spec = default_registry(4).remove(1);
        let cmd = command_for(
            &spec,
            Path::new("/dispatcher"),
            Path::new("/a.webm"),
            true,
            true,
        );
        assert!(cmd.ends_with(&["-d".to_string()]));
        assert!(!cmd.contains(&"-v".to_string()));
    }

    #[test]
    fn default_registry_commands_start_at_the_handler_verb() {
        let registry = default_registry(10);
        let verbs = ["transcribe", "describe", "depict"];
        assert_eq!(registry.len(), verbs.len());
        for (spec, verb) in registry.iter().zip(verbs) {
            assert_eq!(spec.name, verb);
            assert_eq!(spec.command.first().map(String::as_str), Some(verb));
            for arg in &spec.command {
                assert_ne!(arg, "journal");
                assert_ne!(arg, "solstone");
                assert_ne!(arg, "solstone-core-transcribe");
            }
        }
    }

    #[test]
    fn command_for_prepends_the_program_keeps_file_and_debug_flag() {
        let spec = default_registry(4).remove(1);
        let cmd = command_for(
            &spec,
            Path::new("/dispatcher"),
            Path::new("/a.webm"),
            true,
            true,
        );
        assert_eq!(cmd[0], "/dispatcher");
        assert_eq!(cmd[1], "describe");
        assert!(cmd.contains(&"/a.webm".to_string()));
        assert!(!cmd.contains(&"{file}".to_string()));
        assert!(cmd.ends_with(&["-d".to_string()]));
        assert!(!cmd.contains(&"-v".to_string()));
    }

    #[cfg(unix)]
    #[test]
    fn resolve_dispatcher_in_returns_the_unix_executable_sibling() {
        let temp = tempfile::tempdir().expect("dir");
        let candidate = temp.path().join("solstone-core-journal");
        write_mode(&candidate, 0o755);
        assert_eq!(
            resolve_dispatcher_in(temp.path()).expect("executable sibling"),
            candidate
        );
    }

    #[test]
    fn resolve_dispatcher_in_reports_missing_without_searching_path() {
        let temp = tempfile::tempdir().expect("dir");
        let reported = resolve_dispatcher_in(temp.path()).expect_err("missing");
        assert_eq!(
            reported,
            DispatcherResolveError::Missing {
                path: temp.path().join("solstone-core-journal"),
            }
        );
    }

    #[test]
    fn resolve_dispatcher_in_rejects_a_non_regular_candidate() {
        let temp = tempfile::tempdir().expect("dir");
        let candidate = temp.path().join("solstone-core-journal");
        fs::create_dir(&candidate).expect("directory candidate");
        assert_eq!(
            resolve_dispatcher_in(temp.path()).expect_err("non-regular"),
            DispatcherResolveError::NonRegular { path: candidate }
        );
    }

    #[cfg(unix)]
    #[test]
    fn resolve_dispatcher_in_rejects_a_non_executable_regular_file() {
        let temp = tempfile::tempdir().expect("dir");
        let candidate = temp.path().join("solstone-core-journal");
        write_mode(&candidate, 0o644);
        assert_eq!(
            resolve_dispatcher_in(temp.path()).expect_err("non-executable"),
            DispatcherResolveError::NonExecutable { path: candidate }
        );
    }
}
