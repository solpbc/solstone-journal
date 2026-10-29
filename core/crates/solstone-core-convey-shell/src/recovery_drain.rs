// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Process-local drain of owners already admitted on a journal.
//!
//! The restore path holds the target, asks each registered owner to stop
//! using it, then waits until in-flight use has dropped. An empty registry
//! fails closed so a copy cannot start without that proof. Each registration
//! is an owned generation. An older Stop or drop removes only that generation.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use solstone_core_journal_io::JournalRoot;
use solstone_core_journal_io::whole_journal_restore::{
    WholeJournalRestoreAdmission, hold_target_for_restore, release_target_for_restore,
    wait_for_target_use_to_end, whole_journal_restore_admission,
};

type Hook = Arc<dyn Fn(&Path) -> Result<(), String> + Send + Sync>;

struct Slot {
    generation: u64,
    root: Arc<JournalRoot>,
    pause: Hook,
    resume: Hook,
}

struct Registry {
    slots: BTreeMap<(PathBuf, String), Slot>,
}

fn registry() -> &'static Mutex<Registry> {
    static REGISTRY: Mutex<Registry> = Mutex::new(Registry {
        slots: BTreeMap::new(),
    });
    &REGISTRY
}

fn lock_registry() -> std::sync::MutexGuard<'static, Registry> {
    registry()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// One owner's pause and resume hooks. Dropping it removes those hooks only
/// while this generation is still the registered one.
pub struct RecoveryHooks {
    key: (PathBuf, String),
    generation: u64,
    live: AtomicBool,
}

impl RecoveryHooks {
    /// Remove these hooks when they are still the current registration.
    pub fn clear(&self) {
        if !self.live.swap(false, Ordering::AcqRel) {
            return;
        }
        let mut registry = lock_registry();
        let current = registry
            .slots
            .get(&self.key)
            .is_some_and(|slot| slot.generation == self.generation);
        if current {
            registry.slots.remove(&self.key);
        }
    }
}

impl Drop for RecoveryHooks {
    fn drop(&mut self) {
        self.clear();
    }
}

static NEXT_REGISTRATION: AtomicU64 = AtomicU64::new(1);

/// Install hooks for one retained root and owner id. A later registration for
/// that pair leaves the previous token unable to remove its successor.
pub fn register_recovery_hooks(
    id: &str,
    root: Arc<JournalRoot>,
    pause: impl Fn(&Path) -> Result<(), String> + Send + Sync + 'static,
    resume: impl Fn(&Path) -> Result<(), String> + Send + Sync + 'static,
) -> RecoveryHooks {
    let generation = NEXT_REGISTRATION.fetch_add(1, Ordering::AcqRel);
    let key = (root.canonical_path().to_owned(), id.to_owned());
    let mut registry = lock_registry();
    registry.slots.insert(
        key.clone(),
        Slot {
            generation,
            root,
            pause: Arc::new(pause),
            resume: Arc::new(resume),
        },
    );
    RecoveryHooks {
        key,
        generation,
        live: AtomicBool::new(true),
    }
}

fn hooks_for(journal: &Path, pause: bool) -> Result<Vec<Hook>, String> {
    let current = JournalRoot::open(journal).map_err(|_| "restore drain root unavailable")?;
    current
        .revalidate_canonical_binding()
        .map_err(|_| "restore drain root changed")?;
    let registry = lock_registry();
    let mut hooks = Vec::new();
    for ((path, _), slot) in &registry.slots {
        let same_path = path == current.canonical_path();
        let same_object = slot.root.identity() == current.identity();
        if !same_path && !same_object {
            continue;
        }
        slot.root
            .revalidate_canonical_binding()
            .map_err(|_| "restore drain root changed")?;
        if !same_object {
            return Err("restore drain root changed".into());
        }
        hooks.push(Arc::clone(if pause { &slot.pause } else { &slot.resume }));
    }
    Ok(hooks)
}

/// Hold the target, run every pause hook, and wait until in-flight use drops.
///
/// An empty hook list leaves the hold set and returns an error. The caller
/// releases the hold when the restore does not proceed.
pub fn relinquish_admitted_work(journal: &Path) -> Result<(), String> {
    hold_target_for_restore(journal);
    let hooks = hooks_for(journal, true)?;
    if hooks.is_empty() {
        return Err("admitted work has no drain".into());
    }
    for hook in hooks {
        hook(journal)?;
    }
    wait_for_target_use_to_end(journal);
    Ok(())
}

