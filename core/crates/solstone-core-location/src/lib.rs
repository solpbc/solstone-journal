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
pub mod beside;
pub mod body_day;
pub mod body_hidden;
pub mod body_records;
mod cache;
pub mod day;
mod day_display;
pub mod fingerprint;
pub mod install;
pub mod old_imports;
pub mod pack;
pub mod places;
pub mod projection;
mod reconcile_io;
pub mod record;
pub mod segment_format;
pub mod stay;

pub use fingerprint::daily_location_fingerprint;

use solstone_core_window_catalog::{ENGLISH, strings, text};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

pub use area::{AreaName, AreaTier, Boundary, Bounds, TownPoint};
pub use asset::area_tier;
pub use beside::{Beside, DayBeside, location_beside, place_at};
pub use day::{
    DayLocation, DeviceDirectory, Entry, SegmentContext, SourceRef, read_day, read_days,
};
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
        &solstone_core_journal_config::owner_zone(journal),
    ))
}

/// Read several days with the journal's own device names and the owner's zone,
/// each day read once.
pub fn read_journal_days(
    journal: &Path,
    days: &[String],
) -> Result<Vec<DayLocation>, area::AreaTierError> {
    let tier = area_tier()?;
    let packs = accuracy::installed_packs(journal);
    Ok(day::read_days(
        journal,
        days,
        tier,
        packs.as_deref(),
        &JournalDirectory::read(journal),
        &solstone_core_journal_config::owner_zone(journal),
    ))
}

/// One reconcile pass over the whole journal with its own device names.
pub fn reconcile_journal(
    journal: &Path,
    reconciler: &mut Reconciler,
) -> Result<projection::ReconcileReport, area::AreaTierError> {
    // First, because it needs no area tier and a missing one mustn't pause it.
    reconciler.body.reconcile(journal);
    let tier = area_tier()?;
    let packs = accuracy::installed_packs(journal);
    Ok(reconciler.reconcile(
        journal,
        tier,
        packs.as_deref(),
        &JournalDirectory::read(journal),
        &solstone_core_journal_config::owner_zone(journal),
    ))
}

/// Reconcile under one retained owner's mutation authority on this native frame.
pub fn reconcile_journal_owned(
    root: std::sync::Arc<solstone_core_journal_io::JournalRoot>,
    reconciler: &mut Reconciler,
) -> std::io::Result<projection::ReconcileReport> {
    let _owner = reconcile_io::Scope::install(std::sync::Arc::clone(&root))?;
    let result =
        reconcile_journal(root.canonical_path(), reconciler).map_err(std::io::Error::other)?;
    reconcile_io::checkpoint()?;
    Ok(result)
}

/// Days that hold location, from their build records, with reading counts.
pub fn list_location_days(journal: &Path) -> Vec<(String, u64)> {
    location_days_where(journal, |_| true)
}

/// [`list_location_days`] for the days from `from` to `to`: no other day's
/// build record is opened.
pub fn list_location_days_between(journal: &Path, from: &str, to: &str) -> Vec<(String, u64)> {
    location_days_where(journal, |day| from <= day && day <= to)
}

/// The years with a day on or before `through` that [`list_location_days`]
/// lists. Each year's records are read in day order only until one holds.
pub fn location_years(journal: &Path, through: &str) -> BTreeSet<String> {
    let mut days: Vec<String> = solstone_core_journal_io::day_dirs(journal)
        .unwrap_or_default()
        .into_keys()
        .filter(|day| day.as_str() <= through)
        .collect();
    days.sort_unstable();
    let mut years = BTreeSet::new();
    for day in days {
        let Some(year) = day.get(..4) else { continue };
        if !years.contains(year) && projection::read_build_record(journal, &day).is_some() {
            years.insert(year.to_owned());
        }
    }
    years
}

fn location_days_where(journal: &Path, keep: impl Fn(&str) -> bool) -> Vec<(String, u64)> {
    let mut days: Vec<(String, u64)> = solstone_core_journal_io::day_dirs(journal)
        .unwrap_or_default()
        .into_keys()
        .filter(|day| keep(day))
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
        self.label_in(ENGLISH)
    }

    pub fn label_in(&self, locale: &str) -> String {
        if let Some(name) = self.place.owner_name_in(locale) {
            return name;
        }
        if let Some(words) = &self.words {
            if let Some(at) = &words.at {
                return strings(
                    locale,
                    "journal.window.map.area.label.at.place",
                    [("name", &at.name)],
                );
            }
            if let Some(near) = words.near.first() {
                return strings(
                    locale,
                    "journal.window.map.area.label.near.place",
                    [("name", &near.name)],
                );
            }
        }
        self.area
            .label_in(locale)
            .unwrap_or_else(|| text(locale, "journal.window.map.area.no.town"))
    }
}

