// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Operational door state beneath the service's original journal capability.

use serde::Serialize;
use solstone_core_journal_io::{JournalRoot, journal_directory::JournalNamespace};
#[cfg(windows)]
use std::ffi::OsStr;
use std::{io, path::Path, sync::Arc};

pub(crate) struct DoorStateWriter {
    namespace: JournalNamespace,
}
impl DoorStateWriter {
    pub(crate) fn new(root: Arc<JournalRoot>) -> Self {
        Self {
            namespace: JournalNamespace::new(root),
        }
    }
    pub(crate) fn root(&self) -> &Arc<JournalRoot> {
        self.namespace.root()
    }
    pub(crate) fn publish_local(&self, state: &crate::local_door::LocalDoorState) {
        Self::report(self.publish(crate::local_door::LOCAL_DOOR_STATE_PATH, state));
    }
    pub(crate) fn publish_lan(&self, state: &crate::lan_door::LanDoorState) {
        Self::report(self.publish(crate::lan_door::LAN_DOOR_STATE_PATH, state));
    }
    pub(crate) fn publish_byo(&self, state: &crate::byo_door::ByoDoorState) {
        Self::report(self.publish(crate::byo_door::BYO_DOOR_STATE_PATH, state));
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

#[cfg(all(test, feature = "full-tests"))]
mod tests {
    use super::*;
    use serde_json::json;
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
