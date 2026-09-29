// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::PathBuf;
use std::sync::Arc;

use chrono::{DateTime, FixedOffset, Local, Utc};

/// Injectable clock: the current instant together with the local offset.
///
/// Day strings and local wall-time forms come from the local reading
/// (`naive_local()`); epoch milliseconds and UTC timestamps come from the
/// instant, so neither is shifted by the local offset.
#[derive(Clone)]
pub struct Clock(Arc<dyn Fn() -> DateTime<FixedOffset> + Send + Sync>);

impl Clock {
    pub fn local() -> Self {
        Self(Arc::new(|| Local::now().fixed_offset()))
    }

    /// Now in the journal's owner zone, read afresh so a changed setting
    /// applies at once.
    pub fn owner(journal: impl Into<PathBuf>) -> Self {
        let journal = journal.into();
        Self(Arc::new(move || {
            Utc::now()
                .with_timezone(&solstone_core_journal_config::owner_zone(&journal))
                .fixed_offset()
        }))
    }

    pub fn new(now: impl Fn() -> DateTime<FixedOffset> + Send + Sync + 'static) -> Self {
        Self(Arc::new(now))
    }

    pub fn now(&self) -> DateTime<FixedOffset> {
        (self.0)()
    }
}