/// Pause admitted owners and release the hold. The obligation file stays.
///
/// Even with no registered hook, any already-admitted use must finish.
pub fn stop_admitted_work(journal: &Path) -> Result<(), String> {
    hold_target_for_restore(journal);
    let hooks = hooks_for(journal, true)?;
    for hook in &hooks {
        hook(journal)?;
    }
    wait_for_target_use_to_end(journal);
    release_target_for_restore(journal);
    Ok(())
}

/// Release the hold and, once the obligation is gone, let owners continue.
pub fn resume_after_whole_journal_restore(journal: &Path) {
    release_target_for_restore(journal);
    if whole_journal_restore_admission(journal) != WholeJournalRestoreAdmission::Open {
        return;
    }
    let hooks = match hooks_for(journal, false) {
        Ok(hooks) => hooks,
        Err(error) => {
            log::warn!("whole-journal restore resume: {error}");
            return;
        }
    };
    for hook in hooks {
        if let Err(error) = hook(journal) {
            log::warn!("whole-journal restore resume: {error}");
        }
    }
}

struct CapturedHook {
    key: (PathBuf, String),
    generation: u64,
    pause: Hook,
    resume: Hook,
}

impl CapturedHook {
    fn current(&self, root: &Arc<JournalRoot>) -> Result<(), String> {
        root.revalidate_canonical_binding()
            .map_err(|e| e.to_string())?;
        let registry = lock_registry();
        let slot = registry
            .slots
            .get(&self.key)
            .ok_or("restore can't resume from this attempt")?;
        if slot.generation != self.generation || slot.root.identity() != root.identity() {
            return Err("restore can't resume because its running state changed".into());
        }
        slot.root
            .revalidate_canonical_binding()
            .map_err(|e| e.to_string())
    }
}

/// A paused native attempt, retaining its original root, hooks and process hold.
/// Drop releases only this hold; it never resumes or removes the durable record.
pub struct PausedRecovery {
    root: Arc<JournalRoot>,
    hold: Option<solstone_core_journal_io::whole_journal_restore::TargetRestoreHold>,
    hooks: Vec<CapturedHook>,
}

/// Drain the owners registered for this exact root without reopening its path.
pub fn pause_admitted_work(root: Arc<JournalRoot>) -> Result<PausedRecovery, String> {
    use solstone_core_journal_io::whole_journal_restore::TargetRestoreHold;
    let hold = TargetRestoreHold::acquire(Arc::clone(&root)).map_err(|e| e.to_string())?;
    let hooks = {
        let registry = lock_registry();
        let mut hooks = Vec::new();
        for (key, slot) in &registry.slots {
            let same_path = key.0 == root.canonical_path();
            let same_object = slot.root.identity() == root.identity();
            if !same_path && !same_object {
                continue;
            }
            if !same_object {
                return Err("restore couldn't confirm this is the same journal".into());
            }
            slot.root
                .revalidate_canonical_binding()
                .map_err(|e| e.to_string())?;
            hooks.push(CapturedHook {
                key: key.clone(),
                generation: slot.generation,
                pause: Arc::clone(&slot.pause),
                resume: Arc::clone(&slot.resume),
            });
        }
        hooks
    };
    if hooks.is_empty() {
        return Err("couldn't pause your journal for restore".into());
    }
    let paused = PausedRecovery {
        root,
        hold: Some(hold),
        hooks,
    };
    for hook in &paused.hooks {
        hook.current(&paused.root)?;
        (hook.pause)(paused.root.canonical_path())?;
    }
    paused
        .hold
        .as_ref()
        .expect("owned hold")
        .wait_for_admitted_use()
        .map_err(|e| e.to_string())?;
    Ok(paused)
}

impl PausedRecovery {
    pub fn resume(mut self, cancelled: &dyn Fn() -> bool) -> Result<(), String> {
        self.hold
            .as_ref()
            .expect("owned hold")
            .revalidate()
            .map_err(|e| e.to_string())?;
        if cancelled()
            || whole_journal_restore_admission(self.root.canonical_path())
                != WholeJournalRestoreAdmission::Open
        {
            return Err("restore is not ready to resume".into());
        }
        // Validate the complete original census before releasing admission.
        for hook in &self.hooks {
            hook.current(&self.root)?;
        }
        self.root
            .revalidate_canonical_binding()
            .map_err(|e| e.to_string())?;
        drop(self.hold.take());
        let mut failure = None;
        for hook in &self.hooks {
            if cancelled() {
                return Err("restore was stopped".into());
            }
            let result = hook
                .current(&self.root)
                .and_then(|()| (hook.resume)(self.root.canonical_path()));
            if let Err(error) = result {
                log::warn!("whole-journal restore resume: {error}");
                failure.get_or_insert(error);
            }
        }
        if cancelled() {
            return Err("restore was stopped".into());
        }
        failure.map_or(Ok(()), Err)
    }
}