/// Whole seconds since midnight on a wall clock.
pub(crate) fn seconds_into_day(time: jiff::civil::Time) -> u32 {
    let seconds =
        i32::from(time.hour()) * 3600 + i32::from(time.minute()) * 60 + i32::from(time.second());
    // Every field of a time of day is zero or more.
    seconds.unsigned_abs()
}

/// Every place in the journal with the stays it rests on, most recent first.
pub fn read_places(journal: &Path) -> Result<Vec<PlaceView>, area::AreaTierError> {
    let tier = area_tier()?;
    let packs = accuracy::installed_packs(journal);
    let store = places::PlaceStore::read(journal);
    let stays = projection::all_stays(journal);
    let zone = solstone_core_journal_config::owner_zone(journal);
    let mut views: Vec<PlaceView> = members_by_place(&store, &stays)
        .into_iter()
        .map(|(place, members)| place_view(place, members, tier, packs.as_deref(), &zone))
        .collect();
    views.sort_by(|a, b| {
        b.description
            .last
            .cmp(&a.description.last)
            .then_with(|| b.description.dates.cmp(&a.description.dates))
    });
    Ok(views)
}

/// One place [`read_places`] shows, by the days of its stays in a range.
#[derive(Debug, Clone)]
pub struct PlaceDays {
    pub place: places::Place,
    /// [`PlaceView::label`], as [`read_places`] gives it.
    pub label: String,
    /// The days from `from` to `to` it has a stay on.
    pub days: BTreeSet<String>,
}

/// The places [`read_places`] shows that have a stay on a day from `from` to
/// `to`, with those days and the same label, in no set order. Only those
/// days' build records are opened, unless the location-accuracy download is
/// installed: the words for a spot then weigh every stay it rests on, so
/// every day's are read.
pub fn read_place_days_between(
    journal: &Path,
    from: &str,
    to: &str,
) -> Result<Vec<PlaceDays>, area::AreaTierError> {
    let tier = area_tier()?;
    let packs = accuracy::installed_packs(journal);
    let store = places::PlaceStore::read(journal);
    let stays = if packs.is_some() {
        projection::all_stays(journal)
    } else {
        projection::stays_between(journal, from, to)
    };
    let zone = solstone_core_journal_config::owner_zone(journal);
    Ok(members_by_place(&store, &stays)
        .into_iter()
        .filter_map(|(place, members)| {
            let days: BTreeSet<String> = members
                .iter()
                .map(|stay| stay.day.clone())
                .filter(|day| from <= day.as_str() && day.as_str() <= to)
                .collect();
            if days.is_empty() {
                return None;
            }
            let view = place_view(place, members, tier, packs.as_deref(), &zone);
            Some(PlaceDays {
                label: view.label(),
                place: view.place,
                days,
            })
        })
        .collect())
}

/// Each shown place, in the store's order, with the stays it rests on in
/// day order. Each stay belongs to at most one place.
fn members_by_place<'a>(
    store: &'a places::PlaceStore,
    stays: &[places::StayRecord],
) -> Vec<(&'a places::Place, Vec<places::StayRecord>)> {
    let mut by_place: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    for (index, stay) in stays.iter().enumerate() {
        if let Some(place) = store.place_for(stay.latitude, stay.longitude, stay.margin_m) {
            by_place.entry(place.id.as_str()).or_default().push(index);
        }
    }
    store
        .places
        .iter()
        // A dormant place's stays are in the trash: it isn't shown.
        .filter(|place| !place.dormant)
        .map(|place| {
            let members: Vec<places::StayRecord> = by_place
                .get(place.id.as_str())
                .map(|indexes| indexes.iter().map(|&index| stays[index].clone()).collect())
                .unwrap_or_default();
            (place, members)
        })
        .collect()
}

fn place_view(
    place: &places::Place,
    members: Vec<places::StayRecord>,
    tier: &AreaTier,
    packs: Option<&accuracy::Packs>,
    zone: &jiff::tz::TimeZone,
) -> PlaceView {
    let offset = |at: i64| {
        jiff::Timestamp::new(at, 0).map_or(0, |utc| i64::from(zone.to_offset(utc).seconds()))
    };
    let description = places::describe(&members.iter().collect::<Vec<_>>(), offset);
    let clock = |at: Option<i64>| {
        at.and_then(|at| jiff::Timestamp::new(at, 0).ok())
            .map(|utc| {
                utc.to_zoned_datetime(zone.clone())
                    .strftime("%H:%M")
                    .to_string()
            })
    };
    let clocks = members
        .iter()
        .map(|stay| (clock(stay.start), clock(stay.end)))
        .collect();
    let mut area = tier.name_point(place.latitude, place.longitude);
    let mut words = None;
    if let Some((within, found)) = packs.and_then(|packs| {
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
}
