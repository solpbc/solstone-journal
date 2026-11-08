// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The facet a Strava import files its workouts under, and the filing itself.
//!
//! A run is given a facet, none, or nothing. With nothing (a command line
//! without `--facet`, or an import page that sent no facet) it uses the
//! remembered default while that facet is still enabled, and otherwise none;
//! it never makes a facet and never asks. With a facet, every live tile of the
//! download's workouts, written or unchanged, gets it in `talents/facets.json`
//! wherever it has another, so a facet chosen later replaces the earlier one.
//! A workout an earlier reading of its tiles filed an activity record for keeps
//! its tiles' facet, because that record stays where it is, and so does one
//! whose day file can't be read to tell. With none, no file is touched.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use serde_json::{Value, json};
use solstone_core_format::content::strava_body::{STRAVA_STREAM, STRAVA_TILE_FILE};
use solstone_core_import_sources::apple_health::facet_file_text;
use solstone_core_import_sources::apple_health_run::sync_folder;
use solstone_core_import_sources::body_stream::WindowFile;
use solstone_core_import_sources::strava::{Placement, TileAction};
use solstone_core_journal_io::{AtomicWriteOptions, atomic_replace};

use crate::body_activity::{Sensed, TOMBSTONE, window_rows};
use crate::cli_argv::{ImportFacet, require_enabled_facet};

const PRIVATE: AtomicWriteOptions = AtomicWriteOptions { mode: Some(0o600) };

/// Where a run's facet came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FacetBasis {
    /// The owner chose it for this import.
    Chosen,
    /// Nobody chose for this import, and it is the remembered default.
    Remembered,
    /// The owner chose no facet for this import.
    ChosenNone,
    /// Nobody chose for this import, and no enabled default is remembered.
    NoDefault,
}

/// The facet a run files its workouts under, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RunFacet {
    pub facet: Option<String>,
    pub basis: FacetBasis,
}

/// Decide a run's facet before anything is written. A facet the owner chose
/// that is muted or missing refuses the run.
pub(crate) fn run_facet(journal: &Path, given: &ImportFacet) -> Result<RunFacet, String> {
    Ok(match given {
        ImportFacet::Facet(facet) => {
            require_enabled_facet(facet, journal)?;
            RunFacet {
                facet: Some(facet.clone()),
                basis: FacetBasis::Chosen,
            }
        }
        ImportFacet::None => RunFacet {
            facet: None,
            basis: FacetBasis::ChosenNone,
        },
        ImportFacet::Unattended => {
            let remembered =
                solstone_core_facets::default_for_unattended(journal).unwrap_or_else(|error| {
                    log::warn!("Strava import: the remembered facet couldn't be read: {error}");
                    None
                });
            match remembered {
                Some(facet) => RunFacet {
                    facet: Some(facet),
                    basis: FacetBasis::Remembered,
                },
                None => RunFacet {
                    facet: None,
                    basis: FacetBasis::NoDefault,
                },
            }
        }
    })
}

/// The download's workouts by activity id: where each one's first tile was
/// placed, and its live tiles.
pub(crate) type LiveWorkouts = BTreeMap<u64, (Option<(String, String)>, Vec<LiveTile>)>;

/// One live tile of the download's workouts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LiveTile {
    pub day: String,
    pub segment: String,
    pub index: usize,
}

