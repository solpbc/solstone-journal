// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Decode the existing normal-backup argv into explicit linked operations.
//! This module grants no invocation or startup authority.

use std::path::{Component, Path, PathBuf};

const MAX_PATH_BYTES: usize = 4096;
const MAX_SELECTION_BYTES: usize = 8 * 1024 * 1024;
const MAX_RETRY_MILLIS: u64 = 48 * 60 * 60 * 1000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CheckRead {
    Structure,
    All,
    Group { group: u32, groups: u32 },
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Operation {
    Init,
    Open,
    ListSnapshots,
    Snapshot {
        source: PathBuf,
        exclusions: Vec<String>,
        exclude_file: Option<PathBuf>,
    },
    Check(CheckRead),
    /// One snapshot's journal subtree into the admitted existing journal root.
    Restore {
        snapshot: String,
        selector: String,
        target: PathBuf,
    },
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct Command {
    pub operation: Operation,
    pub retry_lock_millis: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct UnsupportedCommand;

fn absolute_path(value: &str) -> Result<PathBuf, UnsupportedCommand> {
    let path = Path::new(value);
    if value.is_empty()
        || value.len() > MAX_PATH_BYTES
        || value.contains(['\0', '\r', '\n'])
        || !path.is_absolute()
        || path
            .components()
            .any(|part| matches!(part, Component::ParentDir | Component::CurDir))
    {
        return Err(UnsupportedCommand);
    }
    Ok(path.to_owned())
}

fn retry_millis(value: &str) -> Result<u64, UnsupportedCommand> {
    let (number, scale) = if let Some(number) = value.strip_suffix("ms") {
        (number, 1)
    } else if let Some(number) = value.strip_suffix('s') {
        (number, 1000)
    } else if let Some(number) = value.strip_suffix('m') {
        (number, 60 * 1000)
    } else if let Some(number) = value.strip_suffix('h') {
        (number, 60 * 60 * 1000)
    } else {
        return Err(UnsupportedCommand);
    };
    if number.is_empty() || !number.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(UnsupportedCommand);
    }
    number
        .parse::<u64>()
        .ok()
        .and_then(|number| number.checked_mul(scale))
        .filter(|millis| *millis <= MAX_RETRY_MILLIS)
        .ok_or(UnsupportedCommand)
}

fn group(value: &str) -> Result<CheckRead, UnsupportedCommand> {
    let (selected, total) = value.split_once('/').ok_or(UnsupportedCommand)?;
    if selected.is_empty()
        || total.is_empty()
        || !selected.bytes().all(|byte| byte.is_ascii_digit())
        || !total.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err(UnsupportedCommand);
    }
    let selected = selected.parse::<u32>().map_err(|_| UnsupportedCommand)?;
    let total = total.parse::<u32>().map_err(|_| UnsupportedCommand)?;
    if selected == 0 || selected > total || total > 256 {
        return Err(UnsupportedCommand);
    }
    Ok(CheckRead::Group {
        group: selected,
        groups: total,
    })
}

/// Convert exactly the producer's rooted glob prefix. No environment expansion
/// occurs in the linked bridge. Only exclude-file lines escape dollars twice.
pub(super) fn relative_exclusion(
    source: &Path,
    pattern: &str,
    from_file: bool,
) -> Result<String, UnsupportedCommand> {
    if pattern.contains(['\0', '\r', '\n']) || pattern.len() > MAX_SELECTION_BYTES {
        return Err(UnsupportedCommand);
    }
    let decoded = if from_file {
        pattern.replace("$$", "$")
    } else {
        pattern.to_owned()
    };
    let prefix = crate::restic_paths::rooted_exclude(source, "");
    let relative = decoded.strip_prefix(&prefix).ok_or(UnsupportedCommand)?;
    if relative.is_empty()
        || relative.starts_with('/')
        || relative.split('/').any(|part| part == ".." || part == ".")
    {
        return Err(UnsupportedCommand);
    }
    let encoded = crate::restic_paths::rooted_exclude(source, relative);
    if (from_file && encoded.replace('$', "$$") != pattern) || (!from_file && encoded != pattern) {
        return Err(UnsupportedCommand);
    }
    Ok(relative.to_owned())
}

pub(super) fn decode(args: &[String]) -> Result<Command, UnsupportedCommand> {
    let mut args = args.iter().map(String::as_str);
    let mut operation = match args.next() {
        Some("init") => Operation::Init,
        Some("cat") if args.next() == Some("config") => Operation::Open,
        Some("snapshots") => Operation::ListSnapshots,
        Some("backup") => Operation::Snapshot {
            source: absolute_path(args.next().ok_or(UnsupportedCommand)?)?,
            exclusions: Vec::new(),
            exclude_file: None,
        },
        Some("check") => Operation::Check(CheckRead::Structure),
        Some("restore") => {
            let (snapshot, selector) = args
                .next()
                .and_then(|value| value.split_once(':'))
                .ok_or(UnsupportedCommand)?;
            if snapshot.len() != 64
                || !snapshot
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            {
                return Err(UnsupportedCommand);
            }
            // The snapshot's own recorded path, matched natively by the same rule.
            let parts = selector.strip_prefix('/').unwrap_or(selector);
            if selector.len() > MAX_PATH_BYTES
                || selector.contains(['\0', '\r', '\n', '\\'])
                || parts.split('/').count() > 64
                || parts
                    .split('/')
                    .any(|part| part.is_empty() || part == "." || part == "..")
            {
                return Err(UnsupportedCommand);
            }
            if args.next() != Some("--target") {
                return Err(UnsupportedCommand);
            }
            let target = absolute_path(args.next().ok_or(UnsupportedCommand)?)?;
            Operation::Restore {
                snapshot: snapshot.to_owned(),
                selector: selector.to_owned(),
                target,
            }
        }
        _ => return Err(UnsupportedCommand),
    };
    let mut retry = None;
    let mut check_selection = false;
    let mut selection_bytes = 0usize;
    while let Some(flag) = args.next() {
        match flag {
            "--retry-lock" if retry.is_none() => {
                retry = Some(retry_millis(args.next().ok_or(UnsupportedCommand)?)?);
            }
            "--exclude" => {
                let Operation::Snapshot {
                    source, exclusions, ..
                } = &mut operation
                else {
                    return Err(UnsupportedCommand);
                };
                let pattern =
                    relative_exclusion(source, args.next().ok_or(UnsupportedCommand)?, false)?;
                selection_bytes = selection_bytes
                    .checked_add(pattern.len())
                    .ok_or(UnsupportedCommand)?;
                if selection_bytes > MAX_SELECTION_BYTES {
                    return Err(UnsupportedCommand);
                }
                exclusions.push(pattern);
            }
            "--exclude-file" => {
                let Operation::Snapshot { exclude_file, .. } = &mut operation else {
                    return Err(UnsupportedCommand);
                };
                if exclude_file.is_some() {
                    return Err(UnsupportedCommand);
                }
                *exclude_file = Some(absolute_path(args.next().ok_or(UnsupportedCommand)?)?);
            }
            "--read-data" if !check_selection => {
                let Operation::Check(mode) = &mut operation else {
                    return Err(UnsupportedCommand);
                };
                *mode = CheckRead::All;
                check_selection = true;
            }
            "--read-data-subset" if !check_selection => {
                let Operation::Check(mode) = &mut operation else {
                    return Err(UnsupportedCommand);
                };
                *mode = group(args.next().ok_or(UnsupportedCommand)?)?;
                check_selection = true;
            }
            _ => return Err(UnsupportedCommand),
        }
    }
    if retry.is_some()
        && matches!(
            operation,
            Operation::Init | Operation::Open | Operation::Restore { .. }
        )
    {
        return Err(UnsupportedCommand);
    }
    Ok(Command {
        operation,
        retry_lock_millis: retry.unwrap_or(0),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn command(args: &[&str]) -> Result<Command, UnsupportedCommand> {
        decode(
            &args
                .iter()
                .map(|value| (*value).to_owned())
                .collect::<Vec<_>>(),
        )
    }

    #[test]
    fn normal_calls_keep_retry_and_exact_weekly_selection() {
        assert_eq!(command(&["init"]).unwrap().operation, Operation::Init);
        assert_eq!(
            command(&["cat", "config"]).unwrap().operation,
            Operation::Open
        );
        assert_eq!(
            command(&["snapshots"]).unwrap().operation,
            Operation::ListSnapshots
        );
        let check = command(&[
            "check",
            "--read-data-subset",
            "52/52",
            "--retry-lock",
            "30m",
        ])
        .unwrap();
        assert_eq!(
            check.operation,
            Operation::Check(CheckRead::Group {
                group: 52,
                groups: 52
            })
        );
        assert_eq!(check.retry_lock_millis, 1_800_000);
        assert_eq!(
            command(&["check"]).unwrap().operation,
            Operation::Check(CheckRead::Structure)
        );
        assert_eq!(
            command(&["check", "--read-data"]).unwrap().operation,
            Operation::Check(CheckRead::All)
        );
        assert_eq!(retry_millis("48h"), Ok(172_800_000));
        assert_eq!(retry_millis("0ms"), Ok(0));
    }

    #[test]
    fn unsupported_or_ambiguous_inputs_refuse() {
        for args in [
            vec![],
            vec!["unlock"],
            vec!["forget", "--prune"],
            vec!["key", "remove", "id"],
            vec!["restore", "latest"],
            vec!["cat", "key"],
            vec!["init", "--retry-lock", "0s"],
            vec!["cat", "config", "--retry-lock", "1m"],
            vec!["check", "--insecure-tls"],
            vec!["check", "--read-data-subset", "1%"],
            vec!["check", "--read-data-subset", "1G"],
            vec!["check", "--read-data-subset", "0/52"],
            vec!["check", "--read-data-subset", "53/52"],
            vec!["check", "--read-data-subset", "1/257"],
            vec!["check", "--read-data-subset", "1/52/2"],
            vec!["check", "--read-data", "--read-data"],
            vec!["check", "--read-data", "--read-data-subset", "1/52"],
            vec!["check", "--retry-lock", "1m", "--retry-lock", "2m"],
            vec!["check", "--retry-lock", "49h"],
            vec!["check", "--retry-lock", "18446744073709551615h"],
            vec!["check", "--retry-lock", "-1s"],
            vec!["check", "--retry-lock"],
            vec!["backup", "relative"],
            vec!["backup", "/journal/../other"],
            vec!["backup", "/journal", "--exclude", "/other/secret"],
            vec!["backup", "/journal", "--tag", "archive"],
            vec!["backup", "/journal", "--force"],
            vec!["backup", "/journal", "--no-cache"],
            vec!["backup", "/journal", "--tag", "spb-offload-archive"],
            vec!["snapshots", "--exclude", "/journal/secret"],
            vec!["check", "--json"],
        ] {
            assert!(
                command(&args).is_err(),
                "accepted unsupported input: {args:?}"
            );
        }
    }

    #[test]
    fn literal_source_globs_and_exclude_file_dollars_are_preserved() {
        let source = Path::new(r"/owner [a]*?/$HOME\notes");
        let direct = r"/owner [[]a][*][?]/$HOME\\notes/imports/body.sqlite";
        assert_eq!(
            relative_exclusion(source, direct, false).unwrap(),
            "imports/body.sqlite"
        );
        let file_line =
            r"/owner [[]a][*][?]/$$HOME\\notes/chronicle/20261012/indexer/messages.sqlite-wal";
        assert_eq!(
            relative_exclusion(source, file_line, true).unwrap(),
            "chronicle/20261012/indexer/messages.sqlite-wal"
        );
        assert!(relative_exclusion(source, direct, true).is_err());
        assert!(relative_exclusion(Path::new("/journal"), "/journal/../secret", false).is_err());
        let backup = command(&[
            "backup",
            "/journal",
            "--exclude",
            "/journal/cache/**",
            "--exclude-file",
            "/private/tmp/excludes",
            "--retry-lock",
            "30m",
        ])
        .unwrap();
        assert_eq!(
            backup.operation,
            Operation::Snapshot {
                source: PathBuf::from("/journal"),
                exclusions: vec!["cache/**".into()],
                exclude_file: Some(PathBuf::from("/private/tmp/excludes"))
            }
        );
    }
}
