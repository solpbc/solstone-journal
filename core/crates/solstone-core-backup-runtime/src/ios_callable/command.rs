// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Decode the existing normal-backup argv into explicit linked operations.
//! This module grants no invocation or startup authority.

use std::path::{Component, Path, PathBuf};

const MAX_PATH_BYTES: usize = 4096;
const MAX_SELECTION_BYTES: usize = 8 * 1024 * 1024;
const MAX_RETRY_MILLIS: u64 = 48 * 60 * 60 * 1000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CheckRead {
    Structure,
    All,
    Group { group: u32, groups: u32 },
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Operation {
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
    Retention {
        hourly: u64,
        daily: u64,
        weekly: u64,
        monthly: u64,
    },
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Command {
    pub operation: Operation,
    pub retry_lock_millis: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct UnsupportedCommand;

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
pub(crate) fn relative_exclusion(
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

#[derive(Default)]
struct RetentionDraft {
    hourly: Option<u64>,
    daily: Option<u64>,
    weekly: Option<u64>,
    monthly: Option<u64>,
    archive: bool,
    offload: bool,
    grouped: bool,
    prune: bool,
}

fn canonical_count(value: &str) -> Result<u64, UnsupportedCommand> {
    if value == "0" {
        return Ok(0);
    }
    if value.is_empty()
        || value.as_bytes().first() == Some(&b'0')
        || value.as_bytes().first() == Some(&b'+')
        || value.as_bytes().first() == Some(&b'-')
        || !value.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err(UnsupportedCommand);
    }
    let parsed = value.parse::<u64>().map_err(|_| UnsupportedCommand)?;
    if parsed > i64::MAX as u64 {
        return Err(UnsupportedCommand);
    }
    Ok(parsed)
}

pub(crate) fn accept_repack(
    retention: bool,
    max_repack_size: Option<&str>,
) -> Result<(), UnsupportedCommand> {
    match (retention, max_repack_size) {
        (true, Some("1G")) => Ok(()),
        (false, None) => Ok(()),
        _ => Err(UnsupportedCommand),
    }
}

pub(crate) fn retention_policy_blob(hourly: u64, daily: u64, weekly: u64, monthly: u64) -> Vec<u8> {
    let archive = crate::engine::ARCHIVE_TAG.as_bytes();
    let offload = solstone_core_backup::archive_readback::OFFLOAD_ARCHIVE_TAG.as_bytes();
    let mut blob = Vec::with_capacity(79);
    blob.extend_from_slice(b"RTN1");
    for count in [hourly, daily, weekly, monthly] {
        blob.extend_from_slice(&count.to_le_bytes());
    }
    blob.extend_from_slice(&2u32.to_le_bytes());
    for tag in [archive, offload] {
        let len = u16::try_from(tag.len()).expect("keep tag length");
        blob.extend_from_slice(&len.to_le_bytes());
        blob.extend_from_slice(tag);
    }
    blob
}

pub(crate) fn decode(args: &[String]) -> Result<Command, UnsupportedCommand> {
    let mut args = args.iter().map(String::as_str);
    let mut retention = None;
    let mut operation = match args.next() {
        Some("init") => Some(Operation::Init),
        Some("cat") if args.next() == Some("config") => Some(Operation::Open),
        Some("snapshots") => Some(Operation::ListSnapshots),
        Some("backup") => Some(Operation::Snapshot {
            source: absolute_path(args.next().ok_or(UnsupportedCommand)?)?,
            exclusions: Vec::new(),
            exclude_file: None,
        }),
        Some("check") => Some(Operation::Check(CheckRead::Structure)),
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
            Some(Operation::Restore {
                snapshot: snapshot.to_owned(),
                selector: selector.to_owned(),
                target,
            })
        }
        Some("forget") => {
            retention = Some(RetentionDraft::default());
            None
        }
        _ => return Err(UnsupportedCommand),
    };
    let mut retry = None;
    let mut check_selection = false;
    let mut selection_bytes = 0usize;
    while let Some(flag) = args.next() {
        if let Some(draft) = retention.as_mut() {
            match flag {
                "--retry-lock" if retry.is_none() => {
                    retry = Some(retry_millis(args.next().ok_or(UnsupportedCommand)?)?);
                }
                "--keep-hourly" if draft.hourly.is_none() => {
                    draft.hourly = Some(canonical_count(args.next().ok_or(UnsupportedCommand)?)?);
                }
                "--keep-daily" if draft.daily.is_none() => {
                    draft.daily = Some(canonical_count(args.next().ok_or(UnsupportedCommand)?)?);
                }
                "--keep-weekly" if draft.weekly.is_none() => {
                    draft.weekly = Some(canonical_count(args.next().ok_or(UnsupportedCommand)?)?);
                }
                "--keep-monthly" if draft.monthly.is_none() => {
                    draft.monthly = Some(canonical_count(args.next().ok_or(UnsupportedCommand)?)?);
                }
                "--keep-tag" => match args.next() {
                    Some(tag) if tag == crate::engine::ARCHIVE_TAG && !draft.archive => {
                        draft.archive = true;
                    }
                    Some(tag)
                        if tag == solstone_core_backup::archive_readback::OFFLOAD_ARCHIVE_TAG
                            && !draft.offload =>
                    {
                        draft.offload = true;
                    }
                    _ => return Err(UnsupportedCommand),
                },
                "--group-by" if !draft.grouped => {
                    if args.next() != Some("tags") {
                        return Err(UnsupportedCommand);
                    }
                    draft.grouped = true;
                }
                "--prune" if !draft.prune => draft.prune = true,
                _ => return Err(UnsupportedCommand),
            }
            continue;
        }
        match flag {
            "--retry-lock" if retry.is_none() => {
                retry = Some(retry_millis(args.next().ok_or(UnsupportedCommand)?)?);
            }
            "--exclude" => {
                let Some(Operation::Snapshot {
                    source, exclusions, ..
                }) = operation.as_mut()
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
                let Some(Operation::Snapshot { exclude_file, .. }) = operation.as_mut() else {
                    return Err(UnsupportedCommand);
                };
                if exclude_file.is_some() {
                    return Err(UnsupportedCommand);
                }
                *exclude_file = Some(absolute_path(args.next().ok_or(UnsupportedCommand)?)?);
            }
            "--read-data" if !check_selection => {
                let Some(Operation::Check(mode)) = operation.as_mut() else {
                    return Err(UnsupportedCommand);
                };
                *mode = CheckRead::All;
                check_selection = true;
            }
            "--read-data-subset" if !check_selection => {
                let Some(Operation::Check(mode)) = operation.as_mut() else {
                    return Err(UnsupportedCommand);
                };
                *mode = group(args.next().ok_or(UnsupportedCommand)?)?;
                check_selection = true;
            }
            _ => return Err(UnsupportedCommand),
        }
    }
    if let Some(draft) = retention {
        let (Some(hourly), Some(daily), Some(weekly), Some(monthly)) =
            (draft.hourly, draft.daily, draft.weekly, draft.monthly)
        else {
            return Err(UnsupportedCommand);
        };
        if !draft.archive || !draft.offload || !draft.grouped || !draft.prune {
            return Err(UnsupportedCommand);
        }
        operation = Some(Operation::Retention {
            hourly,
            daily,
            weekly,
            monthly,
        });
    }
    let operation = operation.ok_or(UnsupportedCommand)?;
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
    #[cfg(unix)]
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

    fn forget(parts: &[&str]) -> Result<Command, UnsupportedCommand> {
        let mut args = vec![
            "forget",
            "--keep-hourly",
            "1",
            "--keep-daily",
            "1",
            "--keep-weekly",
            "1",
            "--keep-monthly",
            "1",
            "--keep-tag",
            crate::engine::ARCHIVE_TAG,
            "--keep-tag",
            solstone_core_backup::archive_readback::OFFLOAD_ARCHIVE_TAG,
            "--group-by",
            "tags",
            "--prune",
        ];
        args.extend_from_slice(parts);
        command(&args)
    }

    #[test]
    fn forget_keeps_zero_counts_tags_grouping_and_retry() {
        let kept = command(&[
            "forget",
            "--prune",
            "--group-by",
            "tags",
            "--keep-tag",
            solstone_core_backup::archive_readback::OFFLOAD_ARCHIVE_TAG,
            "--keep-monthly",
            "0",
            "--keep-tag",
            crate::engine::ARCHIVE_TAG,
            "--retry-lock",
            "30m",
            "--keep-weekly",
            "0",
            "--keep-daily",
            "0",
            "--keep-hourly",
            "0",
        ])
        .unwrap();
        assert_eq!(kept.retry_lock_millis, 1_800_000);
        assert_eq!(
            kept.operation,
            Operation::Retention {
                hourly: 0,
                daily: 0,
                weekly: 0,
                monthly: 0,
            }
        );
        let blob = retention_policy_blob(0, 7, 4, 12);
        assert_eq!(blob.len(), 79);
        assert_eq!(&blob[..4], b"RTN1");
        assert_eq!(u64::from_le_bytes(blob[4..12].try_into().unwrap()), 0);
        assert_eq!(u64::from_le_bytes(blob[12..20].try_into().unwrap()), 7);
        assert_eq!(u64::from_le_bytes(blob[20..28].try_into().unwrap()), 4);
        assert_eq!(u64::from_le_bytes(blob[28..36].try_into().unwrap()), 12);
        assert_eq!(u32::from_le_bytes(blob[36..40].try_into().unwrap()), 2);
        let archive = crate::engine::ARCHIVE_TAG.as_bytes();
        let offload = solstone_core_backup::archive_readback::OFFLOAD_ARCHIVE_TAG.as_bytes();
        assert_eq!(
            u16::from_le_bytes(blob[40..42].try_into().unwrap()) as usize,
            archive.len()
        );
        assert_eq!(&blob[42..42 + archive.len()], archive);
        let offset = 42 + archive.len();
        assert_eq!(
            u16::from_le_bytes(blob[offset..offset + 2].try_into().unwrap()) as usize,
            offload.len()
        );
        assert_eq!(&blob[offset + 2..], offload);
        let max = i64::MAX.to_string();
        let counted = command(&[
            "forget",
            "--keep-hourly",
            max.as_str(),
            "--keep-daily",
            "7",
            "--keep-weekly",
            "4",
            "--keep-monthly",
            "12",
            "--keep-tag",
            crate::engine::ARCHIVE_TAG,
            "--keep-tag",
            solstone_core_backup::archive_readback::OFFLOAD_ARCHIVE_TAG,
            "--group-by",
            "tags",
            "--prune",
        ])
        .unwrap();
        assert_eq!(
            counted.operation,
            Operation::Retention {
                hourly: i64::MAX as u64,
                daily: 7,
                weekly: 4,
                monthly: 12,
            }
        );
        let overflow = (i64::MAX as u64 + 1).to_string();
        for extra in [
            vec!["--keep-tag", crate::engine::ARCHIVE_TAG],
            vec!["--keep-tag", "other"],
            vec!["--keep-last", "1"],
            vec!["--keep-within", "1d"],
            vec!["--keep-yearly", "1"],
            vec!["--dry-run"],
            vec!["--unsafe-allow-remove-all"],
            vec!["--retry-lock", "30m", "--retry-lock", "1m"],
        ] {
            assert!(forget(&extra).is_err(), "accepted {extra:?}");
        }
        for count in ["00", "01", "+1", "-1", overflow.as_str()] {
            assert!(
                command(&[
                    "forget",
                    "--keep-hourly",
                    count,
                    "--keep-daily",
                    "0",
                    "--keep-weekly",
                    "0",
                    "--keep-monthly",
                    "0",
                    "--keep-tag",
                    crate::engine::ARCHIVE_TAG,
                    "--keep-tag",
                    solstone_core_backup::archive_readback::OFFLOAD_ARCHIVE_TAG,
                    "--group-by",
                    "tags",
                    "--prune",
                ])
                .is_err(),
                "accepted count {count}"
            );
        }
        for group in ["host,paths", "tag", "host"] {
            assert!(
                command(&[
                    "forget",
                    "--keep-hourly",
                    "1",
                    "--keep-daily",
                    "1",
                    "--keep-weekly",
                    "1",
                    "--keep-monthly",
                    "1",
                    "--keep-tag",
                    crate::engine::ARCHIVE_TAG,
                    "--keep-tag",
                    solstone_core_backup::archive_readback::OFFLOAD_ARCHIVE_TAG,
                    "--group-by",
                    group,
                    "--prune",
                ])
                .is_err(),
                "accepted grouping {group}"
            );
        }
        assert!(command(&["forget", "--prune"]).is_err());
        assert!(command(&["forget", "--keep-hourly", "1", "--prune"]).is_err());
        assert!(
            command(&[
                "forget",
                "--keep-hourly",
                "1",
                "--keep-daily",
                "1",
                "--keep-weekly",
                "1",
                "--keep-monthly",
                "1",
                "--keep-tag",
                crate::engine::ARCHIVE_TAG,
                "--keep-tag",
                solstone_core_backup::archive_readback::OFFLOAD_ARCHIVE_TAG,
                "--group-by",
                "tags",
            ])
            .is_err()
        );
    }

    #[test]
    fn repack_bound_accepts_only_one_gigabyte_on_retention() {
        assert!(accept_repack(true, Some("1G")).is_ok());
        assert!(accept_repack(false, None).is_ok());
        assert!(accept_repack(true, None).is_err());
        for size in ["1g", "1GB", "2G", "", "1073741824"] {
            assert!(accept_repack(true, Some(size)).is_err(), "{size}");
            assert!(accept_repack(false, Some(size)).is_err(), "{size}");
        }
        assert!(accept_repack(false, Some("1G")).is_err());
    }
}
