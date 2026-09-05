// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Journal-wide allocation of the numeric millisecond Cortex use identity.

use std::ffi::OsStr;
#[cfg(test)]
use std::fs;
use std::io;
use std::path::Path;

use super::{census_cortex_namespace, create_or_admit_cortex_namespace};
#[cfg(unix)]
use crate::read_observed_file_bounded as read_counter;
#[cfg(windows)]
use crate::read_windows_observed_file_bounded as read_counter;
use crate::{DetailedAtomicOutcome, JournalRoot, LockOptions, atomic_replace_detailed, hold_lock};

const COUNTER: &str = "health/cortex-use-id.json";
// Match the Cortex recovery population bound; bootstrap is a one-time census.
const MAXIMUM_BOOTSTRAP_ENTRIES: usize = 4 * 1024 * 1024;

/// Reserve an identity before broadcasting a request. Failed publication never
/// returns an id; a crash after publication can only leave an unused reservation.
pub fn allocate_cortex_use_id(journal: &Path, now_ms: i64) -> io::Result<i64> {
    if now_ms < 0 {
        return Err(io::Error::other("Cortex use clock precedes the Unix epoch"));
    }
    let root = JournalRoot::open(journal).map_err(io::Error::other)?;
    let authority = create_or_admit_cortex_namespace(root).map_err(io::Error::other)?;
    let path = authority.root().canonical_path().join(COUNTER);
    let _lock = hold_lock(
        &path,
        LockOptions {
            mode: Some(0o600),
            ..LockOptions::default()
        },
    )
    .map_err(io::Error::other)?;
    let previous = match read_counter(authority.health(), OsStr::new("cortex-use-id.json"), 128)
        .map_err(io::Error::other)?
    {
        Some(observed) => {
            let id: i64 = serde_json::from_slice(&observed.bytes).map_err(io::Error::other)?;
            if id < 0 {
                return Err(io::Error::other(
                    "Cortex use counter precedes the Unix epoch",
                ));
            }
            Some(id)
        }
        None => {
            let census = census_cortex_namespace(authority, MAXIMUM_BOOTSTRAP_ENTRIES)
                .map_err(io::Error::other)?;
            if census.refused_talent_count() != 0 {
                return Err(io::Error::other(
                    "Cortex use identity bootstrap could not inspect every talent",
                ));
            }
            census
                .talents()
                .iter()
                .flat_map(|talent| talent.entries())
                .filter_map(|entry| {
                    entry
                        .projections()
                        .active()
                        .or_else(|| entry.projections().completed())
                        .and_then(|id| id.parse::<i64>().ok())
                })
                .max()
        }
    };
    let issued = match previous {
        Some(previous) => now_ms.max(
            previous
                .checked_add(1)
                .ok_or_else(|| io::Error::other("Cortex use identity exhausted"))?,
        ),
        None => now_ms,
    };
    match atomic_replace_detailed(&path, format!("{issued}\n").as_bytes(), 0o600)
        .map_err(io::Error::other)?
    {
        DetailedAtomicOutcome::Published => {}
        outcome => {
            return Err(io::Error::other(format!(
                "Cortex use reservation uncertain: {outcome:?}"
            )));
        }
    }
    Ok(issued)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;

    #[test]
    fn bootstrap_and_clock_reversal_preserve_numeric_identity() {
        let root = TempDir::new();
        fs::create_dir_all(root.path().join("talents/a")).unwrap();
        fs::create_dir_all(root.path().join("talents/b")).unwrap();
        fs::write(root.path().join("talents/a/1000.jsonl"), b"{}").unwrap();
        fs::write(root.path().join("talents/b/1001_active.jsonl"), b"{}").unwrap();
        assert_eq!(allocate_cortex_use_id(root.path(), 500).unwrap(), 1002);
        assert_eq!(allocate_cortex_use_id(root.path(), 400).unwrap(), 1003);
        assert_eq!(allocate_cortex_use_id(root.path(), 2000).unwrap(), 2000);
        // A reserved-but-never-broadcast id stays consumed across allocators.
        assert_eq!(allocate_cortex_use_id(root.path(), 2000).unwrap(), 2001);
    }

    #[test]
    fn damaged_or_exhausted_counter_refuses_without_replacing_it() {
        let root = TempDir::new();
        allocate_cortex_use_id(root.path(), 42).unwrap();
        for bytes in ["", "null", "bad", "-1", "9223372036854775807"] {
            fs::write(root.path().join(COUNTER), bytes).unwrap();
            assert!(allocate_cortex_use_id(root.path(), 42).is_err());
            assert_eq!(
                fs::read_to_string(root.path().join(COUNTER)).unwrap(),
                bytes
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn unsafe_or_oversized_counter_is_refused_without_following_it() {
        use nix::sys::stat::Mode;
        use nix::unistd::mkfifo;
        use std::os::unix::fs::symlink;
        let root = TempDir::new();
        allocate_cortex_use_id(root.path(), 42).unwrap();
        let path = root.path().join(COUNTER);
        fs::write(&path, vec![b' '; 129]).unwrap();
        assert!(allocate_cortex_use_id(root.path(), 42).is_err());
        fs::remove_file(&path).unwrap();
        let target = root.path().join("outside");
        fs::write(&target, b"42").unwrap();
        symlink(&target, &path).unwrap();
        assert!(allocate_cortex_use_id(root.path(), 42).is_err());
        assert_eq!(fs::read(&target).unwrap(), b"42");
        fs::remove_file(&path).unwrap();
        mkfifo(&path, Mode::S_IRUSR | Mode::S_IWUSR).unwrap();
        assert!(allocate_cortex_use_id(root.path(), 42).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn publication_failures_never_return_an_unreserved_identity() {
        use crate::atomic::{BoundPublicationPrimitive, run_with_bound_publication_fault};
        for primitive in [
            BoundPublicationPrimitive::Write,
            BoundPublicationPrimitive::FileSync,
            BoundPublicationPrimitive::Rename,
            BoundPublicationPrimitive::ParentSync,
        ] {
            let root = TempDir::new();
            assert_eq!(allocate_cortex_use_id(root.path(), 42).unwrap(), 42);
            let (result, consumed) =
                run_with_bound_publication_fault(primitive, 1, nix::libc::EIO, || {
                    allocate_cortex_use_id(root.path(), 42)
                });
            assert!(consumed, "{primitive:?}");
            assert!(result.is_err(), "{primitive:?}");
            let next = allocate_cortex_use_id(root.path(), 42).unwrap();
            assert_eq!(
                next,
                if primitive == BoundPublicationPrimitive::ParentSync {
                    44
                } else {
                    43
                }
            );
        }
    }

    #[test]
    fn independent_processes_reserve_different_ids_at_the_same_clock() {
        let root = TempDir::new();
        let children = (0..8)
            .map(|index| {
                std::process::Command::new(std::env::current_exe().unwrap())
                    .args([
                        "--exact",
                        "cortex_use::allocation::tests::allocation_process_child",
                    ])
                    .env("CORTEX_ALLOCATION_TEST_ROOT", root.path())
                    .env(
                        "CORTEX_ALLOCATION_TEST_RESULT",
                        root.path().join(format!("result-{index}")),
                    )
                    .stdout(std::process::Stdio::null())
                    .spawn()
                    .unwrap()
            })
            .collect::<Vec<_>>();
        for mut child in children {
            assert!(child.wait().unwrap().success());
        }
        let mut ids = (0..8)
            .map(|index| {
                fs::read_to_string(root.path().join(format!("result-{index}")))
                    .unwrap()
                    .parse::<i64>()
                    .unwrap()
            })
            .collect::<Vec<_>>();
        ids.sort_unstable();
        assert_eq!(ids, (1000..1008).collect::<Vec<_>>());
    }

    #[test]
    fn allocation_process_child() {
        let Some(root) = std::env::var_os("CORTEX_ALLOCATION_TEST_ROOT") else {
            return;
        };
        let id = allocate_cortex_use_id(Path::new(&root), 1000).unwrap();
        fs::write(
            std::env::var_os("CORTEX_ALLOCATION_TEST_RESULT").unwrap(),
            id.to_string(),
        )
        .unwrap();
    }
}
