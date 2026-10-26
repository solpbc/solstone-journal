// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Pause embedded journal services without the shared service stop.
//!
//! Pending recovery is not Stop. Each service task stays alive, reports idle, and
//! starts again after the recorded restore finishes.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use solstone_core_journal_io::JournalRoot;
use solstone_core_journal_io::whole_journal_restore::whole_journal_restore_blocks;
use tokio::sync::{Notify, watch};

pub(crate) struct ServiceControl {
    journal: PathBuf,
    root: Arc<JournalRoot>,
    pause: watch::Sender<bool>,
    child_stop: watch::Sender<bool>,
    idle: Arc<(Mutex<bool>, Condvar)>,
    notify: Arc<Notify>,
}

impl ServiceControl {
    pub(crate) fn new(root: Arc<JournalRoot>) -> Arc<Self> {
        let journal = root.canonical_path().to_owned();
        let (pause, _) = watch::channel(false);
        let (child_stop, _) = watch::channel(false);
        Arc::new(Self {
            journal,
            root,
            pause,
            child_stop,
            idle: Arc::new((Mutex::new(true), Condvar::new())),
            notify: Arc::new(Notify::new()),
        })
    }

    fn pause_watch(&self) -> watch::Receiver<bool> {
        self.pause.subscribe()
    }

    fn child_sender(&self) -> watch::Sender<bool> {
        self.child_stop.clone()
    }

    fn child_watch(&self) -> watch::Receiver<bool> {
        self.child_stop.subscribe()
    }

    fn set_idle(&self, idle: bool) {
        let (lock, wake) = &*self.idle;
        let mut flag = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        *flag = idle;
        if idle {
            wake.notify_all();
        }
    }

    pub(crate) fn pause_and_wait(&self, journal: &Path) -> Result<(), String> {
        self.root
            .revalidate_canonical_binding()
            .map_err(|error| error.to_string())?;
        if !same_journal(&self.journal, journal) {
            return Err("serving is bound to a different journal".into());
        }
        self.pause.send_replace(true);
        self.child_stop.send_replace(true);
        let (lock, wake) = &*self.idle;
        let mut idle = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        while !*idle {
            idle = wake
                .wait(idle)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
        }
        Ok(())
    }

    pub(crate) fn resume(&self, journal: &Path) -> Result<(), String> {
        self.root
            .revalidate_canonical_binding()
            .map_err(|error| error.to_string())?;
        if !same_journal(&self.journal, journal) {
            return Err("serving is bound to a different journal".into());
        }
        self.child_stop.send_replace(false);
        self.pause.send_replace(false);
        self.notify.notify_waiters();
        Ok(())
    }
}

