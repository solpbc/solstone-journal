// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Operational door state beneath the service's original journal capability.

use serde::Serialize;
use solstone_core_journal_io::{JournalRoot, journal_directory::JournalNamespace};
#[cfg(windows)]
use std::ffi::OsStr;
use std::{
    io,
    path::Path,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

// Only publication history: never substitutes for current admission or root checks.
struct DisabledPublication<T> {
    last: Option<(T, Instant)>,
}
impl<T: PartialEq> DisabledPublication<T> {
    fn publish(
        &mut self,
        key: T,
        now: Instant,
        heartbeat: Duration,
        validate: impl FnOnce() -> io::Result<()>,
        write: impl FnOnce() -> io::Result<()>,
    ) -> io::Result<()> {
        let unchanged = self
            .last
            .as_ref()
            .is_some_and(|(previous, at)| *previous == key && now.duration_since(*at) < heartbeat);
        if unchanged {
            let result = validate();
            if result.is_err() {
                self.last = None;
            }
            return result;
        }
        self.last = None;
        write()?;
        self.last = Some((key, now));
        Ok(())
    }
}

pub(crate) struct DoorStateWriter {
    namespace: JournalNamespace,
    lan_disabled: Mutex<DisabledPublication<crate::lan_door::LanDoorState>>,
    byo_disabled: Mutex<DisabledPublication<crate::byo_door::ByoDoorState>>,
}
impl DoorStateWriter {
    pub(crate) fn new(root: Arc<JournalRoot>) -> Self {
        Self {
            namespace: JournalNamespace::new(root),
            lan_disabled: Mutex::new(DisabledPublication { last: None }),
            byo_disabled: Mutex::new(DisabledPublication { last: None }),
        }
    }
    pub(crate) fn root(&self) -> &Arc<JournalRoot> {
        self.namespace.root()
    }
    pub(crate) fn publish_local(&self, state: &crate::local_door::LocalDoorState) {
        Self::report(self.publish(crate::local_door::LOCAL_DOOR_STATE_PATH, state));
    }
    pub(crate) fn publish_lan(&self, state: &crate::lan_door::LanDoorState) {
        let mut history = self.lan_disabled.lock().expect("LAN publication lock");
        history.last = None;
        Self::report(self.publish(crate::lan_door::LAN_DOOR_STATE_PATH, state));
    }
    pub(crate) fn publish_byo(&self, state: &crate::byo_door::ByoDoorState) {
        let mut history = self.byo_disabled.lock().expect("BYO publication lock");
        history.last = None;
        Self::report(self.publish(crate::byo_door::BYO_DOOR_STATE_PATH, state));
    }
    pub(crate) fn publish_lan_disabled(
        &self,
        state: &crate::lan_door::LanDoorState,
        heartbeat: Duration,
    ) {
        let mut key = state.clone();
        key.observed_at = chrono::DateTime::UNIX_EPOCH;
        Self::report(
            self.lan_disabled
                .lock()
                .expect("LAN publication lock")
                .publish(
                    key,
                    Instant::now(),
                    heartbeat,
                    || self.namespace.revalidate(),
                    || self.publish(crate::lan_door::LAN_DOOR_STATE_PATH, state),
                ),
        );
    }
    pub(crate) fn publish_byo_disabled(&self, state: &crate::byo_door::ByoDoorState) {
        let mut key = state.clone();
        key.observed_at = chrono::DateTime::UNIX_EPOCH;
        Self::report(
            self.byo_disabled
                .lock()
                .expect("BYO publication lock")
                .publish(
                    key,
                    Instant::now(),
                    Duration::from_secs(1),
                    || self.namespace.revalidate(),
                    || self.publish(crate::byo_door::BYO_DOOR_STATE_PATH, state),
                ),
        );
    }
    fn report(result: io::Result<()>) {
        if result.is_err() {
            log::warn!("door state publication failed");
        }
    }
    fn publish<T: Serialize>(&self, relative: &str, state: &T) -> io::Result<()> {
        self.namespace.revalidate()?;
        let path = Path::new(relative);
        let parent = self
            .namespace
            .directory(path.parent().expect("fixed state parent"), true)?
            .expect("created state parent");
        let name = path.file_name().expect("fixed state filename");
        let mut bytes = serde_json::to_vec_pretty(state)?;
        bytes.push(b'\n');
        let check = || {
            self.namespace
                .revalidate()
                .map_err(|error| error.to_string())
        };
        #[cfg(unix)]
        match solstone_core_journal_io::atomic_replace_bound_checked(
            parent.as_ref(),
            name,
            &bytes,
            0o600,
            &check,
        )
        .map_err(io::Error::other)?
        {
            solstone_core_journal_io::BoundAtomicOutcome::Published { .. } => {}
            outcome => {
                return Err(io::Error::other(format!(
                    "door state publication uncertain: {outcome:?}"
                )));
            }
        }
        #[cfg(windows)]
        {
            let lock = solstone_core_journal_io::acquire_existing_parent_lock_bound_checked(
                parent.as_ref(),
                OsStr::new(".door-state-publication.lock"),
                solstone_core_journal_io::DEFAULT_LOCK_TIMEOUT,
                solstone_core_journal_io::DEFAULT_LOCK_POLL_INTERVAL,
                &check,
            )
            .map_err(io::Error::other)?;
            match solstone_core_journal_io::atomic_replace_detailed_bound_checked(
                parent.as_ref(),
                &lock,
                name,
                &bytes,
                0o600,
                &check,
            )
            .map_err(io::Error::other)?
            {
                solstone_core_journal_io::DetailedAtomicOutcome::Published => {}
                outcome => {
                    return Err(io::Error::other(format!(
                        "door state publication uncertain: {outcome:?}"
                    )));
                }
            }
        }
        self.namespace.revalidate()
    }
}

#[cfg(test)]
mod publication_tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn disabled_history_preserves_changes_heartbeat_and_fresh_checks() {
        let mut history = DisabledPublication { last: None };
        let writes = Cell::new(0);
        let checks = Cell::new(0);
        let start = Instant::now();
        let mut publish = |key, elapsed| {
            history.publish(
                key,
                start + Duration::from_millis(elapsed),
                Duration::from_secs(1),
                || {
                    checks.set(checks.get() + 1);
                    Ok(())
                },
                || {
                    writes.set(writes.get() + 1);
                    Ok(())
                },
            )
        };
        publish(("host-a", 7), 0).unwrap();
        publish(("host-a", 7), 500).unwrap();
        assert_eq!((writes.get(), checks.get()), (1, 1));
        publish(("host-a", 7), 1000).unwrap();
        assert_eq!(writes.get(), 2);
        publish(("host-b", 7), 1001).unwrap();
        publish(("host-b", 8), 1002).unwrap();
        assert_eq!(writes.get(), 4);
    }

    #[test]
    fn failed_publication_or_validation_never_suppresses_retry() {
        let mut history = DisabledPublication { last: None };
        let now = Instant::now();
        let heartbeat = Duration::from_secs(10);
        assert!(
            history
                .publish(
                    7,
                    now,
                    heartbeat,
                    || Ok(()),
                    || Err(io::Error::other("write failed"))
                )
                .is_err()
        );
        assert!(history.last.is_none());
        history
            .publish(7, now, heartbeat, || panic!("must retry write"), || Ok(()))
            .unwrap();
        assert!(
            history
                .publish(
                    7,
                    now,
                    heartbeat,
                    || Err(io::Error::other("root replaced")),
                    || panic!("must refuse")
                )
                .is_err()
        );
        assert!(history.last.is_none());
        history
            .publish(7, now, heartbeat, || panic!("must retry write"), || Ok(()))
            .unwrap();
    }
}

