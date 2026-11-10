// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The facet a Strava import files its workouts under, and the filing itself.
//!
//! A run is given a facet, none, or nothing. With nothing (a command line
//! without `--facet`, or an import page that sent no facet) it uses the
//! remembered default while that facet is still enabled, and otherwise none;
//! it never makes a facet and never asks. Genuinely new workouts use that facet
//! in `talents/facets.json`. Only an owner's answer moves an earlier filing,
//! including one surviving solely on a tile. Otherwise existing tiles keep
//! their assignments and missing assignments inherit the workout's earlier
//! filing, even when this run has no facet. A workout whose day file can't be
//! read keeps its tiles' facet.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use serde_json::Value;
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

pub(crate) fn validate_assignments(journal: &Path, workouts: &LiveWorkouts) -> Result<(), String> {
    for (_, tiles) in workouts.values() {
        for tile in tiles {
            let path = tile_dir(journal, &tile.day, &tile.segment).join("talents/facets.json");
            match fs::read_to_string(&path) {
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(format!("{}: {error}", path.display())),
            }
        }
    }
    Ok(())
}

/// What filing the download's workouts under a facet did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct Filing {
    /// Workouts whose tiles now carry the facet and didn't all before.
    pub filed: u64,
    /// Of those, workouts a tile of which had another facet before.
    pub moved: u64,
    /// Workouts already listed with the owner's activities, by an earlier
    /// reading of their tiles or an earlier import, which keep their tiles'
    /// facet beside that record, so a listing and its workout never split.
    pub kept_sensed: u64,
    /// Earlier tile filings retained even when the activity record is absent.
    pub kept_filed: BTreeMap<String, u64>,
    /// Workouts whose day file couldn't be read to tell, which keep their
    /// tiles' facet.
    pub kept_unreadable: u64,
    /// Tiles whose facet changed, `(day, segment)`, so search files them again.
    pub refiled_tiles: Vec<(String, String)>,
}

/// Told the tiles a filing is about to change, `(day, segment)`, before any
/// is written; an error stops the filing.
pub(crate) type BeforeRefile<'a> = &'a mut dyn FnMut(&[(String, String)]) -> Result<(), String>;

/// File new workouts under `facet`, preserving earlier tile assignments unless
/// the owner permits a move. Missing tile assignments inherit the workout's
/// known filing. A workout whose day can't be read stays unchanged.
/// `before_write` receives every changing tile before any assignment is written,
/// so an interrupted run still knows which tiles to publish again. Call before
/// the record writer, so its new record uses the workout's actual facet.
pub(crate) fn file_strava_workouts(
    journal: &Path,
    workouts: &LiveWorkouts,
    facet: Option<&str>,
    may_move_listed: bool,
    before_write: BeforeRefile<'_>,
) -> Result<Filing, String> {
    let mut filing = Filing::default();
    let mut sensed = Sensed::default();
    let wanted = facet.map(facet_file_text);
    // Per workout to file: whether a tile had another facet, and its tiles
    // that change.
    let mut planned = Vec::new();
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
        let listed = sensed
            .facet(journal, &day, &window, &row)
            .and_then(|sensed| {
                if sensed.is_some() {
                    return Ok(sensed);
                }
                crate::body_activity::record_facet(
                    journal,
                    &day,
                    &crate::body_activity::account_key(STRAVA_STREAM, &activity_id.to_string()),
                )
            });
        let listed = match listed {
            Ok(facet) => facet,
            Err(_) => {
                filing.kept_unreadable += 1;
                continue;
            }
        };
        let mut current_tiles = Vec::new();
        let mut previous: Option<(usize, String)> = None;
        for tile in tiles {
            let path = tile_dir(journal, &tile.day, &tile.segment).join("talents/facets.json");
            let current = match fs::read_to_string(&path) {
                Ok(text) => Some(text),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                Err(error) => return Err(format!("{}: {error}", path.display())),
            };
            if let Some(text) = &current
                && previous
                    .as_ref()
                    .is_none_or(|(index, _)| tile.index < *index)
            {
                previous = Some((tile.index, text.clone()));
            }
            current_tiles.push((tile, current));
        }
        let kept = !may_move_listed && (listed.is_some() || previous.is_some());
        let target = if kept {
            previous
                .map(|(_, text)| text)
                .or_else(|| listed.as_deref().map(facet_file_text))
        } else {
            wanted.clone()
        };
        let Some(target) = target else {
            continue;
        };
        if kept && listed.is_some() {
            filing.kept_sensed += 1;
        } else if kept && Some(target.as_str()) != wanted.as_deref() {
            *filing
                .kept_filed
                .entry(crate::body_segments::filing_words(journal, &target))
                .or_default() += 1;
        }
        let counts_as_filed = facet.is_some()
            && (!kept || (listed.is_none() && Some(target.as_str()) == wanted.as_deref()));
        let target_value = serde_json::from_str::<Value>(&target).ok();
        let mut had_other = false;
        let mut changing = Vec::new();
        for (tile, current) in current_tiles {
            if kept && current.is_some() {
                continue;
            }
            if current
                .as_deref()
                .and_then(|text| serde_json::from_str::<Value>(text).ok())
                == target_value
                && current.is_some()
            {
                continue;
            }
            had_other |= current.is_some();
            changing.push((tile.day.clone(), tile.segment.clone()));
        }
        if !changing.is_empty() {
            planned.push((counts_as_filed, had_other, target, changing));
        }
    }
    let tiles = planned
        .iter()
        .flat_map(|(_, _, _, changing)| changing.iter().cloned())
        .collect::<Vec<_>>();
    if tiles.is_empty() {
        return Ok(filing);
    }
    before_write(&tiles)?;
    for (counts_as_filed, had_other, target, changing) in planned {
        for (day, segment) in changing {
            let dir = tile_dir(journal, &day, &segment);
            fs::create_dir_all(dir.join("talents")).map_err(|error| error.to_string())?;
            atomic_replace(dir.join("talents/facets.json"), target.as_bytes(), PRIVATE)
                .map_err(|error| error.to_string())?;
            sync_folder(&dir)?;
            filing.refiled_tiles.push((day, segment));
        }
        filing.filed += u64::from(counts_as_filed);
        if counts_as_filed && had_other {
            filing.moved += 1;
        }
    }
    Ok(filing)
}
