// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Location in the journal: reading what the phones send, naming it at area
//! level from the bundled area tier, and the one writer of location-derived
//! files.
//!
//! Everything here runs on the journal. Naming is a lookup in a file that
//! ships byte-for-byte the same to every journal, so no service is asked
//! where anyone was.
//!
//! This crate owns two derived files per day (see [`projection`]):
//! `chronicle/<day>/location.md` and `chronicle/<day>/health/location.json`.
//! The captured `location.jsonl` in each segment is never written here.

pub mod accuracy;
pub mod area;
pub mod asset;
mod cache;
pub mod day;
pub mod fingerprint;
pub mod install;
pub mod pack;
pub mod places;
pub mod projection;
pub mod record;
pub mod stay;

pub use fingerprint::daily_location_fingerprint;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub use area::{AreaName, AreaTier, Boundary, Bounds, TownPoint};
pub use asset::area_tier;
pub use day::{DayLocation, DeviceDirectory, Entry, SegmentContext, SourceRef, read_day};
pub use projection::{Reconciler, rebuild_day};

/// Device names from the journal itself: each segment's upload record, and the
/// description a paired client gave of itself.
pub struct JournalDirectory {
    descriptions: BTreeMap<String, (Option<String>, Option<String>)>,
}

impl JournalDirectory {
    pub fn read(journal: &Path) -> Self {
        let descriptions = solstone_core_sol_link::read_descriptions(journal)
            .unwrap_or_default()
            .into_iter()
            .map(|(cid, description)| {
                let reported = description.reported.as_ref();
                let name = description
                    .owner_label
                    .clone()
                    .or_else(|| reported.and_then(|reported| reported.name.clone()))
                    .filter(|name| !name.trim().is_empty());
                let device_type = reported.and_then(|reported| reported.device_type.clone());
                (cid, (name, device_type))
            })
            .collect();
        Self { descriptions }
    }
}

impl DeviceDirectory for JournalDirectory {
    fn segment_context(&self, segment_dir: &Path) -> SegmentContext {
        let Ok(report) = solstone_core_callosum::read_device_ingest_events(segment_dir) else {
            return SegmentContext::default();
        };
        let Some(record) = report.records.last() else {
            return SegmentContext::default();
        };
        let zone = report
            .records
            .iter()
            .rev()
            .find_map(solstone_core_callosum::DeviceIngestEvent::reported_zone);
        SegmentContext {
            cid: Some(record.cid.clone()).filter(|cid| !cid.is_empty()),
            platform: record
                .meta
                .get("platform")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned),
            tz: zone.as_ref().and_then(|zone| zone.tz.clone()),
            utc_offset_seconds: zone.and_then(|zone| zone.utc_offset_seconds),
        }
    }

    fn describe(&self, cid: &str) -> (Option<String>, Option<String>) {
        self.descriptions.get(cid).cloned().unwrap_or((None, None))
    }
}

/// Read one day with the journal's own device names and the owner's zone.
pub fn read_journal_day(journal: &Path, day: &str) -> Result<DayLocation, area::AreaTierError> {
    let tier = area_tier()?;
    let packs = accuracy::installed_packs(journal);
    Ok(read_day(
        journal,
        day,
        tier,
        packs.as_deref(),
        &JournalDirectory::read(journal),
        solstone_core_journal_config::owner_zone(journal),
    ))
}

/// One reconcile pass over the whole journal with its own device names.
pub fn reconcile_journal(
    journal: &Path,
    reconciler: &mut Reconciler,
) -> Result<projection::ReconcileReport, area::AreaTierError> {
    let tier = area_tier()?;
    let packs = accuracy::installed_packs(journal);
    Ok(reconciler.reconcile(
        journal,
        tier,
        packs.as_deref(),
        &JournalDirectory::read(journal),
        solstone_core_journal_config::owner_zone(journal),
    ))
}

/// Days that hold location, from their build records, with reading counts.
pub fn list_location_days(journal: &Path) -> Vec<(String, u64)> {
    let mut days: Vec<(String, u64)> = solstone_core_journal_io::day_dirs(journal)
        .unwrap_or_default()
        .into_keys()
        .filter_map(|day| {
            let record = projection::read_build_record(journal, &day)?;
            Some((day, (record.readings + record.visits).max(1)))
        })
        .collect();
    days.sort();
    days
}

