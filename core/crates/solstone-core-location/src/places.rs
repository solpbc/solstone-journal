// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Places: stays that recur at the same spot, and what the owner calls them.
//!
//! A place is worked out from the record, never from a guess about the owner:
//!
//! - only stays whose margin is [`PLACE_RADIUS_M`] or less take part, so a
//!   coarse position never makes or joins a place;
//! - a new place needs stays on two different dates whose centres all lie
//!   within [`PLACE_RADIUS_M`] of each other;
//! - a stay joins the place whose centre is within [`PLACE_RADIUS_M`]; places
//!   keep at least twice that apart, so there is never more than one;
//! - a place's centre is recomputed from the stays that survive, and a place
//!   with none left is gone, with whatever the owner called it.
//!
//! The owner may name a place, call it "home" or "work", or hide it from the
//! map. Nothing asks them to. Those choices live in
//! `solstone/apps/map/places.json` beside the places themselves, written only
//! here.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use solstone_core_journal_io::LockOptions;

use crate::pack::distance_m;

/// How close stays must be to make, or join, one place.
pub const PLACE_RADIUS_M: f64 = 50.0;
/// Places keep at least this far apart, twice the radius, so a stay is never
/// within reach of two.
const PLACE_SPACING_M: f64 = 2.0 * PLACE_RADIUS_M;

fn mean(stays: &[StayRecord], group: &[usize]) -> (f64, f64) {
    let count = group.len().max(1) as f64;
    (
        group
            .iter()
            .map(|&index| stays[index].latitude)
            .sum::<f64>()
            / count,
        group
            .iter()
            .map(|&index| stays[index].longitude)
            .sum::<f64>()
            / count,
    )
}
const STORE_VERSION: u32 = 1;
/// The longest name an owner may give a place.
pub const NAME_LIMIT: usize = 80;

/// The words an owner may add to a place: never inferred, never asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Alias {
    Home,
    Work,
}

impl Alias {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Home => "home",
            Self::Work => "work",
        }
    }
}

/// One stay as the places pass sees it, from a day's build record.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StayRecord {
    pub day: String,
    /// Seconds since the epoch, when known.
    pub start: Option<i64>,
    pub end: Option<i64>,
    pub latitude: f64,
    pub longitude: f64,
    pub margin_m: f64,
    /// The device key the stay came from.
    pub device: String,
    /// `reported` (a visit the phone reported) or `worked` (worked out from
    /// readings).
    pub kind: String,
}

impl StayRecord {
    fn eligible(&self) -> bool {
        self.margin_m <= PLACE_RADIUS_M
    }

    fn identity(&self) -> String {
        format!(
            "{}|{}|{:?}|{:.6}|{:.6}",
            self.day, self.device, self.start, self.latitude, self.longitude
        )
    }
}

/// One place.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Place {
    pub id: String,
    pub latitude: f64,
    pub longitude: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alias: Option<Alias>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub hidden: bool,
}

impl Place {
    /// What the owner calls it, if anything: their name, else their alias.
    pub fn owner_name(&self) -> Option<String> {
        self.name
            .clone()
            .or_else(|| self.alias.map(|alias| alias.as_str().to_owned()))
    }
}

/// `solstone/apps/map/places.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlaceStore {
    pub version: u32,
    pub places: Vec<Place>,
}

impl Default for PlaceStore {
    fn default() -> Self {
        Self {
            version: STORE_VERSION,
            places: Vec::new(),
        }
    }
}

pub fn store_path(journal: &Path) -> PathBuf {
    journal
        .join("solstone")
        .join("apps")
        .join("map")
        .join("places.json")
}