impl solstone_core_backup_web::PausedRestore for PausedRecovery {
    fn resume(self: Box<Self>, cancelled: &dyn Fn() -> bool) -> Result<(), String> {
        PausedRecovery::resume(*self, cancelled)
    }
}

pub struct EmbeddedRestoreLifecycle;

impl solstone_core_backup_web::EmbeddedRestoreLifecycle for EmbeddedRestoreLifecycle {
    fn drain(
        &self,
        root: &Arc<JournalRoot>,
    ) -> Result<Box<dyn solstone_core_backup_web::PausedRestore>, String> {
        pause_admitted_work(Arc::clone(root))
            .map(|paused| Box::new(paused) as Box<dyn solstone_core_backup_web::PausedRestore>)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    fn lock_tests() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: Mutex<()> = Mutex::new(());
        LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn clear_for_test() {
        let mut registry = lock_registry();
        registry.slots.clear();
    }

    #[cfg(all(windows, feature = "full-tests"))]
    #[test]
    fn owned_windows_spelling_runs_only_the_captured_generation() {
        let _tests = lock_tests();
        clear_for_test();
        let journal = tempfile::tempdir().unwrap();
        let canonical = journal.path().canonicalize().unwrap();
        let ordinary = PathBuf::from(canonical.to_str().unwrap().strip_prefix(r"\\?\").unwrap());
        let root = Arc::new(JournalRoot::open(&ordinary).unwrap());
        let bound = Arc::new(JournalRoot::open(&canonical).unwrap());
        assert_ne!(root.canonical_path(), bound.canonical_path());
        let pauses = Arc::new(AtomicUsize::new(0));
        let resumes = Arc::new(AtomicUsize::new(0));
        let p = pauses.clone();
        let r = resumes.clone();
        let original = register_recovery_hooks(
            "bound-windows-owner",
            root.clone(),
            move |_| {
                p.fetch_add(1, Ordering::SeqCst);
                Ok(())
            },
            move |_| {
                r.fetch_add(1, Ordering::SeqCst);
                Ok(())
            },
        );
        pause_admitted_work(bound.clone())
            .unwrap()
            .resume(&|| false)
            .unwrap();
        assert_eq!(pauses.load(Ordering::SeqCst), 1);
        assert_eq!(resumes.load(Ordering::SeqCst), 1);
        let paused = pause_admitted_work(bound.clone()).unwrap();
        let successor_calls = Arc::new(AtomicUsize::new(0));
        let r = successor_calls.clone();
        let successor = register_recovery_hooks(
            "bound-windows-owner",
            root,
            |_| Ok(()),
            move |_| {
                r.fetch_add(1, Ordering::SeqCst);
                Ok(())
            },
        );
        assert!(paused.resume(&|| false).is_err());
        assert_eq!(resumes.load(Ordering::SeqCst), 1);
        assert_eq!(successor_calls.load(Ordering::SeqCst), 0);
        drop(original);
        pause_admitted_work(bound)
            .unwrap()
            .resume(&|| false)
            .unwrap();
        assert_eq!(successor_calls.load(Ordering::SeqCst), 1);
        drop(successor);
        clear_for_test();
    }

    #[cfg(windows)]
    #[test]
    fn canonical_windows_spelling_runs_the_retained_owner_hooks() {
        let _tests = lock_tests();
        clear_for_test();
        let journal = tempfile::tempdir().unwrap();
        let canonical = journal.path().canonicalize().unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let pauses = Arc::clone(&calls);
        let resumes = Arc::clone(&calls);
        let hooks = register_recovery_hooks(
            "native-owner",
            Arc::new(JournalRoot::open(journal.path()).unwrap()),
            move |_| {
                pauses.fetch_add(1, Ordering::SeqCst);
                Ok(())
            },
            move |_| {
                resumes.fetch_add(1, Ordering::SeqCst);
                Ok(())
            },
        );
        relinquish_admitted_work(&canonical).unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        resume_after_whole_journal_restore(&canonical);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        drop(hooks);
        clear_for_test();
    }

    #[test]
    fn empty_drain_fails_and_a_registered_owner_runs_before_wait() {
        let _tests = lock_tests();
        clear_for_test();
        let journal = tempfile::tempdir().expect("journal");
        let error = relinquish_admitted_work(journal.path()).unwrap_err();
        assert!(error.contains("no drain"));
        release_target_for_restore(journal.path());

        let pauses = Arc::new(AtomicUsize::new(0));
        let resumes = Arc::new(AtomicUsize::new(0));
        let pause_count = Arc::clone(&pauses);
        let resume_count = Arc::clone(&resumes);
        let hooks = register_recovery_hooks(
            "test-owner",
            Arc::new(JournalRoot::open(journal.path()).expect("root")),
            move |_| {
                pause_count.fetch_add(1, Ordering::SeqCst);
                Ok(())
            },
            move |_| {
                resume_count.fetch_add(1, Ordering::SeqCst);
                Ok(())
            },
        );
        relinquish_admitted_work(journal.path()).expect("drain");
        assert_eq!(pauses.load(Ordering::SeqCst), 1);
        assert!(
            solstone_core_journal_io::whole_journal_restore::target_held_for_restore(
                journal.path()
            )
        );
        resume_after_whole_journal_restore(journal.path());
        assert_eq!(resumes.load(Ordering::SeqCst), 1);
        assert!(
            !solstone_core_journal_io::whole_journal_restore::target_held_for_restore(
                journal.path()
            )
        );
        drop(hooks);
        clear_for_test();
    }

    #[test]
    fn an_older_registration_cannot_remove_its_successor() {
        let _tests = lock_tests();
        clear_for_test();
        let journal = tempfile::tempdir().expect("journal");
        let first_pauses = Arc::new(AtomicUsize::new(0));
        let second_pauses = Arc::new(AtomicUsize::new(0));
        let first_count = Arc::clone(&first_pauses);
        let second_count = Arc::clone(&second_pauses);
        let first = register_recovery_hooks(
            "foreground",
            Arc::new(JournalRoot::open(journal.path()).expect("root")),
            move |_| {
                first_count.fetch_add(1, Ordering::SeqCst);
                Ok(())
            },
            |_| Ok(()),
        );
        let second = register_recovery_hooks(
            "foreground",
            Arc::new(JournalRoot::open(journal.path()).expect("root")),
            move |_| {
                second_count.fetch_add(1, Ordering::SeqCst);
                Ok(())
            },
            |_| Ok(()),
        );
        drop(first);
        relinquish_admitted_work(journal.path()).expect("successor drain");
        assert_eq!(first_pauses.load(Ordering::SeqCst), 0);
        assert_eq!(second_pauses.load(Ordering::SeqCst), 1);
        second.clear();
        let error = relinquish_admitted_work(journal.path()).unwrap_err();
        assert!(error.contains("no drain"));
        release_target_for_restore(journal.path());
        clear_for_test();
    }

    #[test]
    fn the_same_owner_name_on_another_root_cannot_replace_or_satisfy_this_drain() {
        let _tests = lock_tests();
        clear_for_test();
        let first = tempfile::tempdir().expect("first journal");
        let second = tempfile::tempdir().expect("second journal");
        let unknown = tempfile::tempdir().expect("unregistered journal");
        let first_calls = Arc::new(AtomicUsize::new(0));
        let second_calls = Arc::new(AtomicUsize::new(0));
        let count_first = Arc::clone(&first_calls);
        let count_second = Arc::clone(&second_calls);
        let first_hook = register_recovery_hooks(
            "foreground",
            Arc::new(JournalRoot::open(first.path()).expect("first root")),
            move |_| {
                count_first.fetch_add(1, Ordering::SeqCst);
                Ok(())
            },
            |_| Ok(()),
        );
        let second_hook = register_recovery_hooks(
            "foreground",
            Arc::new(JournalRoot::open(second.path()).expect("second root")),
            move |_| {
                count_second.fetch_add(1, Ordering::SeqCst);
                Ok(())
            },
            |_| Ok(()),
        );
        assert!(relinquish_admitted_work(unknown.path()).is_err());
        release_target_for_restore(unknown.path());
        relinquish_admitted_work(first.path()).expect("first drain");
        assert_eq!(first_calls.load(Ordering::SeqCst), 1);
        assert_eq!(second_calls.load(Ordering::SeqCst), 0);
        release_target_for_restore(first.path());
        drop(first_hook);
        relinquish_admitted_work(second.path()).expect("second drain survives first drop");
        assert_eq!(second_calls.load(Ordering::SeqCst), 1);
        release_target_for_restore(second.path());
        drop(second_hook);
        clear_for_test();
    }

    #[cfg(unix)]
    #[test]
    fn a_replaced_root_cannot_borrow_the_previous_owner_drain() {
        let _tests = lock_tests();
        clear_for_test();
        let directory = tempfile::tempdir().expect("fixture");
        let journal = directory.path().join("journal");
        std::fs::create_dir(&journal).expect("journal");
        let calls = Arc::new(AtomicUsize::new(0));
        let count = Arc::clone(&calls);
        let hooks = register_recovery_hooks(
            "foreground",
            Arc::new(JournalRoot::open(&journal).expect("root")),
            move |_| {
                count.fetch_add(1, Ordering::SeqCst);
                Ok(())
            },
            |_| Ok(()),
        );
        std::fs::rename(&journal, directory.path().join("original")).expect("retain original");
        std::fs::create_dir(&journal).expect("replacement");
        assert!(relinquish_admitted_work(&journal).is_err());
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        release_target_for_restore(&journal);
        drop(hooks);
        clear_for_test();
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[cfg(unix)]
    #[test]
    fn owned_phase_root_loss_drops_only_its_hold_and_fresh_owner_can_resume() {
        let _tests = lock_tests();
        let temporary = tempfile::tempdir().unwrap();
        let path = temporary.path().join("journal");
        std::fs::create_dir(&path).unwrap();
        let original = Arc::new(JournalRoot::open(&path).unwrap());
        let old = register_recovery_hooks("phase-root", original.clone(), |_| Ok(()), |_| Ok(()));
        let paused = pause_admitted_work(original).unwrap();
        std::fs::rename(&path, temporary.path().join("original")).unwrap();
        std::fs::create_dir(&path).unwrap();
        std::fs::write(path.join("sentinel"), b"fresh owner").unwrap();
        let successor = Arc::new(JournalRoot::open(&path).unwrap());
        let resumed = Arc::new(AtomicUsize::new(0));
        let counter = resumed.clone();
        let fresh = register_recovery_hooks(
            "phase-root",
            successor.clone(),
            |_| Ok(()),
            move |_| {
                counter.fetch_add(1, Ordering::SeqCst);
                Ok(())
            },
        );
        assert!(paused.resume(&|| false).is_err());
        assert_eq!(resumed.load(Ordering::SeqCst), 0);
        drop(old);
        let next = pause_admitted_work(successor.clone()).unwrap();
        next.resume(&|| false).unwrap();
        assert_eq!(resumed.load(Ordering::SeqCst), 1);
        assert!(
            solstone_core_journal_io::whole_journal_restore::begin_target_use_bound(successor)
                .is_ok()
        );
        assert_eq!(
            std::fs::read(path.join("sentinel")).unwrap(),
            b"fresh owner"
        );
        drop(fresh);
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[test]
    fn owned_phase_cannot_resume_a_successor_registration_on_the_same_root() {
        let _tests = lock_tests();
        let temporary = tempfile::tempdir().unwrap();
        let root = Arc::new(JournalRoot::open(temporary.path()).unwrap());
        let old = register_recovery_hooks("phase-generation", root.clone(), |_| Ok(()), |_| Ok(()));
        let paused = pause_admitted_work(root.clone()).unwrap();
        let resumed = Arc::new(AtomicUsize::new(0));
        let counter = resumed.clone();
        let current = register_recovery_hooks(
            "phase-generation",
            root.clone(),
            |_| Ok(()),
            move |_| {
                counter.fetch_add(1, Ordering::SeqCst);
                Ok(())
            },
        );
        assert!(paused.resume(&|| false).is_err());
        assert_eq!(resumed.load(Ordering::SeqCst), 0);
        drop(old);
        pause_admitted_work(root.clone())
            .unwrap()
            .resume(&|| false)
            .unwrap();
        assert_eq!(resumed.load(Ordering::SeqCst), 1);
        assert!(
            solstone_core_journal_io::whole_journal_restore::begin_target_use_bound(root).is_ok()
        );
        drop(current);
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[test]
    fn owned_phase_unwind_releases_hold_without_resuming() {
        let _tests = lock_tests();
        let temporary = tempfile::tempdir().unwrap();
        let root = Arc::new(JournalRoot::open(temporary.path()).unwrap());
        let resumed = Arc::new(AtomicUsize::new(0));
        let counter = resumed.clone();
        let _hook = register_recovery_hooks(
            "phase-unwind",
            root.clone(),
            |_| Ok(()),
            move |_| {
                counter.fetch_add(1, Ordering::SeqCst);
                Ok(())
            },
        );
        let paused = pause_admitted_work(root.clone()).unwrap();
        assert!(
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
                let _paused = paused;
                panic!("native frame unwound");
            }))
            .is_err()
        );
        assert_eq!(resumed.load(Ordering::SeqCst), 0);
        assert!(
            solstone_core_journal_io::whole_journal_restore::begin_target_use_bound(root.clone())
                .is_ok()
        );
        pause_admitted_work(root)
            .unwrap()
            .resume(&|| false)
            .unwrap();
        assert_eq!(resumed.load(Ordering::SeqCst), 1);
    }
}