pub(crate) fn persist_bound_port(
    root: &Arc<JournalRoot>,
    port: u16,
    checkpoint: &dyn Fn() -> Result<(), String>,
) -> Result<(), String> {
    use solstone_core_journal_config_write::{BoundConfigOutcome, DirectoryFlush};
    let _use =
        solstone_core_journal_io::whole_journal_restore::begin_target_use_bound(Arc::clone(root))
            .map_err(|error| error.to_string())?;
    let check = || {
        checkpoint()?;
        root.revalidate_canonical_binding()
            .map_err(|error| error.to_string())?;
        if whole_journal_restore_blocks(root.canonical_path()) {
            return Err("whole-journal restore is pending".to_owned());
        }
        Ok(())
    };
    let transaction = solstone_core_journal_config_write::persist_direct_door_port_at_root_checked(
        root, port, &check,
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
    check()
}

pub(crate) fn resume_services(
    controls: &[Arc<ServiceControl>],
    journal: &Path,
    port: u16,
    permanent: &watch::Receiver<bool>,
) -> Result<(), String> {
    let Some(first) = controls.first() else {
        return Err("no retained service owner".into());
    };
    let check = || {
        if stopped(permanent) {
            return Err("serving has stopped".into());
        }
        for control in controls {
            control
                .root
                .revalidate_canonical_binding()
                .map_err(|error| error.to_string())?;
            if !same_journal(&control.journal, journal)
                || control.root.identity() != first.root.identity()
            {
                return Err("serving is bound to a different journal".into());
            }
        }
        Ok(())
    };
    let _use = solstone_core_journal_io::whole_journal_restore::begin_target_use_bound(Arc::clone(
        &first.root,
    ))
    .map_err(|error| error.to_string())?;
    check()?;
    persist_bound_port(&first.root, port, &check)?;
    check()?;
    for control in controls {
        control.resume(journal)?;
    }
    Ok(())
}

fn same_journal(left: &Path, right: &Path) -> bool {
    match (left.canonicalize(), right.canonicalize()) {
        (Ok(left), Ok(right)) => left == right,
        _ => left == right,
    }
}

struct ForwardTask(Option<tokio::task::JoinHandle<()>>);

impl Drop for ForwardTask {
    fn drop(&mut self) {
        if let Some(task) = &self.0 {
            task.abort();
        }
    }
}

impl ForwardTask {
    async fn finish(mut self) {
        if let Some(task) = self.0.take() {
            task.abort();
            let _ = task.await;
        }
    }
}

fn stopped(stop: &watch::Receiver<bool>) -> bool {
    *stop.borrow() || stop.has_changed().is_err()
}

pub(crate) async fn run(control: Arc<ServiceControl>, permanent: watch::Receiver<bool>) {
    run_with(
        control,
        permanent,
        "embedded direct agent service stopped",
        |root, stop| async move {
            solstone_core_mcp_endpoint::run_embedded_direct_doors(root, stop)
                .await
                .map_err(|error| error.class().to_owned())
        },
    )
    .await;
}

pub(crate) async fn run_mcp(control: Arc<ServiceControl>, permanent: watch::Receiver<bool>) {
    run_with(
        control,
        permanent,
        "embedded agent service stopped",
        |root, stop| async move {
            solstone_core_mcp_endpoint::run_embedded_service(root, stop)
                .await
                .map_err(|error| error.class().to_owned())
        },
    )
    .await;
}

pub(crate) async fn run_relay(control: Arc<ServiceControl>, permanent: watch::Receiver<bool>) {
    run_with(
        control,
        permanent,
        "embedded relay service stopped",
        |root, stop| async move {
            solstone_core_spl::run_embedded_service(root, solstone_core_spl::Verbosity::Quiet, stop)
                .await
                .map_err(|error| error.class().to_owned())
        },
    )
    .await;
}

async fn run_with<F, Fut>(
    control: Arc<ServiceControl>,
    mut permanent: watch::Receiver<bool>,
    label: &'static str,
    mut service: F,
) where
    F: FnMut(PathBuf, watch::Receiver<bool>) -> Fut,
    Fut: std::future::Future<Output = Result<(), String>>,
{
    let mut pause = control.pause_watch();
    let child_tx = control.child_sender();
    let child_rx = control.child_watch();
    let mut forward_permanent = permanent.clone();
    let mut forward_pause = pause.clone();
    let forward = ForwardTask(Some(tokio::spawn(async move {
        loop {
            if stopped(&forward_permanent) {
                child_tx.send_replace(true);
                break;
            }
            child_tx.send_replace(*forward_pause.borrow());
            tokio::select! {
                changed = forward_permanent.changed() => {
                    if changed.is_err() {
                        child_tx.send_replace(true);
                        break;
                    }
                }
                changed = forward_pause.changed() => {
                    if changed.is_err() {
                        child_tx.send_replace(true);
                        break;
                    }
                }
            }
        }
    })));
    let root = control.journal.clone();
    loop {
        if stopped(&permanent) {
            break;
        }
        if *pause.borrow()
            || whole_journal_restore_blocks(&root)
            || control.root.revalidate_canonical_binding().is_err()
        {
            control.set_idle(true);
            tokio::select! {
                _ = permanent.changed() => continue,
                _ = pause.changed() => continue,
                _ = control.notify.notified() => continue,
            }
        }
        control.set_idle(false);
        if *pause.borrow()
            || stopped(&permanent)
            || whole_journal_restore_blocks(&root)
            || control.root.revalidate_canonical_binding().is_err()
        {
            control.set_idle(true);
            continue;
        }
        let result = service(root.clone(), child_rx.clone()).await;
        control.set_idle(true);
        if stopped(&permanent) {
            break;
        }
        if *pause.borrow()
            || whole_journal_restore_blocks(&root)
            || control.root.revalidate_canonical_binding().is_err()
        {
            continue;
        }
        if let Err(error) = result {
            log::warn!("{label}: {error}");
        }
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(1)) => {}
            _ = permanent.changed() => break,
            _ = pause.changed() => {}
        }
    }
    control.child_stop.send_replace(true);
    forward.finish().await;
    control.set_idle(true);
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use solstone_core_journal_io::whole_journal_restore::{
        WholeJournalRestoreSource, publish_whole_journal_restore,
    };

    fn pending_control() -> (tempfile::TempDir, Arc<ServiceControl>) {
        let journal = tempfile::tempdir().unwrap();
        publish_whole_journal_restore(
            journal.path(),
            &WholeJournalRestoreSource::Byo {
                repository: "synthetic".into(),
                backend: "local".into(),
            },
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            "/journal",
        )
        .unwrap();
        let control = ServiceControl::new(Arc::new(JournalRoot::open(journal.path()).unwrap()));
        (journal, control)
    }

    #[tokio::test]
    async fn stop_joins_the_forwarder_while_recovery_is_pending() {
        let (_journal, control) = pending_control();
        let (stop, receiver) = watch::channel(false);
        stop.send_replace(true);
        tokio::time::timeout(Duration::from_secs(1), run(control, receiver))
            .await
            .unwrap();
        assert_eq!(
            stop.receiver_count(),
            0,
            "no detached permanent-watch receiver"
        );
    }

    #[tokio::test]
    async fn a_closed_stop_channel_ends_the_paused_owner() {
        let (_journal, control) = pending_control();
        let (stop, receiver) = watch::channel(false);
        drop(stop);
        tokio::time::timeout(Duration::from_secs(1), run(control, receiver))
            .await
            .unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn admitted_write_drains_before_copy_and_the_retained_service_resumes() {
        use solstone_core_convey_shell::recovery_drain::{
            register_recovery_hooks, relinquish_admitted_work, resume_after_whole_journal_restore,
        };
        use solstone_core_journal_io::whole_journal_restore::clear_whole_journal_restore;
        use std::sync::atomic::{AtomicUsize, Ordering};

        let journal = tempfile::tempdir().unwrap();
        let root = Arc::new(JournalRoot::open(journal.path()).unwrap());
        let control = ServiceControl::new(Arc::clone(&root));
        let pause = Arc::clone(&control);
        let resume = Arc::clone(&control);
        let refusal_control = Arc::clone(&control);
        let (stop, receiver) = watch::channel(false);
        let resume_stop = receiver.clone();
        let _hooks = register_recovery_hooks(
            "controller-proof",
            Arc::clone(&root),
            move |path| pause.pause_and_wait(path),
            move |path| resume_services(&[Arc::clone(&resume)], path, 9137, &resume_stop),
        );
        let calls = Arc::new(AtomicUsize::new(0));
        let count = Arc::clone(&calls);
        let (events, mut started) = tokio::sync::mpsc::unbounded_channel();
        let (release_send, receive_release) = tokio::sync::oneshot::channel();
        let release = Arc::new(Mutex::new(Some(receive_release)));
        let (observed, observe_stop) = tokio::sync::oneshot::channel();
        let observed = Arc::new(Mutex::new(Some(observed)));
        let service = tokio::spawn(run_with(
            control,
            receiver,
            "test service",
            move |path, mut child_stop| {
                let sequence = count.fetch_add(1, Ordering::AcqRel);
                let events = events.clone();
                let wait_release = release.lock().unwrap().take();
                let observed = observed.lock().unwrap().take();
                async move {
                    if sequence == 0 {
                        std::fs::write(path.join("accepted"), b"before restore")
                            .map_err(|e| e.to_string())?;
                        events.send(0).unwrap();
                        while !stopped(&child_stop) {
                            let _ = child_stop.changed().await;
                        }
                        observed.unwrap().send(()).unwrap();
                        wait_release.unwrap().await.unwrap();
                        std::fs::write(path.join("drained"), b"joined writer")
                            .map_err(|e| e.to_string())?;
                    } else {
                        let config: serde_json::Value = serde_json::from_slice(
                            &std::fs::read(path.join("config/journal.json")).unwrap(),
                        )
                        .unwrap();
                        assert_eq!(
                            config["pairing"]["direct_port"], 9137,
                            "current bound port is persisted before any service resumes"
                        );
                        std::fs::write(path.join("resumed"), b"after retry")
                            .map_err(|e| e.to_string())?;
                        events.send(sequence).unwrap();
                        while !stopped(&child_stop) {
                            let _ = child_stop.changed().await;
                        }
                    }
                    Ok(())
                }
            },
        ));
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(2), started.recv())
                .await
                .unwrap(),
            Some(0)
        );
        let record = publish_whole_journal_restore(
            journal.path(),
            &WholeJournalRestoreSource::Byo {
                repository: "synthetic".into(),
                backend: "local".into(),
            },
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            "/journal",
        )
        .unwrap();
        let target = journal.path().to_owned();
        let drain = tokio::task::spawn_blocking(move || relinquish_admitted_work(&target));
        tokio::time::timeout(Duration::from_secs(2), observe_stop)
            .await
            .unwrap()
            .unwrap();
        assert!(
            !drain.is_finished(),
            "copy cannot begin before the admitted writer joins"
        );
        assert!(!journal.path().join("drained").exists());
        release_send.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(2), drain)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(
            std::fs::read(journal.path().join("drained")).unwrap(),
            b"joined writer"
        );
        assert_eq!(
            calls.load(Ordering::Acquire),
            1,
            "pending recovery does not restart the service"
        );
        std::fs::create_dir_all(journal.path().join("config")).unwrap();
        std::fs::write(
            journal.path().join("config/journal.json"),
            br#"{"pairing":{"direct_port":5555},"preserved":"restored"}"#,
        )
        .unwrap();
        let restored = std::fs::read(journal.path().join("config/journal.json")).unwrap();
        assert!(
            resume_services(&[refusal_control], journal.path(), 9137, &stop.subscribe()).is_err()
        );
        assert_eq!(
            std::fs::read(journal.path().join("config/journal.json")).unwrap(),
            restored
        );
        clear_whole_journal_restore(journal.path(), &record).unwrap();
        resume_after_whole_journal_restore(journal.path());
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(2), started.recv())
                .await
                .unwrap(),
            Some(1)
        );
        assert_eq!(
            std::fs::read(journal.path().join("resumed")).unwrap(),
            b"after retry"
        );
        stop.send_replace(true);
        tokio::time::timeout(Duration::from_secs(2), service)
            .await
            .unwrap()
            .unwrap();
        drop(_hooks);
        assert_eq!(stop.receiver_count(), 0);
    }

    #[test]
    fn failed_port_publication_does_not_resume_any_service() {
        let journal = tempfile::tempdir().unwrap();
        let root = Arc::new(JournalRoot::open(journal.path()).unwrap());
        let controls = [
            ServiceControl::new(root.clone()),
            ServiceControl::new(root.clone()),
            ServiceControl::new(root),
        ];
        for control in &controls {
            control.pause_and_wait(journal.path()).unwrap();
        }
        let (stop, receiver) = watch::channel(false);
        std::fs::create_dir(journal.path().join("config")).unwrap();
        let config = journal.path().join("config/journal.json");
        std::fs::write(&config, b"invalid restored configuration").unwrap();
        assert!(resume_services(&controls, journal.path(), 9137, &receiver).is_err());
        assert_eq!(
            std::fs::read(&config).unwrap(),
            b"invalid restored configuration"
        );
        assert!(controls.iter().all(|control| *control.pause.borrow()));
        std::fs::write(&config, br#"{"pairing":{"direct_port":5555}}"#).unwrap();
        stop.send_replace(true);
        assert!(resume_services(&controls, journal.path(), 9137, &receiver).is_err());
        assert_eq!(
            std::fs::read(&config).unwrap(),
            br#"{"pairing":{"direct_port":5555}}"#
        );
        assert!(controls.iter().all(|control| *control.pause.borrow()));
    }
}
