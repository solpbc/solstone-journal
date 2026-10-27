// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::sync::Arc;

use solstone_core_backup::Destination;
use solstone_core_backup_runtime::{BackupServices, restore::MobileRestoreIdentity};
use solstone_core_journal_io::JournalRoot;

use crate::{BackupWebDeps, operation::Terminal};

/// The embedding host owns admitted-work drain and reusable service resumption.
pub trait EmbeddedRestoreLifecycle: Send + Sync {
    fn drain(&self, root: &Arc<JournalRoot>) -> Result<Box<dyn PausedRestore>, String>;
}

/// An owned paused attempt. Drop releases its process hold without resuming;
/// the durable restore record remains authoritative after failure or unwind.
pub trait PausedRestore: Send {
    fn resume(self: Box<Self>, cancelled: &dyn Fn() -> bool) -> Result<(), String>;
}

#[derive(Clone)]
pub(crate) enum RestoreHost {
    Desktop,
    Embedded(Option<Arc<dyn EmbeddedRestoreLifecycle>>),
}

pub(crate) fn run(
    deps: &BackupWebDeps,
    services: &BackupServices<'_>,
    destination: Destination,
    key: &str,
    identity: MobileRestoreIdentity,
) -> Terminal {
    match &deps.restore_host {
        RestoreHost::Desktop => crate::map_restore(solstone_core_backup_runtime::restore_journal(
            &deps.journal_root,
            services,
            deps.restore_recorder.as_ref(),
            destination,
            key,
        )),
        RestoreHost::Embedded(None) => Terminal::error("destination_admission_failed"),
        RestoreHost::Embedded(Some(host)) => {
            let root = match deps.retained_root() {
                Ok(root) => root,
                Err(_) => return Terminal::error("destination_admission_failed"),
            };
            let mut paused = None;
            let outcome = solstone_core_backup_runtime::restore::restore_mobile_latest_journal(
                root.clone(),
                services,
                deps.restore_recorder.as_ref(),
                destination,
                key,
                identity,
                &mut || {
                    if paused.is_some() {
                        return Err("restore attempt has already drained".into());
                    }
                    paused = Some(host.drain(&root)?);
                    Ok(())
                },
            );
            let ready = matches!(outcome.status.as_str(), "ok" | "degraded");
            if ready
                && services
                    .runner
                    .cancellation()
                    .is_some_and(|stop| stop.is_cancelled())
            {
                drop(paused);
                return Terminal::error("restore_failed");
            }
            if ready {
                let Some(attempt) = paused.take() else {
                    return Terminal::error("destination_admission_failed");
                };
                if attempt
                    .resume(&|| {
                        services
                            .runner
                            .cancellation()
                            .is_some_and(|stop| stop.is_cancelled())
                    })
                    .is_err()
                {
                    return Terminal::error("destination_admission_failed");
                }
            }
            drop(paused);
            crate::map_restore(outcome)
        }
    }
}