impl PlaceStore {
    pub fn read(journal: &Path) -> Self {
        crate::reconcile_io::read(&store_path(journal))
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Self>(&bytes).ok())
            .filter(|store| store.version == STORE_VERSION)
            .unwrap_or_default()
    }

    /// Read the store for a fingerprint.
    ///
    /// A missing file is an empty store. IO, invalid JSON, and a wrong version
    /// are errors. This read takes no place-store lock.
    pub fn read_strict(journal: &Path) -> std::io::Result<Self> {
        let bytes = match crate::reconcile_io::read(&store_path(journal)) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default());
            }
            Err(error) => return Err(error),
        };
        let store: Self = serde_json::from_slice(&bytes)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
        if store.version != STORE_VERSION {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "place store version",
            ));
        }
        Ok(store)
    }

    pub fn write(&self, journal: &Path) -> std::io::Result<()> {
        let path = store_path(journal);
        let mut bytes = serde_json::to_vec_pretty(self).unwrap_or_default();
        bytes.push(b'\n');
        crate::reconcile_io::replace(&path, &bytes)
    }

    /// Read, change and write the store under its lock, so the background
    /// pass and an owner's edit never overwrite each other. Writes only when
    /// the store changed. Returns what `change` returned.
    pub fn update<T>(journal: &Path, change: impl FnOnce(&mut Self) -> T) -> std::io::Result<T> {
        crate::reconcile_io::transaction(|| Self::update_inner(journal, change))
    }

    fn update_inner<T>(journal: &Path, change: impl FnOnce(&mut Self) -> T) -> std::io::Result<T> {
        let _admission = crate::reconcile_io::hold_lock(
            &solstone_core_journal_io::locking::daily_location_admission_path(journal),
            LockOptions::default(),
            false,
        )?;
        let path = store_path(journal);
        let _lock = crate::reconcile_io::hold_lock(&path, LockOptions::default(), false)?;
        let mut store = if crate::reconcile_io::is_owned() {
            Self::read_strict(journal)?
        } else {
            Self::read(journal)
        };
        let before = store.clone();
        let result = change(&mut store);
        if store != before {
            store.write(journal)?;
        }
        Ok(result)
    }

    pub fn get(&self, id: &str) -> Option<&Place> {
        self.places.iter().find(|place| place.id == id)
    }

    /// The place a stay belongs to: the only one whose centre is close enough.
    pub fn place_for(&self, latitude: f64, longitude: f64, margin_m: f64) -> Option<&Place> {
        if margin_m > PLACE_RADIUS_M {
            return None;
        }
        let mut close = self.places.iter().filter(|place| {
            distance_m(place.latitude, place.longitude, latitude, longitude) <= PLACE_RADIUS_M
        });
        let first = close.next()?;
        close.next().is_none().then_some(first)
    }

    /// Whether a position is within reach of a place the owner hid from the
    /// map. Everything there stays off the map, whatever it is.
    pub fn hides(&self, latitude: f64, longitude: f64) -> bool {
        self.places.iter().any(|place| {
            place.hidden
                && distance_m(place.latitude, place.longitude, latitude, longitude)
                    <= PLACE_RADIUS_M
        })
    }

    /// A fingerprint of every name and alias, so day documents that use them
    /// are rewritten when the owner changes one.
    pub fn names_fingerprint(&self) -> String {
        let mut digest = Sha256::new();
        for place in &self.places {
            if let Some(name) = place.owner_name() {
                digest.update(place.id.as_bytes());
                digest.update([0]);
                digest.update(name.as_bytes());
                digest.update([0]);
            }
        }
        format!("{:x}", digest.finalize())
    }
}

