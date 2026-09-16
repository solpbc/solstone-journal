// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The durability class of every artifact the supervisor reads, and the one
//! reader that enforces it.
//!
//! The rule, verbatim from the owner of the product: *anything that bricks
//! the supervisor is a hard blocker, except the single config JSON, which
//! must be valid and only ever atomically written. Every other file the
//! journal reads must be a regenerable cache, acceptable to wipe and start
//! over, or have a routine to fix or heal it.*
//!
//! [`read_json_durable`] is how a read site says which of those it is. A
//! `MustBeValid` artifact that cannot be parsed is an error, as it always was.
//! Anything else that cannot be parsed is set aside beside itself as
//! `<name>.wedged-<stamp><ext>`, never deleted, and reads as absent, so the
//! next write rebuilds it and `journal doctor` can name what was set aside.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::de::DeserializeOwned;

/// What the journal may do with an artifact it cannot read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DurabilityClass {
    /// The owner's configuration. Written atomically, never healed: an
    /// unreadable one is an error the owner has to see.
    MustBeValid,
    /// Derived state the next run rebuilds from its sources.
    RegenerableCache,
    /// Bookkeeping whose loss costs at most some repeated work.
    Wipeable,
    /// State with its own repair routine; the reader sets it aside and the
    /// routine rebuilds from what survives.
    Healable,
}

/// One artifact the supervisor reads, with the class it was declared under.
#[derive(Debug, Clone, Copy)]
pub struct DurableArtifact {
    /// Relative to the journal root; `*` stands for one path segment.
    pub path: &'static str,
    pub class: DurabilityClass,
    /// Why it is that class, in one line.
    pub rationale: &'static str,
}

/// The declared inventory for the supervisor's boot and tick paths. Every
/// entry names the routine that stands behind its class.
pub const SUPERVISOR_ARTIFACTS: &[DurableArtifact] = &[
    DurableArtifact {
        path: "config/journal.json",
        class: DurabilityClass::MustBeValid,
        rationale: "the owner's configuration; written under a lock with atomic replace",
    },
    DurableArtifact {
        path: "config/schedules.json",
        class: DurabilityClass::Healable,
        rationale: "an unreadable file falls back to the default schedules with a diagnostic",
    },
    DurableArtifact {
        path: "health/scheduler.json",
        class: DurabilityClass::RegenerableCache,
        rationale: "last-run bookkeeping; an unreadable file reads as an empty map",
    },
    DurableArtifact {
        path: "health/direct-door.json",
        class: DurabilityClass::RegenerableCache,
        rationale: "rewritten unconditionally at every boot",
    },
    DurableArtifact {
        path: "health/catchup-state.json",
        class: DurabilityClass::Wipeable,
        rationale: "retry attempts and backoff watermarks; set aside and rebuilt by the next write",
    },
    DurableArtifact {
        path: "health/daily-adoption.json",
        class: DurabilityClass::Wipeable,
        rationale: "adoption bookkeeping; the accepted unit records are the authority",
    },
    DurableArtifact {
        path: "health/parent-loss/active-generation.json",
        class: DurabilityClass::Healable,
        rationale: "set aside; the successor is allocated above every generation on disk and the open one is closed from its admissions",
    },
    DurableArtifact {
        path: "health/parent-loss/generations/*/record.json",
        class: DurabilityClass::Healable,
        rationale: "set aside; the generation is closed from its admissions directory",
    },
    DurableArtifact {
        path: "health/sync/*.check",
        class: DurabilityClass::Wipeable,
        rationale: "heartbeats; a dead run's is retired by its successor or the stale collector",
    },
    DurableArtifact {
        path: "health/callosum.sock",
        class: DurabilityClass::Wipeable,
        rationale: "a stale socket is removed before bind",
    },
];

/// What a durable read found.
#[derive(Debug)]
pub enum DurableRead<T> {
    Present(T),
    Absent,
    /// The artifact could not be parsed and was set aside at this path.
    SetAside(PathBuf),
}

/// Read a JSON artifact under its declared class.
pub fn read_json_durable<T: DeserializeOwned>(
    path: &Path,
    class: DurabilityClass,
) -> io::Result<DurableRead<T>> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(DurableRead::Absent),
        Err(error) => return Err(error),
    };
    match serde_json::from_slice(&bytes) {
        Ok(value) => Ok(DurableRead::Present(value)),
        Err(error) if class == DurabilityClass::MustBeValid => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{}: {error}", path.display()),
        )),
        Err(_) => set_aside(path).map(DurableRead::SetAside),
    }
}

