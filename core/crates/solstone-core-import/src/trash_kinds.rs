// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Every kind of thing the trash holds, in one list, so every caller that
//! recovers or sweeps knows them all: a kind left out here leaves its items
//! stuck in the trash.

use std::path::Path;

use chrono::{DateTime, Utc};
use solstone_core_indexer_store::RetentionIndex;
use solstone_core_retention::recording_trash::Recordings;
use solstone_core_trash::{Kind, Swept};

use crate::import_trash::Imports;

/// Run `act` with every kind of thing the trash holds.
pub fn with_kinds<T>(journal: &Path, act: impl FnOnce(&[&dyn Kind]) -> T) -> T {
    let index = RetentionIndex::new(journal);
    let recordings = Recordings::new(&index);
    let imports = Imports::new(&index);
    act(&[&recordings, &imports])
}

/// Finish what a stop interrupted and erase what is due, for every kind.
pub fn sweep(journal: &Path, now: DateTime<Utc>) -> Swept {
    with_kinds(journal, |kinds| {
        solstone_core_trash::sweep(journal, kinds, now)
    })
}