/// The installed or checkout path of the area tier, for diagnostics.
pub fn area_tier_path() -> Result<PathBuf, asset::AssetError> {
    asset::resolve_area_tier_path()
}

/// One place as the map app shows it: the owner's choices, the stays it
/// rests on, what the record says about it, and the words for where it is.
#[derive(Debug, Clone)]
pub struct PlaceView {
    pub place: places::Place,
    pub stays: Vec<places::StayRecord>,
    /// Each stay's start and end as a clock in the owner's zone.
    pub clocks: Vec<(Option<String>, Option<String>)>,
    pub description: places::Description,
    /// The area or containment words for where it is.
    pub area: AreaName,
    /// What the location-accuracy download says about the spot, if anything.
    pub words: Option<accuracy::StopWords>,
}

impl PlaceView {
    /// The name to show: the owner's, else the best words for the spot.
    pub fn label(&self) -> String {
        if let Some(name) = self.place.owner_name() {
            return name;
        }
        if let Some(words) = &self.words {
            if let Some(at) = &words.at {
                return format!("at {}", at.name);
            }
            if let Some(near) = words.near.first() {
                return format!("near {}", near.name);
            }
        }
        self.area
            .label()
            .unwrap_or_else(|| "a place with no named town nearby".to_owned())
    }
}

/// Every place in the journal with the stays it rests on, most recent first.
pub fn read_places(journal: &Path) -> Result<Vec<PlaceView>, area::AreaTierError> {
    let tier = area_tier()?;
    let packs = accuracy::installed_packs(journal);
    let store = places::PlaceStore::read(journal);
    let stays = projection::all_stays(journal);
    let zone = solstone_core_journal_config::owner_zone(journal);
    let offset = |at: i64| {
        use chrono::{Offset, TimeZone};
        chrono::DateTime::from_timestamp(at, 0).map_or(0, |utc| {
            i64::from(
                zone.offset_from_utc_datetime(&utc.naive_utc())
                    .fix()
                    .local_minus_utc(),
            )
        })
    };
    // Each stay belongs to at most one place; find it once.
    let mut by_place: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    for (index, stay) in stays.iter().enumerate() {
        if let Some(place) = store.place_for(stay.latitude, stay.longitude, stay.margin_m) {
            by_place.entry(place.id.as_str()).or_default().push(index);
        }
    }
    let mut views: Vec<PlaceView> = store
        .places
        .iter()
        .map(|place| {
            let members: Vec<places::StayRecord> = by_place
                .get(place.id.as_str())
                .map(|indexes| indexes.iter().map(|&index| stays[index].clone()).collect())
                .unwrap_or_default();
            let description = places::describe(&members.iter().collect::<Vec<_>>(), offset);
            let clock = |at: Option<i64>| {
                at.and_then(|at| chrono::DateTime::from_timestamp(at, 0))
                    .map(|utc| utc.with_timezone(&zone).format("%H:%M").to_string())
            };
            let clocks = members
                .iter()
                .map(|stay| (clock(stay.start), clock(stay.end)))
                .collect();
            let mut area = tier.name_point(place.latitude, place.longitude);
            let mut words = None;
            if let Some((within, found)) = packs.as_deref().and_then(|packs| {
                packs.stop(
                    place.latitude,
                    place.longitude,
                    // A circle from the centre that holds every stay with its
                    // own margin, so "at" holds for each of them.
                    Some(
                        members
                            .iter()
                            .map(|stay| {
                                pack::distance_m(
                                    place.latitude,
                                    place.longitude,
                                    stay.latitude,
                                    stay.longitude,
                                ) + stay.margin_m
                            })
                            .fold(accuracy::MARGIN_FLOOR_M, f64::max),
                    ),
                )
            }) {
                area.within = within;
                words = Some(found).filter(|found| !found.is_empty());
            }
            PlaceView {
                place: place.clone(),
                stays: members,
                clocks,
                description,
                area,
                words,
            }
        })
        .collect();
    views.sort_by(|a, b| {
        b.description
            .last
            .cmp(&a.description.last)
            .then_with(|| b.description.dates.cmp(&a.description.dates))
    });
    Ok(views)
}
