// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Every kind of thing the trash holds, in one list, so every caller that
//! recovers or sweeps knows them all: a kind left out here leaves its items
//! stuck in the trash.

use std::path::Path;

use chrono::{DateTime, Utc};
use solstone_core_facets::entity_trash::Entities;
use solstone_core_facets::facet_trash::{FacetDrops, Facets};
use solstone_core_indexer_store::RetentionIndex;
use solstone_core_retention::recording_trash::Recordings;
use solstone_core_speaker_resolve::speakers_trash::Speakers;
use solstone_core_trash::{Kind, Swept};

use crate::import_trash::Imports;

/// Run `act` with every kind of thing the trash holds.
pub fn with_kinds<T>(journal: &Path, act: impl FnOnce(&[&dyn Kind]) -> T) -> T {
    let index = RetentionIndex::new(journal);
    let recordings = Recordings::new(&index);
    let imports = Imports::new(&index);
    let speakers = Speakers::new(&index);
    let facets = Facets::new(&index);
    let drops = FacetDrops::new(&index);
    let entities = Entities::new(&index);
    act(&[&recordings, &imports, &speakers, &facets, &drops, &entities])
}

/// Finish what a stop interrupted and erase what is due, for every kind.
///
/// Erasing an archive leaves its rows in the shared claims database, and those
/// go here too: a run neither in the journal nor in the trash is gone for good.
pub fn sweep(journal: &Path, now: DateTime<Utc>) -> Swept {
    let mut swept = with_kinds(journal, |kinds| {
        solstone_core_trash::sweep(journal, kinds, now)
    });
    // Read afresh for each run it asks about; a trash that can't be read never
    // lets an archive's claims go.
    let held = |id: &str| {
        solstone_core_trash::held_paths(journal).map_or(true, |paths| {
            paths.iter().any(|path| *path == format!("imports/{id}"))
        })
    };
    if let Err(error) = crate::audio_archive::claims::forget_gone_runs(journal, &held) {
        swept.failed.push(format!("archive claims: {error}"));
    }
    swept
}
