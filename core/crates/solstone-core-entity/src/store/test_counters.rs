// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#[cfg(any(test, feature = "test-hooks"))]
pub mod test_instrumentation {
    use std::cell::{Cell, RefCell};
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum CacheFailMode {
        FailCache,
        FailEnqueue,
    }

    thread_local! {
        static ENTITIES_DIR_LISTINGS: Cell<usize> = const { Cell::new(0) };
        static RELATIONSHIP_READS: RefCell<HashMap<PathBuf, usize>> = RefCell::new(HashMap::new());
        static VISIBLE_HISTORY_READS: RefCell<HashMap<String, usize>> = RefCell::new(HashMap::new());
        static CACHE_FAILURE_HOOK: Cell<Option<CacheFailMode>> = const { Cell::new(None) };
    }

    pub fn reset_test_counters() {
        ENTITIES_DIR_LISTINGS.with(|c| c.set(0));
        RELATIONSHIP_READS.with(|r| r.borrow_mut().clear());
        VISIBLE_HISTORY_READS.with(|v| v.borrow_mut().clear());
    }

    pub fn increment_entities_dir_listing() {
        ENTITIES_DIR_LISTINGS.with(|c| c.set(c.get() + 1));
    }

    pub fn entities_dir_listing_count() -> usize {
        ENTITIES_DIR_LISTINGS.with(|c| c.get())
    }

    pub fn increment_relationship_read(path: &Path) {
        RELATIONSHIP_READS.with(|r| {
            let mut map = r.borrow_mut();
            *map.entry(path.to_path_buf()).or_default() += 1;
        });
    }

    pub fn relationship_read_count_for_path(path: &Path) -> usize {
        RELATIONSHIP_READS.with(|r| r.borrow().get(path).copied().unwrap_or(0))
    }

    pub fn total_relationship_read_count() -> usize {
        RELATIONSHIP_READS.with(|r| r.borrow().values().sum())
    }

    pub fn increment_visible_history_read(entity_dir: &str) {
        VISIBLE_HISTORY_READS.with(|v| {
            let mut map = v.borrow_mut();
            *map.entry(entity_dir.to_owned()).or_default() += 1;
        });
    }

    pub fn visible_history_read_count(entity_dir: &str) -> usize {
        VISIBLE_HISTORY_READS.with(|v| v.borrow().get(entity_dir).copied().unwrap_or(0))
    }

    pub fn total_visible_history_read_count() -> usize {
        VISIBLE_HISTORY_READS.with(|v| v.borrow().values().sum())
    }

    pub struct CacheFailureGuard;
    impl Drop for CacheFailureGuard {
        fn drop(&mut self) {
            CACHE_FAILURE_HOOK.with(|h| h.set(None));
        }
    }

    pub fn set_write_resolved_cache_failure_hook(mode: CacheFailMode) -> CacheFailureGuard {
        CACHE_FAILURE_HOOK.with(|h| h.set(Some(mode)));
        CacheFailureGuard
    }

    pub fn take_cache_failure_hook() -> Option<CacheFailMode> {
        CACHE_FAILURE_HOOK.with(|h| h.take())
    }

    pub fn peek_cache_failure_hook() -> Option<CacheFailMode> {
        CACHE_FAILURE_HOOK.with(|h| h.get())
    }
}
