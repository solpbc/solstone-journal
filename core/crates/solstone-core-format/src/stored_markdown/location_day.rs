// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Presentation of the visible lines in a day's location document. Text slots
//! become ranges in the source Markdown when the writer serializes the receipt.

use std::{collections::HashMap, path::Path};

use jiff::civil::Date;
use serde::{Deserialize, Serialize};
use solstone_core_window_catalog::{Value, render, strings, text};

use crate::calendar::{MONTHS, WEEKDAYS};

fn month_name(month: i8, locale: &str) -> String {
    month
        .checked_sub(1)
        .and_then(|month| MONTHS.get(month as usize))
        .map(|id| text(locale, id))
        .unwrap_or_default()
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceWord<T> {
    Watch,
    AndroidTablet,
    AndroidPhone,
    Tablet,
    Phone,
    Literal(T),
}

impl DeviceWord<String> {
    pub fn from_source(word: &str) -> Self {
        match word {
            "watch" => Self::Watch,
            "android tablet" => Self::AndroidTablet,
            "android phone" => Self::AndroidPhone,
            "tablet" => Self::Tablet,
            "phone" => Self::Phone,
            _ => Self::Literal(word.to_owned()),
        }
    }

    pub fn render(&self, locale: &str) -> String {
        let id = match self {
            Self::Watch => "journal.window.sources.source.or.date.unit.label.watch",
            Self::AndroidTablet => "journal.window.map.device.android.tablet",
            Self::AndroidPhone => "journal.window.map.device.android.phone",
            Self::Tablet => "journal.window.map.device.tablet",
            Self::Phone => "journal.window.map.device.phone",
            Self::Literal(word) => return word.clone(),
        };
        text(locale, id)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Device<T> {
    pub word: DeviceWord<T>,
    pub name: Option<T>,
}

impl Device<String> {
    pub fn render(&self, locale: &str) -> String {
        let word = self.word.render(locale);
        match &self.name {
            Some(name) => strings(
                locale,
                "journal.window.map.device.possessive.named",
                [("device", &word), ("name", name)],
            ),
            None => strings(
                locale,
                "journal.window.map.device.possessive",
                [("device", &word)],
            ),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Area<T> {
    Inside(T),
    Near(T),
    NoTown(T),
}

impl Area<String> {
    fn render(&self, locale: &str) -> String {
        match self {
            Self::Inside(place) => strings(
                locale,
                "journal.window.map.area.label.inside",
                [("place", place)],
            ),
            Self::Near(name) => strings(
                locale,
                "journal.window.map.area.label.near.place",
                [("name", name)],
            ),
            Self::NoTown(area) => strings(
                locale,
                "journal.window.location.day.area.unnamed",
                [("area", area)],
            ),
        }
    }
}

/// The pack's visible landmark and nearby names in source order.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Landmarks<T> {
    pub at: Option<(T, Option<T>)>,
    pub near: Vec<T>,
}

fn with_context(locale: &str, specific: &str, broader: &str) -> String {
    strings(
        locale,
        "journal.window.location.day.place.context",
        [("specific", specific), ("broader", broader)],
    )
}

impl Landmarks<String> {
    fn render(&self, locale: &str) -> String {
        let at = self.at.as_ref().map(|(place, inside)| match inside {
            Some(inside) => strings(
                locale,
                "journal.window.location.day.place.inside",
                [("place", place), ("inside", inside)],
            ),
            None => strings(
                locale,
                "journal.window.map.area.label.at.place",
                [("name", place)],
            ),
        });
        let near = match self.near.as_slice() {
            [] => None,
            [place] => Some(strings(
                locale,
                "journal.window.map.area.label.near.place",
                [("name", place)],
            )),
            [leading @ .., last] => Some(strings(
                locale,
                "journal.window.location.day.nearby.list",
                [("leading", &leading.join(", ")), ("last", last)],
            )),
        };
        match (at, near) {
            (Some(at), Some(near)) => with_context(locale, &at, &near),
            (Some(words), None) | (None, Some(words)) => words,
            (None, None) => String::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OwnerName<T> {
    Literal(T),
    Home,
    Work,
}

impl OwnerName<String> {
    fn render(&self, locale: &str) -> String {
        match self {
            Self::Literal(name) => name.clone(),
            Self::Home => text(locale, "journal.window.map.option.label.home"),
            Self::Work => text(locale, "journal.window.map.option.label.work"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Location<T> {
    pub area: Area<T>,
    pub landmarks: Option<Landmarks<T>>,
    pub owner_name: Option<OwnerName<T>>,
    /// A digest of the visible place id, only when a literal and a Map alias
    /// can produce identical source bytes. It carries no name or coordinates.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub place_key: Option<String>,
}

impl Location<String> {
    fn render(&self, locale: &str) -> String {
        let mut location = self.area.render(locale);
        if let Some(landmarks) = &self.landmarks {
            location = with_context(locale, &landmarks.render(locale), &location);
        }
        if let Some(owner_name) = &self.owner_name {
            location = strings(
                locale,
                "journal.window.location.day.place.named",
                [
                    ("owner_name", &owner_name.render(locale)),
                    ("location", &location),
                ],
            );
        }
        location
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Moment<T> {
    pub month_day: Option<(i8, i8)>,
    pub clock: T,
}

impl Moment<String> {
    fn render(&self, locale: &str) -> String {
        let Some((month, day)) = self.month_day else {
            return self.clock.clone();
        };
        let date = strings(
            locale,
            "journal.window.map.date.short",
            [
                ("month", &month_name(month, locale)),
                ("day", &day.to_string()),
            ],
        );
        strings(
            locale,
            "journal.window.location.day.moment",
            [("date", &date), ("clock", &self.clock)],
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Copy<T = String> {
    Heading {
        date: Date,
    },
    HeadingLiteral {
        day: T,
    },
    Empty,
    Readings {
        area: Option<Area<T>>,
        first: Moment<T>,
        last: Option<Moment<T>>,
        count: usize,
        approximate: bool,
        device: Device<T>,
    },
    Visit {
        location: Location<T>,
        arrival: Option<Moment<T>>,
        departure: Option<Moment<T>>,
        device: Device<T>,
    },
    Stay {
        location: Location<T>,
        count: usize,
        first: Moment<T>,
        last: Moment<T>,
        device: Device<T>,
    },
}

impl Copy<String> {
    pub fn render(&self, locale: &str) -> String {
        match self {
            Self::Heading { date } => {
                let date = strings(
                    locale,
                    "journal.window.calendar.note.date",
                    [
                        (
                            "weekday",
                            &text(
                                locale,
                                WEEKDAYS[date.weekday().to_monday_zero_offset() as usize],
                            ),
                        ),
                        ("month", &month_name(date.month(), locale)),
                        ("day", &date.day().to_string()),
                        ("year", &format!("{:04}", date.year())),
                    ],
                );
                strings(
                    locale,
                    "journal.window.location.day.heading",
                    [("date", &date)],
                )
            }
            Self::HeadingLiteral { day } => strings(
                locale,
                "journal.window.location.day.heading",
                [("date", day)],
            ),
            Self::Empty => text(locale, "journal.window.location.day.empty"),
            Self::Readings {
                area,
                first,
                last,
                count,
                approximate,
                device,
            } => {
                let first = first.render(locale);
                let when = match last {
                    Some(last) => strings(
                        locale,
                        "journal.window.map.date.time.range",
                        [("first", &first), ("last", &last.render(locale))],
                    ),
                    None => strings(
                        locale,
                        "journal.window.location.day.instant",
                        [("time", &first)],
                    ),
                };
                let area = area.as_ref().map(|area| area.render(locale));
                let id = match (area.is_some(), approximate) {
                    (true, false) => "journal.window.location.day.readings.area",
                    (true, true) => "journal.window.location.day.readings.area.approximate",
                    (false, false) => "journal.window.location.day.readings.unlocated",
                    (false, true) => "journal.window.location.day.readings.unlocated.approximate",
                };
                let device = device.render(locale);
                let digits = count.to_string();
                let quantity = i32::try_from(*count).unwrap_or(i32::MAX);
                let mut args = vec![
                    ("quantity", Value::Cardinal(i64::from(quantity))),
                    ("count", Value::String(&digits)),
                    ("when", Value::String(&when)),
                    ("device", Value::String(&device)),
                ];
                if let Some(area) = area.as_ref() {
                    args.push(("area", Value::String(area)));
                }
                render(locale, id, &args)
            }
            Self::Visit {
                arrival,
                departure,
                device,
                location,
            } => {
                let arrival = match arrival {
                    Some(time) => strings(
                        locale,
                        "journal.window.map.entry.time.arrival",
                        [("time", &time.render(locale))],
                    ),
                    None => text(locale, "journal.window.map.entry.time.missing.arrival"),
                };
                let departure = match departure {
                    Some(time) => strings(
                        locale,
                        "journal.window.map.entry.time.departure",
                        [("time", &time.render(locale))],
                    ),
                    None => text(locale, "journal.window.map.entry.time.missing.departure"),
                };
                strings(
                    locale,
                    "journal.window.location.day.visit",
                    [
                        ("device", &device.render(locale)),
                        ("location", &location.render(locale)),
                        ("arrival", &arrival),
                        ("departure", &departure),
                    ],
                )
            }
            Self::Stay {
                location,
                count,
                first,
                last,
                device,
            } => strings(
                locale,
                "journal.window.location.day.stay",
                [
                    ("count", &count.to_string()),
                    ("device", &device.render(locale)),
                    ("location", &location.render(locale)),
                    ("first", &first.render(locale)),
                    ("last", &last.render(locale)),
                ],
            ),
        }
    }
}

fn optional<T, U>(value: &Option<T>, f: impl FnOnce(&T) -> Option<U>) -> Option<Option<U>> {
    match value {
        Some(value) => Some(Some(f(value)?)),
        None => Some(None),
    }
}

impl<T> Area<T> {
    fn map_text<U>(&self, f: &mut impl FnMut(&T) -> Option<U>) -> Option<Area<U>> {
        Some(match self {
            Self::Inside(v) => Area::Inside(f(v)?),
            Self::Near(v) => Area::Near(f(v)?),
            Self::NoTown(v) => Area::NoTown(f(v)?),
        })
    }
}

impl<T> Device<T> {
    fn map_text<U>(&self, f: &mut impl FnMut(&T) -> Option<U>) -> Option<Device<U>> {
        let word = match &self.word {
            DeviceWord::Watch => DeviceWord::Watch,
            DeviceWord::AndroidTablet => DeviceWord::AndroidTablet,
            DeviceWord::AndroidPhone => DeviceWord::AndroidPhone,
            DeviceWord::Tablet => DeviceWord::Tablet,
            DeviceWord::Phone => DeviceWord::Phone,
            DeviceWord::Literal(v) => DeviceWord::Literal(f(v)?),
        };
        Some(Device {
            word,
            name: optional(&self.name, f)?,
        })
    }
}

impl<T> Location<T> {
    fn map_text<U>(&self, f: &mut impl FnMut(&T) -> Option<U>) -> Option<Location<U>> {
        let landmarks = optional(&self.landmarks, |landmarks| {
            Some(Landmarks {
                at: optional(&landmarks.at, |(place, inside)| {
                    Some((f(place)?, optional(inside, &mut *f)?))
                })?,
                near: landmarks
                    .near
                    .iter()
                    .map(&mut *f)
                    .collect::<Option<Vec<_>>>()?,
            })
        })?;
        Some(Location {
            area: self.area.map_text(f)?,
            landmarks,
            place_key: self.place_key.clone(),
            owner_name: optional(&self.owner_name, |owner| {
                Some(match owner {
                    OwnerName::Home => OwnerName::Home,
                    OwnerName::Work => OwnerName::Work,
                    OwnerName::Literal(value) => OwnerName::Literal(f(value)?),
                })
            })?,
        })
    }
}

impl<T> Moment<T> {
    fn map_text<U>(&self, f: &mut impl FnMut(&T) -> Option<U>) -> Option<Moment<U>> {
        Some(Moment {
            month_day: self.month_day,
            clock: f(&self.clock)?,
        })
    }
}

impl<T> Copy<T> {
    pub(super) fn map_text<U>(&self, mut f: impl FnMut(&T) -> Option<U>) -> Option<Copy<U>> {
        Some(match self {
            Self::Heading { date } => Copy::Heading { date: *date },
            Self::HeadingLiteral { day } => Copy::HeadingLiteral { day: f(day)? },
            Self::Empty => Copy::Empty,
            Self::Readings {
                area,
                first,
                last,
                count,
                approximate,
                device,
            } => Copy::Readings {
                area: optional(area, |area| area.map_text(&mut f))?,
                first: first.map_text(&mut f)?,
                last: optional(last, |last| last.map_text(&mut f))?,
                count: *count,
                approximate: *approximate,
                device: device.map_text(&mut f)?,
            },
            Self::Visit {
                location,
                arrival,
                departure,
                device,
            } => Copy::Visit {
                location: location.map_text(&mut f)?,
                arrival: optional(arrival, |time| time.map_text(&mut f))?,
                departure: optional(departure, |time| time.map_text(&mut f))?,
                device: device.map_text(&mut f)?,
            },
            Self::Stay {
                location,
                count,
                first,
                last,
                device,
            } => Copy::Stay {
                location: location.map_text(&mut f)?,
                count: *count,
                first: first.map_text(&mut f)?,
                last: last.map_text(&mut f)?,
                device: device.map_text(&mut f)?,
            },
        })
    }
}

/// Check only stamped, visible names. Older documents have no name-source
/// dependency and keep their original English when the catalog changes.
pub fn name_origins_current(
    root: &Path,
    path: &Path,
    literal_names: &HashMap<String, bool>,
) -> bool {
    if literal_names.is_empty() {
        return true;
    }
    let Some(retained) = super::directory(root, path) else {
        return true;
    };
    let Some((_, receipt)) = super::read_document(root, retained, path, 32 * 1024 * 1024, None)
    else {
        return true;
    };
    receipt.spans.iter().all(|span| {
        let super::Message::LocationDay { copy } = &span.copy else {
            return true;
        };
        let (Copy::Stay { location, .. } | Copy::Visit { location, .. }) = copy.as_ref() else {
            return true;
        };
        let Some(key) = &location.place_key else {
            return true;
        };
        literal_names.get(key).copied()
            == Some(matches!(location.owner_name, Some(OwnerName::Literal(_))))
    })
}