#[cfg(all(test, feature = "full-tests"))]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn disabled_writer_forces_errors_stop_and_retries_failed_writes() {
        let temporary = tempfile::tempdir().unwrap();
        let writer = DoorStateWriter::new(Arc::new(JournalRoot::open(temporary.path()).unwrap()));
        let disabled = crate::lan_door::LanDoorState {
            listening: false,
            observed_at: chrono::Utc::now(),
            reason: Some("disabled".to_owned()),
            fingerprint: None,
            addresses: None,
        };
        let path = temporary.path().join(crate::lan_door::LAN_DOOR_STATE_PATH);
        writer.publish_lan_disabled(&disabled, Duration::from_secs(10));
        let first = std::fs::read(&path).unwrap();
        let mut next = disabled.clone();
        next.observed_at += chrono::Duration::milliseconds(1);
        writer.publish_lan_disabled(&next, Duration::from_secs(10));
        assert_eq!(std::fs::read(&path).unwrap(), first);
        for reason in ["config_invalid", "not_running"] {
            let mut terminal = next.clone();
            terminal.reason = Some(reason.to_owned());
            writer.publish_lan(&terminal);
            assert!(writer.lan_disabled.lock().unwrap().last.is_none());
            assert_eq!(
                crate::lan_door::read_lan_door_state(temporary.path()),
                Some(terminal)
            );
            writer.publish_lan_disabled(&next, Duration::from_secs(10));
            assert_eq!(
                crate::lan_door::read_lan_door_state(temporary.path()),
                Some(next.clone())
            );
        }
        // A directory occupying the publication leaf makes the real primitive
        // fail. Removing that collision must allow the very next pass to retry.
        writer.publish_lan(&next);
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        writer.publish_lan_disabled(&next, Duration::from_secs(10));
        assert!(writer.lan_disabled.lock().unwrap().last.is_none());
        std::fs::remove_dir(&path).unwrap();
        writer.publish_lan_disabled(&next, Duration::from_secs(10));
        assert_eq!(
            crate::lan_door::read_lan_door_state(temporary.path()),
            Some(next)
        );
    }

    #[test]
    fn retained_state_writer_preserves_original_terminal_and_refuses_replacements() {
        for replace_root in [true, false] {
            let temporary = tempfile::tempdir().unwrap();
            let journal = temporary.path().join("journal");
            std::fs::create_dir(&journal).unwrap();
            let writer = DoorStateWriter::new(Arc::new(JournalRoot::open(&journal).unwrap()));
            for name in [
                "local-door-state.json",
                "lan-door-state.json",
                "byo-door-state.json",
            ] {
                writer
                    .publish(
                        &format!("mcp-endpoint/{name}"),
                        &json!({"listening":false,"reason":"not_running"}),
                    )
                    .unwrap();
            }
            let slot = if replace_root {
                journal.clone()
            } else {
                journal.join("mcp-endpoint")
            };
            let retained = temporary.path().join("retained");
            if let Err(error) = std::fs::rename(&slot, &retained) {
                assert!(
                    cfg!(windows) && matches!(error.raw_os_error(), Some(5 | 32)),
                    "move admitted namespace: {error}"
                );
                // Retained Windows publication ancestors can prevent the
                // replacement itself. Confirm that stronger native outcome,
                // including a successful rename after the owner closes.
                assert!(!retained.exists());
                writer.root().revalidate_canonical_binding().unwrap();
                for name in [
                    "local-door-state.json",
                    "lan-door-state.json",
                    "byo-door-state.json",
                ] {
                    let state: serde_json::Value = serde_json::from_slice(
                        &std::fs::read(journal.join("mcp-endpoint").join(name)).unwrap(),
                    )
                    .unwrap();
                    assert_eq!(state["listening"], false);
                    assert_eq!(state["reason"], "not_running");
                }
                drop(writer);
                std::fs::rename(&slot, &retained).expect("closed owner releases namespace");
                continue;
            }
            std::fs::create_dir(&slot).unwrap();
            for name in [
                "local-door-state.json",
                "lan-door-state.json",
                "byo-door-state.json",
            ] {
                assert!(
                    writer
                        .publish(&format!("mcp-endpoint/{name}"), &json!({"listening":true}))
                        .is_err()
                );
                let original = if replace_root {
                    retained.join("mcp-endpoint").join(name)
                } else {
                    retained.join(name)
                };
                let state: serde_json::Value =
                    serde_json::from_slice(&std::fs::read(original).unwrap()).unwrap();
                assert_eq!(state["listening"], false);
            }
            assert_eq!(std::fs::read_dir(slot).unwrap().count(), 0);
        }
    }
}
