// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Owner mutations of the shared schedules map under retained directory authority.

use super::{ScheduleError, ScheduleMutation};
use serde_json::{Map, Value};
use solstone_core_journal_io::journal_directory::JournalNamespace;
use solstone_core_journal_io::{JournalRoot, LockOptions};
use std::{ffi::OsStr, io::Read, path::Path, sync::Arc};

/// Mutate the same raw map and stable sidecar as the pathname entry, retaining
/// the root and every ancestor through lock, read and publication. Malformed
/// owner mutations refuse without moving their source; runtime healing keeps
/// its existing separate entry point.
pub fn mutate_schedule_entries_at_root_checked<T>(
    root: Arc<JournalRoot>,
    checkpoint: &dyn Fn() -> Result<(), String>,
    mutator: impl FnOnce(&mut Map<String, Value>) -> ScheduleMutation<T>,
) -> Result<T, ScheduleError> {
    let namespace = JournalNamespace::new(root);
    let check = || {
        checkpoint()?;
        namespace.revalidate().map_err(|error| error.to_string())
    };
    check().map_err(ScheduleError::Io)?;
    let parent = namespace
        .directory(Path::new("config"), true)
        .map_err(io_error)?
        .expect("created schedule parent");
    let diagnostic = namespace.root().canonical_path().join("config");
    let name = OsStr::new("schedules.json");
    let _lock = solstone_core_journal_io::hold_lock_bound_checked(
        parent.as_ref(),
        name,
        &diagnostic,
        LockOptions::default(),
        false,
        &check,
    )
    .map_err(io_error)?;
    check().map_err(ScheduleError::Io)?;
    #[cfg(unix)]
    let file = solstone_core_journal_io::flat_directory::open_regular_file_from_bound_parent(
        parent.as_ref(),
        name,
    );
    #[cfg(windows)]
    let file =
        solstone_core_journal_io::windows_sync_dir::open_windows_regular_file_from_bound_parent(
            parent.as_ref(),
            name,
            &diagnostic,
        );
    let mut file = file.map_err(io_error)?;
    #[cfg(unix)]
    let mode = {
        use std::os::unix::fs::PermissionsExt;
        file.as_ref()
            .map(|file| {
                file.metadata()
                    .map(|metadata| metadata.permissions().mode() & 0o777)
            })
            .transpose()
            .map_err(io_error)?
            .unwrap_or(0o600)
    };
    #[cfg(windows)]
    let mode = 0o600;
    let mut bytes = Vec::new();
    if let Some(file) = &mut file {
        file.read_to_end(&mut bytes).map_err(io_error)?;
    }
    let mut raw = if bytes.trim_ascii().is_empty() {
        Map::new()
    } else {
        match serde_json::from_slice(&bytes) {
            Ok(Value::Object(raw)) => raw,
            _ => {
                return Err(ScheduleError::MalformedConfig {
                    path: diagnostic.join(name),
                });
            }
        }
    };
    check().map_err(ScheduleError::Io)?;
    let mutation = mutator(&mut raw);
    check().map_err(ScheduleError::Io)?;
    if !mutation.changed {
        return Ok(mutation.value);
    }
    let mut bytes = serde_json::to_vec_pretty(&Value::Object(raw)).map_err(io_error)?;
    bytes.push(b'\n');
    #[cfg(unix)]
    match solstone_core_journal_io::atomic_replace_bound_checked(
        parent.as_ref(),
        name,
        &bytes,
        mode,
        &check,
    )
    .map_err(io_error)?
    {
        solstone_core_journal_io::BoundAtomicOutcome::Published { .. } => {}
        solstone_core_journal_io::BoundAtomicOutcome::PublishedDurabilityUncertain {
            source,
            ..
        } => {
            return Err(ScheduleError::Io(format!(
                "schedules published; durability uncertain: {source}"
            )));
        }
        solstone_core_journal_io::BoundAtomicOutcome::PublishedObservationUncertain {
            source,
            ..
        } => {
            return Err(ScheduleError::Io(format!(
                "schedules published; observation uncertain: {source}"
            )));
        }
    }
    #[cfg(windows)]
    {
        let lock = solstone_core_journal_io::acquire_existing_parent_lock_bound_checked(
            parent.as_ref(),
            OsStr::new(".schedules-publication.lock"),
            solstone_core_journal_io::DEFAULT_LOCK_TIMEOUT,
            solstone_core_journal_io::DEFAULT_LOCK_POLL_INTERVAL,
            &check,
        )
        .map_err(io_error)?;
        match solstone_core_journal_io::atomic_replace_detailed_bound_checked(
            parent.as_ref(),
            &lock,
            name,
            &bytes,
            mode,
            &check,
        )
        .map_err(io_error)?
        {
            solstone_core_journal_io::DetailedAtomicOutcome::Published => {}
            solstone_core_journal_io::DetailedAtomicOutcome::PublishedDurabilityUncertain {
                source,
            } => {
                return Err(ScheduleError::Io(format!(
                    "schedules published; durability uncertain: {source}"
                )));
            }
            _ => {
                return Err(ScheduleError::Io(
                    "schedules published; observation uncertain".to_owned(),
                ));
            }
        }
    }
    namespace.revalidate().map_err(io_error)?;
    Ok(mutation.value)
}

