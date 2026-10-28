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
}
