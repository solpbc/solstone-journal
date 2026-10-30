// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Where a device was while an activity ran: the day's location entry that
//! covers the activity's middle, in the words the map uses. It says what a
//! device reported, never where the owner was, and never a coordinate.

use std::path::Path;

use chrono::{NaiveDate, Timelike};

use crate::day::{DayLocation, Entry, LocalTime};
use crate::places::PlaceStore;
use crate::record::Precision;

const DAY_SECONDS: u32 = 24 * 60 * 60;

/// What sits beside one activity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Beside {
    /// "your iphone", "your watch".
    pub device: String,
    /// "near Golden, Colorado", "at home".
    pub words: String,
    /// The place the spot belongs to, when the journal has worked one out.
    pub place_id: Option<String>,
}

/// One day's location and the owner's places, read once for every activity
/// that day. A day without location, or one that cannot be read, has nothing
/// beside any activity.
pub struct DayBeside {
    day: Option<DayLocation>,
    store: PlaceStore,
}

impl DayBeside {
    pub fn read(journal: &Path, day: &str) -> Self {
        let located = crate::projection::read_build_record(journal, day)
            .and_then(|_| crate::read_journal_day(journal, day).ok());
        Self {
            day: located,
            store: PlaceStore::read(journal),
        }
    }

    /// `span` is the activity's `[start, end)` in seconds of the day on the
    /// recording device's clock, as its segment keys are written; `stream`
    /// is the activity's stream.
    pub fn beside(&self, span: (u32, u32), stream: Option<&str>) -> Option<Beside> {
        location_beside(self.day.as_ref()?, &self.store, span, stream)
    }
}

/// The entry covering the middle of `span`. The device that recorded the
/// activity wins over another, a visit or stay over loose readings, precise
/// readings over approximate ones, and a shorter entry over a longer one.
pub fn location_beside(
    day: &DayLocation,
    store: &PlaceStore,
    span: (u32, u32),
    stream: Option<&str>,
) -> Option<Beside> {
    let middle = span.0 + span.1.saturating_sub(span.0) / 2;
    day.entries
        .iter()
        .filter_map(|entry| {
            let (start, end) = covered(entry, &day.day)?;
            (start <= middle && middle <= end).then_some((entry, end - start))
        })
        .max_by_key(|(entry, length)| {
            let same_stream = stream
                .is_some_and(|stream| entry.sources().iter().any(|source| source.stream == stream));
            (same_stream, rank(entry), std::cmp::Reverse(*length))
        })
        .and_then(|(entry, _)| describe(day, store, entry))
}

fn rank(entry: &Entry) -> u8 {
    match entry {
        Entry::Visit { .. } | Entry::Stay { .. } => 2,
        Entry::Readings {
            precision: Precision::Precise,
            ..
        } => 1,
        _ => 0,
    }
}

/// Seconds of `day` an entry covers, on its device's own clock. A time on an
/// earlier day starts the day; one on a later day ends it. A visit with no
/// reported arrival or departure is taken to run from the start or to the end.
fn covered(entry: &Entry, day: &str) -> Option<(u32, u32)> {
    let (first, last) = match entry {
        Entry::Readings { first, last, .. } | Entry::Stay { first, last, .. } => {
            (Some(first), Some(last))
        }
        Entry::Visit {
            arrival, departure, ..
        } => (arrival.as_ref(), departure.as_ref()),
        Entry::Unreadable { .. } => return None,
    };
    let date = NaiveDate::parse_from_str(day, "%Y%m%d").ok()?;
    let start = match first {
        None => 0,
        Some(time) => clock_on(time, date, 0)?,
    };
    let end = match last {
        None => DAY_SECONDS,
        Some(time) => clock_on(time, date, DAY_SECONDS)?,
    };
    (start <= end).then_some((start, end))
}

/// Seconds since midnight when `time` falls on `date`; `outside` when it falls
/// on the side of `date` that `outside` stands for; otherwise none.
fn clock_on(time: &LocalTime, date: NaiveDate, outside: u32) -> Option<u32> {
    let local = time.utc.with_timezone(&time.offset);
    let on = local.date_naive();
    if on == date {
        Some(local.time().num_seconds_from_midnight())
    } else if (on < date) == (outside == 0) {
        Some(outside)
    } else {
        None
    }
}

