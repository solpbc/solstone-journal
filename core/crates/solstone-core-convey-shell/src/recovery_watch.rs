// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! An owned wake-up bridge for consumers of the shared recovery admission.
//! The watch is a change signal, never an independent readiness flag. Keeping
//! it in the consumer future avoids a detached Condvar waiter at Stop.

use std::sync::Arc;

use solstone_core_journal_io::JournalRoot;
use tokio::sync::watch;

use crate::recovery_drain::{RecoveryHooks, register_recovery_hooks};

pub struct RecoveryWatch {
    changes: watch::Receiver<()>,
    _hooks: RecoveryHooks,
}

impl RecoveryWatch {
    pub fn new(id: &str, root: Arc<JournalRoot>) -> Self {
        let (pause, changes) = watch::channel(());
        let resume = pause.clone();
        let hooks = register_recovery_hooks(
            id,
            root,
            move |_| {
                pause.send_replace(());
                Ok(())
            },
            move |_| {
                resume.send_replace(());
                Ok(())
            },
        );
        Self {
            changes,
            _hooks: hooks,
        }
    }

    pub fn subscribe(&self) -> watch::Receiver<()> {
        self.changes.clone()
    }
}