/// Every tile of the download's workouts that is live after this run's
/// writes, by activity id, with where its first tile was placed. Tiles this
/// run didn't manage to write, and keys it stopped at, are left out.
pub(crate) fn live_tiles(
    journal: &Path,
    placement: &Placement,
    written: &dyn Fn(&str, &str) -> bool,
) -> LiveWorkouts {
    let mut workouts = LiveWorkouts::new();
    for action in &placement.tiles {
        let (activity_id, day, segment, index, live) = match action {
            TileAction::Created {
                day,
                segment,
                slice,
                workout,
            }
            | TileAction::Updated {
                day,
                segment,
                slice,
                workout,
                ..
            } => (
                workout.activity_id,
                day,
                segment,
                slice.index,
                written(day, segment),
            ),
            TileAction::Unchanged {
                day,
                segment,
                activity_id,
                slice,
            } => (*activity_id, day, segment, slice.index, true),
            TileAction::StayedDeleted {
                day,
                segment,
                activity_id,
                slice,
            } => (*activity_id, day, segment, slice.index, false),
            TileAction::NoFreeKey { .. } | TileAction::DuplicateIdentity { .. } => continue,
        };
        let entry = workouts.entry(activity_id).or_default();
        if index == 0 && entry.0.is_none() {
            entry.0 = Some((day.clone(), segment.clone()));
        }
        let dir = tile_dir(journal, day, segment);
        if live && !dir.join(TOMBSTONE).exists() && dir.join(STRAVA_TILE_FILE).is_file() {
            entry.1.push(LiveTile {
                day: day.clone(),
                segment: segment.clone(),
                index,
            });
        }
    }
    workouts
}

fn tile_dir(journal: &Path, day: &str, segment: &str) -> std::path::PathBuf {
    journal
        .join("chronicle")
        .join(day)
        .join(STRAVA_STREAM)
        .join(segment)
}

/// What filing the download's workouts under a facet did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct Filing {
    /// Workouts whose tiles now carry the facet and didn't all before.
    pub filed: u64,
    /// Of those, workouts a tile of which had another facet before.
    pub moved: u64,
    /// Workouts an earlier reading of their tiles filed an activity record
    /// for, which keep their tiles' facet with that record.
    pub kept_sensed: u64,
    /// Workouts whose day file couldn't be read to tell, which keep their
    /// tiles' facet.
    pub kept_unreadable: u64,
    /// Tiles whose facet changed, `(day, segment)`, so search files them again.
    pub refiled_tiles: Vec<(String, String)>,
}

/// File every live tile of the download's workouts under `facet` wherever it
/// has another, except a sensed workout's and one whose day can't be read.
/// Call it before the record writer, so a new workout's record goes in `facet`.
pub(crate) fn file_strava_workouts(
    journal: &Path,
    workouts: &LiveWorkouts,
    facet: &str,
) -> Result<Filing, String> {
    let mut filing = Filing::default();
    let mut sensed = Sensed::default();
    let wanted = json!([{ "facet": facet }]);
    for (activity_id, (first, tiles)) in workouts {
        let Some(lowest) = tiles.iter().min_by_key(|tile| tile.index) else {
            continue;
        };
        let lowest_dir = tile_dir(journal, &lowest.day, &lowest.segment);
        let Some(mut row) = window_rows(&lowest_dir, WindowFile::StravaTile)
            .and_then(|rows| rows.into_iter().next())
        else {
            continue;
        };
        row.id = activity_id.to_string();
        let (day, window) = first
            .clone()
            .unwrap_or_else(|| (lowest.day.clone(), lowest.segment.clone()));
        match sensed.check(journal, &day, &window, &row) {
            Ok(false) => {}
            Ok(true) => {
                filing.kept_sensed += 1;
                continue;
            }
            Err(_) => {
                filing.kept_unreadable += 1;
                continue;
            }
        }
        let mut changed = false;
        let mut had_other = false;
        for tile in tiles {
            let dir = tile_dir(journal, &tile.day, &tile.segment);
            let path = dir.join("talents/facets.json");
            let current = fs::read_to_string(&path)
                .ok()
                .and_then(|text| serde_json::from_str::<Value>(&text).ok());
            if current.as_ref() == Some(&wanted) {
                continue;
            }
            had_other |= path.exists();
            fs::create_dir_all(dir.join("talents")).map_err(|error| error.to_string())?;
            atomic_replace(&path, facet_file_text(facet).as_bytes(), PRIVATE)
                .map_err(|error| error.to_string())?;
            sync_folder(&dir)?;
            filing
                .refiled_tiles
                .push((tile.day.clone(), tile.segment.clone()));
            changed = true;
        }
        if changed {
            filing.filed += 1;
            if had_other {
                filing.moved += 1;
            }
        }
    }
    Ok(filing)
}