fn describe(day: &DayLocation, store: &PlaceStore, entry: &Entry) -> Option<Beside> {
    let (device, words, place_id) = match entry {
        Entry::Readings { device, area, .. } => (*device, area.label()?, None),
        Entry::Visit {
            device,
            area,
            place,
            point,
            ..
        }
        | Entry::Stay {
            device,
            area,
            place,
            point,
            ..
        } => {
            let known = crate::accuracy::margin_m(point.accuracy_m)
                .and_then(|margin| store.place_for(point.latitude, point.longitude, margin));
            let words = known
                .and_then(crate::places::Place::owner_name)
                .map(|name| format!("at {name}"))
                .or_else(|| {
                    let place = place.as_ref()?;
                    place
                        .at
                        .as_ref()
                        .map(|at| format!("at {}", at.name))
                        .or_else(|| place.near.first().map(|near| format!("near {}", near.name)))
                })
                .or_else(|| area.label())?;
            (*device, words, known.map(|place| place.id.clone()))
        }
        Entry::Unreadable { .. } => return None,
    };
    Some(Beside {
        device: day.device_phrase(device),
        words,
        place_id,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::day::read_day;
    use crate::day::tests::{directory, fix, iphone_header, tier, write};
    use crate::places::Place;

    const DAY: &str = "20261001";

    /// Readings near Golden 14:00–14:04 and a visit near Denver 15:02–16:40,
    /// both from the phone; readings near Golden 15:30–15:34 from a second device.
    fn seed(root: &Path) {
        let header = iphone_header("full", "full");
        write(
            root,
            DAY,
            "phone_mobile-segment",
            "140000_300",
            &format!(
                "{header}\n{}\n{}\n",
                fix("2026-10-01T20:00:10Z", 39.7555, -105.2211, 10.0),
                fix("2026-10-01T20:04:10Z", 39.7556, -105.2212, 10.0)
            ),
        );
        write(
            root,
            DAY,
            "phone_mobile-segment",
            "150000_300",
            &format!(
                "{header}\n{}\n",
                r#"{"arrival":"2026-10-01T21:02:00Z","departure":"2026-10-01T22:40:00Z","h_acc":40,"lat":39.7527,"lon":-105.0001,"schema":"solstone.location.visit/1"}"#
            ),
        );
        write(
            root,
            DAY,
            "watch_audio",
            "153000_300",
            &format!(
                "{header}\n{}\n{}\n",
                fix("2026-10-01T21:30:10Z", 39.7555, -105.2211, 10.0),
                fix("2026-10-01T21:34:10Z", 39.7556, -105.2212, 10.0)
            ),
        );
    }

    fn day(root: &Path) -> DayLocation {
        read_day(
            root,
            DAY,
            tier(),
            None,
            &directory(),
            chrono_tz::America::Denver,
        )
    }

    fn at(h: u32, m: u32) -> u32 {
        h * 3600 + m * 60
    }

    #[test]
    fn an_activity_gets_the_entry_that_covers_its_middle_in_area_words() {
        let root = tempfile::tempdir().unwrap();
        seed(root.path());
        let day = day(root.path());
        let store = PlaceStore::default();
        let golden = location_beside(&day, &store, (at(14, 0), at(14, 4)), None).unwrap();
        assert!(golden.words.starts_with("near Golden"), "{golden:?}");
        assert!(golden.device.contains("iphone"), "{golden:?}");
        assert_eq!(golden.place_id, None);
        let visit = location_beside(&day, &store, (at(15, 10), at(16, 0)), None).unwrap();
        assert!(visit.words.starts_with("near Denver"), "{visit:?}");
        assert!(location_beside(&day, &store, (at(9, 0), at(9, 30)), None).is_none());
        for beside in [&golden, &visit] {
            for coordinate in ["39.75", "105."] {
                assert!(!beside.words.contains(coordinate), "{beside:?}");
            }
        }
    }

    #[test]
    fn the_recording_device_wins_and_otherwise_a_visit_beats_loose_readings() {
        let root = tempfile::tempdir().unwrap();
        seed(root.path());
        let day = day(root.path());
        let store = PlaceStore::default();
        let span = (at(15, 30), at(15, 34));
        let watch = location_beside(&day, &store, span, Some("watch_audio")).unwrap();
        assert!(watch.words.starts_with("near Golden"), "{watch:?}");
        let other = location_beside(&day, &store, span, Some("laptop")).unwrap();
        assert!(other.words.starts_with("near Denver"), "{other:?}");
    }

    #[test]
    fn a_named_place_is_said_by_its_name_and_carries_its_id() {
        let root = tempfile::tempdir().unwrap();
        seed(root.path());
        let day = day(root.path());
        let mut store = PlaceStore {
            version: 1,
            places: vec![Place {
                id: "office".to_owned(),
                latitude: 39.7527,
                longitude: -105.0001,
                name: None,
                alias: None,
                hidden: false,
            }],
        };
        let span = (at(15, 10), at(16, 0));
        let unnamed = location_beside(&day, &store, span, None).unwrap();
        assert!(unnamed.words.starts_with("near Denver"), "{unnamed:?}");
        assert_eq!(unnamed.place_id.as_deref(), Some("office"));
        store.places[0].name = Some("the office".to_owned());
        let named = location_beside(&day, &store, span, None).unwrap();
        assert_eq!(named.words, "at the office");
        assert_eq!(named.place_id.as_deref(), Some("office"));
    }

    #[test]
    fn a_day_without_location_has_nothing_beside_its_activities() {
        let root = tempfile::tempdir().unwrap();
        let beside = DayBeside::read(root.path(), DAY);
        assert_eq!(beside.beside((at(15, 0), at(16, 0)), None), None);
    }
}
