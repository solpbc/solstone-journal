// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Process file descriptor limit adjustment for MCP listener connections.

use nix::sys::resource::{RLIM_INFINITY, Resource, rlim_t};

const NR_OPEN_PATH: &str = "/proc/sys/fs/nr_open";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SoftNofileTarget {
    Set(u64),
    NoCall,
}

/// Compute the desired soft `RLIMIT_NOFILE` target.
///
/// `hard: None` represents `RLIM_INFINITY`. The target is `min(finite hard, cap)`
/// (or `cap` when hard is infinite), bounded below by `soft`. If the computed target
/// equals `soft`, `NoCall` is returned.
pub(crate) fn soft_nofile_target(soft: u64, hard: Option<u64>, cap: u64) -> SoftNofileTarget {
    let ceiling = match hard {
        Some(h) => h.min(cap),
        None => cap,
    };
    let target = ceiling.max(soft);
    if target == soft {
        SoftNofileTarget::NoCall
    } else {
        SoftNofileTarget::Set(target)
    }
}

pub(crate) fn read_nr_open_cap() -> Option<u64> {
    let content = std::fs::read_to_string(NR_OPEN_PATH)
        .inspect_err(|_| {
            log::warn!("could not read nr_open");
        })
        .ok()?;
    content
        .trim()
        .parse::<u64>()
        .inspect_err(|_| {
            log::warn!("could not parse nr_open");
        })
        .ok()
}

pub(crate) fn apply_soft_nofile_limit() {
    apply_soft_nofile_limit_with_sources(
        read_nr_open_cap,
        |resource| nix::sys::resource::getrlimit(resource).map_err(|e| e.to_string()),
        |resource, soft, hard| {
            nix::sys::resource::setrlimit(resource, soft, hard).map_err(|e| e.to_string())
        },
    );
}

pub(crate) fn apply_soft_nofile_limit_with_sources<F, G, S>(read_cap: F, get_limit: G, set_limit: S)
where
    F: FnOnce() -> Option<u64>,
    G: FnOnce(Resource) -> Result<(rlim_t, rlim_t), String>,
    S: FnOnce(Resource, rlim_t, rlim_t) -> Result<(), String>,
{
    let Some(cap) = read_cap() else {
        return;
    };
    let (cur, max) = match get_limit(Resource::RLIMIT_NOFILE) {
        Ok(limits) => limits,
        Err(_) => {
            log::warn!("could not query RLIMIT_NOFILE");
            return;
        }
    };
    #[allow(clippy::unnecessary_cast)]
    let hard_opt = if max == RLIM_INFINITY {
        None
    } else {
        Some(max as u64)
    };
    #[allow(clippy::unnecessary_cast)]
    match soft_nofile_target(cur as u64, hard_opt, cap) {
        SoftNofileTarget::NoCall => {}
        SoftNofileTarget::Set(target) => {
            let (new_soft, new_hard) = match hard_opt {
                None => (target as rlim_t, target as rlim_t),
                Some(hard) => (target as rlim_t, hard as rlim_t),
            };
            if set_limit(Resource::RLIMIT_NOFILE, new_soft, new_hard).is_err() {
                log::warn!("could not set RLIMIT_NOFILE");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rlimit_target_cases() {
        // (256, infinity, 61440) -> 61440
        assert_eq!(
            soft_nofile_target(256, None, 61440),
            SoftNofileTarget::Set(61440)
        );

        // (256, 4096, 61440) -> 4096
        assert_eq!(
            soft_nofile_target(256, Some(4096), 61440),
            SoftNofileTarget::Set(4096)
        );

        // (8192, 8192, _) -> NoCall
        assert_eq!(
            soft_nofile_target(8192, Some(8192), 61440),
            SoftNofileTarget::NoCall
        );
        assert_eq!(
            soft_nofile_target(8192, Some(8192), 1024),
            SoftNofileTarget::NoCall
        );

        // (1024, infinity, 1024) -> NoCall
        assert_eq!(
            soft_nofile_target(1024, None, 1024),
            SoftNofileTarget::NoCall
        );

        // (4096, infinity, 2048) -> NoCall
        assert_eq!(
            soft_nofile_target(4096, None, 2048),
            SoftNofileTarget::NoCall
        );
    }

    #[test]
    fn unreadable_cap_skips_syscall() {
        let mut get_called = false;
        let mut set_called = false;
        apply_soft_nofile_limit_with_sources(
            || None,
            |_| {
                get_called = true;
                Ok((256, 1024))
            },
            |_, _, _| {
                set_called = true;
                Ok(())
            },
        );
        assert!(!get_called);
        assert!(!set_called);
    }

    #[test]
    fn setrlimit_failure_is_handled_gracefully() {
        let mut set_called = false;
        apply_soft_nofile_limit_with_sources(
            || Some(61440),
            |_| Ok((256, 4096)),
            |_, soft, hard| {
                set_called = true;
                assert_eq!(soft, 4096);
                assert_eq!(hard, 4096);
                Err("EINVAL".to_string())
            },
        );
        assert!(set_called);
    }
}