/// Recompute the places from every surviving stay. Returns the stays each
/// place rests on, by place id.
pub fn recompute(store: &mut PlaceStore, stays: &[StayRecord]) -> BTreeMap<String, Vec<usize>> {
    let eligible: Vec<usize> = (0..stays.len())
        .filter(|&index| stays[index].eligible())
        .collect();
    let mut members: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    let mut unassigned = Vec::new();
    for &index in &eligible {
        let stay = &stays[index];
        match store.place_for(stay.latitude, stay.longitude, stay.margin_m) {
            Some(place) => members.entry(place.id.clone()).or_default().push(index),
            None => {
                let near_any = store.places.iter().any(|place| {
                    distance_m(
                        place.latitude,
                        place.longitude,
                        stay.latitude,
                        stay.longitude,
                    ) <= PLACE_RADIUS_M
                });
                // Close to two places is ambiguous: it joins neither and
                // starts nothing.
                if !near_any {
                    unassigned.push(index);
                }
            }
        }
    }
    // New places from what is left: a stay, and every later one within the
    // radius of all already chosen, on at least two dates.
    let mut used: BTreeSet<usize> = BTreeSet::new();
    unassigned.sort_by_key(|&index| (stays[index].day.clone(), stays[index].start));
    for (position, &seed) in unassigned.iter().enumerate() {
        if used.contains(&seed) {
            continue;
        }
        let mut group = vec![seed];
        for &other in &unassigned[position + 1..] {
            if used.contains(&other) {
                continue;
            }
            let fits = group.iter().all(|&chosen| {
                distance_m(
                    stays[chosen].latitude,
                    stays[chosen].longitude,
                    stays[other].latitude,
                    stays[other].longitude,
                ) <= PLACE_RADIUS_M
            });
            if fits {
                group.push(other);
            }
        }
        let dates: BTreeSet<&str> = group
            .iter()
            .map(|&index| stays[index].day.as_str())
            .collect();
        if dates.len() < 2 {
            continue;
        }
        // A new place keeps clear of every other, so no stay is ever within
        // reach of two places.
        let (latitude, longitude) = mean(stays, &group);
        if store.places.iter().any(|place| {
            distance_m(place.latitude, place.longitude, latitude, longitude) <= PLACE_SPACING_M
        }) {
            continue;
        }
        used.extend(group.iter().copied());
        let id = {
            let digest = Sha256::digest(stays[seed].identity().as_bytes());
            format!("{:x}", digest)[..12].to_owned()
        };
        store.places.push(Place {
            id: id.clone(),
            latitude,
            longitude,
            name: None,
            alias: None,
            hidden: false,
        });
        members.insert(id, group);
    }
    // Centres follow the surviving stays, unless the move would leave one of
    // them out of reach or bring the place within reach of another, so a
    // place never walks away from the stays it rests on. A place with none
    // left is gone.
    store.places.retain(|place| members.contains_key(&place.id));
    for index in 0..store.places.len() {
        let group = &members[&store.places[index].id];
        let (latitude, longitude) = mean(stays, group);
        let strands = group.iter().any(|&member| {
            distance_m(
                latitude,
                longitude,
                stays[member].latitude,
                stays[member].longitude,
            ) > PLACE_RADIUS_M
        });
        let crowded = strands
            || store.places.iter().enumerate().any(|(other, place)| {
                other != index
                    && distance_m(place.latitude, place.longitude, latitude, longitude)
                        <= PLACE_SPACING_M
            });
        if !crowded {
            store.places[index].latitude = latitude;
            store.places[index].longitude = longitude;
        }
    }
    members
}

/// What the record says about one place.
#[derive(Debug, Clone, PartialEq, Default, Serialize)]
pub struct Description {
    pub dates: usize,
    pub first: Option<String>,
    pub last: Option<String>,
    /// Dates with at least two hours of stay between 22:00 and 06:00, in the
    /// clock given.
    pub night_dates: usize,
}

/// Count the dates a place was recorded on, and the nighttime ones.
pub fn describe(stays: &[&StayRecord], offset_seconds: impl Fn(i64) -> i64) -> Description {
    let mut dates: BTreeSet<&str> = BTreeSet::new();
    let mut nights: BTreeSet<String> = BTreeSet::new();
    for stay in stays {
        dates.insert(&stay.day);
        let (Some(start), Some(end)) = (stay.start, stay.end) else {
            continue;
        };
        // Walk the stay in local time and count its seconds inside each
        // night window (22:00 to 06:00), keyed by the night's evening date.
        let mut night_seconds: BTreeMap<i64, i64> = BTreeMap::new();
        let mut at = start;
        while at < end {
            let local = at + offset_seconds(at);
            let second_of_day = local.rem_euclid(86_400);
            let day_number = local.div_euclid(86_400);
            let (inside, until, night) = if second_of_day >= 22 * 3600 {
                (true, 86_400 - second_of_day, day_number)
            } else if second_of_day < 6 * 3600 {
                (true, 6 * 3600 - second_of_day, day_number - 1)
            } else {
                (false, 22 * 3600 - second_of_day, 0)
            };
            let step = until.min(end - at).max(1);
            if inside {
                *night_seconds.entry(night).or_default() += step;
            }
            at += step;
        }
        for (night, seconds) in night_seconds {
            if seconds >= 2 * 3600 {
                nights.insert(night.to_string());
            }
        }
    }
    Description {
        dates: dates.len(),
        first: dates.iter().next().map(|day| (*day).to_owned()),
        last: dates.iter().next_back().map(|day| (*day).to_owned()),
        night_dates: nights.len(),
    }
}