fn io_error(error: impl std::fmt::Display) -> ScheduleError {
    ScheduleError::Io(error.to_string())
}

#[cfg(any(unix, windows))]
#[cfg(all(test, feature = "full-tests"))]
mod tests {
    use super::*;
    use serde_json::json;
    use std::fs;

    #[test]
    fn bound_mutation_preserves_choices_noops_and_malformed_source() {
        let temporary = tempfile::tempdir().unwrap();
        fs::create_dir(temporary.path().join("config")).unwrap();
        let path = temporary.path().join("config/schedules.json");
        let original = br#"{"daily_time":"00:15", "custom":{"cmd":["custom-owner-command"],"every":"daily","extra":"kept"}}"#;
        fs::write(&path, original).unwrap();
        let root = Arc::new(JournalRoot::open(temporary.path()).unwrap());
        mutate_schedule_entries_at_root_checked(Arc::clone(&root), &|| Ok(()), |_| {
            ScheduleMutation {
                changed: false,
                value: (),
            }
        })
        .unwrap();
        assert_eq!(fs::read(&path).unwrap(), original);
        #[cfg(unix)]
        let mode = {
            use std::os::unix::fs::PermissionsExt;
            fs::metadata(path.with_file_name("schedules.json.lock"))
                .unwrap()
                .permissions()
                .mode()
        };
        mutate_schedule_entries_at_root_checked(Arc::clone(&root), &|| Ok(()), |raw| {
            raw.insert("source".into(), json!({"enabled":false}));
            ScheduleMutation {
                changed: true,
                value: (),
            }
        })
        .unwrap();
        let updated: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(updated["daily_time"], "00:15");
        assert_eq!(updated["custom"]["extra"], "kept");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(path.with_file_name("schedules.json.lock"))
                    .unwrap()
                    .permissions()
                    .mode(),
                mode
            );
        }
        fs::write(&path, b"{corrupt owner schedules").unwrap();
        assert!(
            mutate_schedule_entries_at_root_checked::<()>(root, &|| Ok(()), |_| panic!(
                "corrupt schedules must not reach mutation"
            ))
            .is_err()
        );
        assert_eq!(fs::read(path).unwrap(), b"{corrupt owner schedules");
    }

    #[test]
    fn root_and_nested_parent_replacements_receive_no_schedule_or_lock_entry() {
        for replace_root in [true, false] {
            let temporary = tempfile::tempdir().unwrap();
            let journal = temporary.path().join("journal");
            fs::create_dir_all(journal.join("config")).unwrap();
            let original = br#"{"daily_time":"00:15","custom":{"cmd":["owner"],"every":"daily"}}"#;
            fs::write(journal.join("config/schedules.json"), original).unwrap();
            let root = Arc::new(JournalRoot::open(&journal).unwrap());
            let slot = if replace_root {
                journal.clone()
            } else {
                journal.join("config")
            };
            let retained = temporary.path().join("retained");
            let result = mutate_schedule_entries_at_root_checked(root, &|| Ok(()), |raw| {
                fs::rename(&slot, &retained).unwrap();
                fs::create_dir(&slot).unwrap();
                raw.insert("daily_time".into(), json!("04:00"));
                ScheduleMutation {
                    changed: true,
                    value: (),
                }
            });
            assert!(result.is_err());
            assert_eq!(fs::read_dir(&slot).unwrap().count(), 0);
            let saved = if replace_root {
                retained.join("config/schedules.json")
            } else {
                retained.join("schedules.json")
            };
            assert_eq!(fs::read(saved).unwrap(), original);
        }
    }
}
