// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::future::Future;
use std::sync::Mutex;

use tokio::sync::watch;
use tokio::task::JoinHandle;

use super::{DoorLifecycle, Ordering};

struct State {
    stopped: bool,
    tasks: Vec<JoinHandle<()>>,
}

pub(super) struct Carriers {
    stop: watch::Sender<bool>,
    state: Mutex<State>,
}

impl Carriers {
    pub(super) fn new() -> Self {
        let (stop, _) = watch::channel(false);
        Self {
            stop,
            state: Mutex::new(State {
                stopped: false,
                tasks: Vec::new(),
            }),
        }
    }

    pub(super) fn spawn(&self, carrier: impl Future<Output = ()> + Send + 'static) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.stopped {
            return;
        }
        state.tasks.retain(|task| !task.is_finished());
        let mut stop = self.stop.subscribe();
        state.tasks.push(tokio::spawn(async move {
            if *stop.borrow() {
                return;
            }
            tokio::select! {
                () = carrier => {},
                _ = stop.changed() => {},
            }
        }));
    }

    pub(super) fn request_stop(&self) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.stopped = true;
        self.stop.send_replace(true);
    }

    pub(super) async fn stop_and_join(&self) {
        self.request_stop();
        let tasks = {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            std::mem::take(&mut state.tasks)
        };
        for task in tasks {
            let _ = task.await;
        }
    }

    pub(super) fn resume(&self) -> Result<(), String> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| "paired door is unavailable")?;
        if !state.tasks.is_empty() {
            return Err("paired door has not drained".into());
        }
        state.stopped = false;
        self.stop.send_replace(false);
        Ok(())
    }
}

impl DoorLifecycle {
    fn check_recovery_root(&self, journal: &std::path::Path) -> Result<(), String> {
        self.target_use
            .revalidate()
            .map_err(|error| error.to_string())?;
        if journal != self.parts.journal_root {
            return Err("paired door is bound to a different journal".into());
        }
        Ok(())
    }

    pub(crate) async fn pause_for_recovery(&self, journal: &std::path::Path) -> Result<(), String> {
        self.check_recovery_root(journal)?;
        let _rebinding = self.rebinding.lock().await;
        self.recovery_paused.store(true, Ordering::Release);
        let previous = self.running.lock().expect("door lifecycle lock").take();
        if let Some(previous) = previous {
            previous.retire().await;
        }
        self.loopback_carriers.stop_and_join().await;
        self.parts.relay_admissions.clear_door_port();
        self.check_recovery_root(journal)
    }

    pub(crate) async fn resume_after_recovery(
        &self,
        journal: &std::path::Path,
    ) -> Result<(), String> {
        use solstone_core_journal_config_write::{BoundConfigOutcome, DirectoryFlush};
        use solstone_core_journal_io::JournalRoot;
        use std::sync::Arc;

        let _rebinding = self.rebinding.lock().await;
        let _target_use = self.target_use.begin().map_err(|error| error.to_string())?;
        let root = Arc::new(JournalRoot::open(journal).map_err(|error| error.to_string())?);
        let check = || {
            self.check_recovery_root(journal)?;
            if self.stopped.load(Ordering::Acquire) {
                return Err("paired door has stopped".into());
            }
            if solstone_core_journal_io::whole_journal_restore::whole_journal_restore_blocks(
                journal,
            ) {
                return Err("whole-journal restore is pending".into());
            }
            Ok(())
        };
        check()?;
        let port = self.selected_port.load(Ordering::Acquire);
        if port == 0 {
            return Err("paired door has no selected port".into());
        }
        let transaction =
            solstone_core_journal_config_write::persist_direct_door_port_at_root_checked(
                &root, port, &check,
            )
            .map_err(|error| error.to_string())?;
        match transaction.outcome {
            BoundConfigOutcome::Unchanged => {}
            BoundConfigOutcome::Published(publication)
                if publication.directory_flush == DirectoryFlush::Synced
                    && publication.durability_uncertainty.is_none()
                    && publication.observation_uncertainty.is_none()
                    && publication.confirmation_refusal.is_none() => {}
            BoundConfigOutcome::Published(_) => {
                return Err("bound port publication is uncertain".into());
            }
        }
        check()?;
        self.loopback_carriers.resume()?;
        self.recovery_paused.store(false, Ordering::Release);
        let mut options = self.parts.to_options();
        options.port = port;
        let started =
            super::start_with_carriers(options, Arc::clone(&self.loopback_carriers)).await;
        if !self.install_started(started).await {
            self.recovery_paused.store(true, Ordering::Release);
            self.loopback_carriers.stop_and_join().await;
            return Err("paired door did not resume".into());
        }
        self.publish_record().map_err(|error| error.to_string())?;
        check()
    }

    pub(crate) async fn shutdown_and_wait(&self) {
        self.shutdown();
        let _rebinding = self.rebinding.lock().await;
        let previous = self.running.lock().expect("door lifecycle lock").take();
        if let Some(previous) = previous {
            previous.retire().await;
        }
        self.loopback_carriers.stop_and_join().await;
        self.parts.relay_admissions.clear_door_port();
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    use super::Carriers;
    use crate::door::{DoorOutcome, DoorStart};

    #[tokio::test]
    async fn retiring_a_listener_joins_its_accepted_network_carrier() {
        struct Dropped(Arc<AtomicBool>);
        impl Drop for Dropped {
            fn drop(&mut self) {
                self.0.store(true, Ordering::Release);
            }
        }

        let carriers = Arc::new(Carriers::new());
        let dropped = Arc::new(AtomicBool::new(false));
        let guard = Dropped(Arc::clone(&dropped));
        let (ready, started) = tokio::sync::oneshot::channel();
        carriers.spawn(async move {
            let _guard = guard;
            ready.send(()).expect("carrier entered");
            std::future::pending::<()>().await;
        });
        started.await.expect("accepted carrier started");
        let listener = DoorStart {
            outcome: DoorOutcome::Bound("127.0.0.1:47657".parse().expect("address")),
            refresh_task: None,
            accept_task: None,
            network_carriers: Some(carriers),
            pairing_reaper_task: None,
            pairing_cap_refusals: None,
        };
        listener.retire().await;
        assert!(dropped.load(Ordering::Acquire), "accepted carrier joined");
    }
}