/// Clean an owner's name: trimmed, one line, at most [`NAME_LIMIT`] characters.
pub fn clean_name(name: &str) -> Option<String> {
    let one_line: String = name
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect();
    let trimmed = one_line.split_whitespace().collect::<Vec<_>>().join(" ");
    (!trimmed.is_empty()).then(|| trimmed.chars().take(NAME_LIMIT).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(feature = "full-tests")]
    use std::fs;
    #[cfg(feature = "full-tests")]
    use std::sync::mpsc;
    #[cfg(feature = "full-tests")]
    use std::thread;

    #[cfg(feature = "full-tests")]
    #[test]
    fn place_update_waits_for_the_admission_guard() {
        let journal = tempfile::tempdir().unwrap();
        let store = PlaceStore {
            version: 1,
            places: vec![Place {
                id: "place-1".to_owned(),
                latitude: 1.0,
                longitude: 2.0,
                name: Some("old".to_owned()),
                alias: None,
                hidden: false,
            }],
        };
        store.write(journal.path()).unwrap();
        let before = fs::read(store_path(journal.path())).unwrap();
        let guard =
            solstone_core_journal_io::hold_daily_location_admission(journal.path()).unwrap();
        let path = journal.path().to_path_buf();
        let (sender, receiver) = mpsc::channel();
        let (ready_sender, ready) = mpsc::channel();
        let worker = thread::spawn(move || {
            ready_sender.send(()).unwrap();
            PlaceStore::update(&path, |store| {
                store.places[0].name = Some("new".to_owned());
            })
            .unwrap();
            sender.send(()).unwrap();
        });
        ready.recv().unwrap();
        assert!(matches!(
            receiver.recv_timeout(std::time::Duration::from_millis(100)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ));
        assert_eq!(fs::read(store_path(journal.path())).unwrap(), before);
        drop(guard);
        receiver.recv().unwrap();
        worker.join().unwrap();
        let after = fs::read_to_string(store_path(journal.path())).unwrap();
        assert!(after.contains("new"));
        assert!(!after.contains("old"));
    }

    const LAT: f64 = 39.7527;
    const LON: f64 = -105.0003;

    fn stay(day: &str, east_m: f64, margin_m: f64) -> StayRecord {
        StayRecord {
            day: day.to_owned(),
            start: Some(1_759_330_000),
            end: Some(1_759_333_600),
            latitude: LAT,
            longitude: LON + east_m / (111_195.0 * LAT.to_radians().cos()),
            margin_m,
            device: "phone".to_owned(),
            kind: "reported".to_owned(),
        }
    }

    #[test]
    fn a_place_needs_two_dates() {
        let mut store = PlaceStore::default();
        recompute(
            &mut store,
            &[stay("20261001", 0.0, 25.0), stay("20261001", 5.0, 25.0)],
        );
        assert!(store.places.is_empty(), "one date is not a place");
        let members = recompute(
            &mut store,
            &[stay("20261001", 0.0, 25.0), stay("20261003", 10.0, 30.0)],
        );
        assert_eq!(store.places.len(), 1);
        assert_eq!(members[&store.places[0].id].len(), 2);
    }

    #[test]
    fn coarse_stays_never_make_or_join_a_place() {
        let mut store = PlaceStore::default();
        recompute(
            &mut store,
            &[stay("20261001", 0.0, 80.0), stay("20261002", 0.0, 90.0)],
        );
        assert!(store.places.is_empty());
    }

    #[test]
    fn an_identity_and_its_owner_name_survive_new_stays() {
        let mut store = PlaceStore::default();
        let mut stays = vec![stay("20261001", 0.0, 25.0), stay("20261002", 0.0, 25.0)];
        recompute(&mut store, &stays);
        let id = store.places[0].id.clone();
        store.places[0].name = Some("the climbing gym".to_owned());
        stays.push(stay("20261005", 20.0, 25.0));
        let members = recompute(&mut store, &stays);
        assert_eq!(store.places.len(), 1);
        assert_eq!(store.places[0].id, id);
        assert_eq!(members[&id].len(), 3);
        assert_eq!(
            store.places[0].owner_name().as_deref(),
            Some("the climbing gym")
        );
    }

    #[test]
    fn a_place_whose_stays_are_all_deleted_is_gone_with_its_name() {
        let mut store = PlaceStore::default();
        recompute(
            &mut store,
            &[stay("20261001", 0.0, 25.0), stay("20261002", 0.0, 25.0)],
        );
        store.places[0].alias = Some(Alias::Home);
        recompute(&mut store, &[]);
        assert!(store.places.is_empty());
    }

    #[test]
    fn places_further_apart_than_the_radius_stay_separate() {
        let mut store = PlaceStore::default();
        recompute(
            &mut store,
            &[
                stay("20261001", 0.0, 25.0),
                stay("20261002", 0.0, 25.0),
                stay("20261001", 200.0, 25.0),
                stay("20261002", 200.0, 25.0),
            ],
        );
        assert_eq!(store.places.len(), 2);
    }

    #[test]
    fn places_keep_clear_of_each_other_so_a_stay_never_has_two() {
        let mut store = PlaceStore::default();
        let mut stays = vec![
            stay("20261001", 0.0, 25.0),
            stay("20261002", 0.0, 25.0),
            stay("20261001", 70.0, 25.0),
            stay("20261002", 70.0, 25.0),
        ];
        recompute(&mut store, &stays);
        assert_eq!(
            store.places.len(),
            1,
            "a second place 70 m away would crowd the first"
        );
        stays.push(stay("20261004", 35.0, 25.0));
        let members = recompute(&mut store, &stays);
        assert_eq!(members.values().map(Vec::len).sum::<usize>(), 3);
        // Far enough away, a second place forms.
        stays.push(stay("20261001", 300.0, 25.0));
        stays.push(stay("20261003", 300.0, 25.0));
        recompute(&mut store, &stays);
        assert_eq!(store.places.len(), 2);
    }

    #[test]
    fn a_place_never_walks_away_from_the_stays_it_rests_on() {
        let mut store = PlaceStore::default();
        let mut stays = vec![stay("20261001", 0.0, 25.0), stay("20261002", 0.0, 25.0)];
        recompute(&mut store, &stays);
        for (index, east) in [45.0, 48.0, 49.0, 49.5].iter().enumerate() {
            stays.push(stay(&format!("2026101{index}"), *east, 25.0));
            recompute(&mut store, &stays);
        }
        let members = recompute(&mut store, &stays);
        let id = store.places[0].id.clone();
        assert!(
            members[&id].contains(&0) && members[&id].contains(&1),
            "the founding stays stay members"
        );
        // Deleting the later days leaves the place resting on its first two.
        stays.truncate(2);
        recompute(&mut store, &stays);
        assert_eq!(store.places.len(), 1);
        assert_eq!(store.places[0].id, id);
    }

    #[test]
    fn nights_count_two_hours_between_ten_and_six() {
        // 23:00 to 07:00 UTC with no offset: seven hours of night.
        let night = StayRecord {
            start: Some(23 * 3600),
            end: Some(31 * 3600),
            ..stay("19700101", 0.0, 25.0)
        };
        // 12:00 to 18:00: none.
        let day = StayRecord {
            start: Some(12 * 3600 + 86_400),
            end: Some(18 * 3600 + 86_400),
            ..stay("19700102", 0.0, 25.0)
        };
        let description = describe(&[&night, &day], |_| 0);
        assert_eq!(description.dates, 2);
        assert_eq!(description.night_dates, 1);
        assert_eq!(description.first.as_deref(), Some("19700101"));
        assert_eq!(description.last.as_deref(), Some("19700102"));
    }

    #[test]
    fn owner_names_are_cleaned() {
        assert_eq!(
            clean_name("  my\nfavourite   café "),
            Some("my favourite café".to_owned())
        );
        assert_eq!(clean_name("   "), None);
        assert_eq!(
            clean_name(&"x".repeat(200)).map(|name| name.len()),
            Some(NAME_LIMIT)
        );
    }

    #[test]
    fn the_store_round_trips_under_its_lock() {
        let journal = tempfile::tempdir().unwrap();
        PlaceStore::update(journal.path(), |store| {
            recompute(
                store,
                &[stay("20261001", 0.0, 25.0), stay("20261002", 0.0, 25.0)],
            );
            store.places[0].hidden = true;
        })
        .unwrap();
        let read = PlaceStore::read(journal.path());
        assert_eq!(read.places.len(), 1);
        assert!(read.places[0].hidden);
    }
}