/// Move an artifact beside itself as `<stem>.wedged-<unix seconds><ext>` and
/// return the new path. Never deletes.
pub fn set_aside(path: &Path) -> io::Result<PathBuf> {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or_default();
    let stem = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("artifact");
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| format!(".{extension}"))
        .unwrap_or_default();
    let mut aside = path.with_file_name(format!("{stem}.wedged-{stamp}{extension}"));
    let mut sequence = 1;
    while aside.exists() {
        aside = path.with_file_name(format!("{stem}.wedged-{stamp}-{sequence}{extension}"));
        sequence += 1;
    }
    fs::rename(path, &aside)?;
    Ok(aside)
}

/// The marker every set-aside artifact carries in its name.
pub const SET_ASIDE_MARKER: &str = ".wedged-";

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn a_readable_artifact_is_present_under_every_class() {
        let directory = TempDir::new().expect("temporary root");
        let path = directory.path().join("state.json");
        fs::write(&path, b"{\"n\":1}").expect("write");
        for class in [
            DurabilityClass::MustBeValid,
            DurabilityClass::RegenerableCache,
            DurabilityClass::Wipeable,
            DurabilityClass::Healable,
        ] {
            let read = read_json_durable::<serde_json::Value>(&path, class).expect("read");
            assert!(matches!(read, DurableRead::Present(value) if value["n"] == 1));
        }
        assert!(matches!(
            read_json_durable::<serde_json::Value>(
                &directory.path().join("missing.json"),
                DurabilityClass::Wipeable
            )
            .expect("read"),
            DurableRead::Absent
        ));
    }

    #[test]
    fn the_config_class_alone_refuses_an_unreadable_artifact() {
        let directory = TempDir::new().expect("temporary root");
        let path = directory.path().join("journal.json");
        fs::write(&path, b"{ not json").expect("write");
        let error = read_json_durable::<serde_json::Value>(&path, DurabilityClass::MustBeValid)
            .expect_err("must-be-valid refuses");
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(path.is_file(), "the config is never moved");
    }

    #[test]
    fn every_other_class_sets_an_unreadable_artifact_aside_and_keeps_it() {
        for class in [
            DurabilityClass::RegenerableCache,
            DurabilityClass::Wipeable,
            DurabilityClass::Healable,
        ] {
            let directory = TempDir::new().expect("temporary root");
            let path = directory.path().join("catchup-state.json");
            fs::write(&path, b"{ not json").expect("write");
            let read =
                read_json_durable::<serde_json::Value>(&path, class).expect("set aside, not error");
            let DurableRead::SetAside(aside) = read else {
                panic!("expected a set-aside read for {class:?}");
            };
            assert!(
                !path.exists(),
                "the damaged artifact no longer sits at its path"
            );
            assert!(aside.is_file(), "the damaged bytes are preserved");
            let name = aside.file_name().unwrap().to_str().unwrap();
            assert!(name.starts_with("catchup-state.wedged-") && name.ends_with(".json"));
            assert_eq!(fs::read(&aside).expect("preserved bytes"), b"{ not json");
            assert!(matches!(
                read_json_durable::<serde_json::Value>(&path, class).expect("second read"),
                DurableRead::Absent
            ));
        }
    }

    #[test]
    fn a_second_set_aside_in_the_same_second_does_not_overwrite_the_first() {
        let directory = TempDir::new().expect("temporary root");
        let path = directory.path().join("state.json");
        fs::write(&path, b"one").expect("write");
        let first = set_aside(&path).expect("first");
        fs::write(&path, b"two").expect("write again");
        let second = set_aside(&path).expect("second");
        assert_ne!(first, second);
        assert_eq!(fs::read(&first).unwrap(), b"one");
        assert_eq!(fs::read(&second).unwrap(), b"two");
    }

    #[test]
    fn the_inventory_declares_exactly_one_must_be_valid_artifact() {
        let strict: Vec<_> = SUPERVISOR_ARTIFACTS
            .iter()
            .filter(|artifact| artifact.class == DurabilityClass::MustBeValid)
            .map(|artifact| artifact.path)
            .collect();
        assert_eq!(strict, vec!["config/journal.json"]);
        for artifact in SUPERVISOR_ARTIFACTS {
            assert!(
                !artifact.rationale.is_empty(),
                "{} has no rationale",
                artifact.path
            );
        }
    }
}
