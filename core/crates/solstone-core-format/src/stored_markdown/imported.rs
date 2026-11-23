// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Words the journal adds to imported Markdown transcripts (PDF documents,
//! imported images and screenshots, and ICS calendar events). Text slots
//! refer to byte ranges inside the source Markdown when stored in a receipt.

use serde::{Deserialize, Serialize};
use solstone_core_window_catalog::{Value, packaged_locale, render, strings, text};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RasterNote {
    Preserved { page: usize },
    Stripped,
    NoneProduced,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecurrenceFrequency {
    Daily,
    Weekly,
    Monthly,
    Yearly,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum WeekdayToken<T = String> {
    Mon,
    Tue,
    Wed,
    Thu,
    Fri,
    Sat,
    Sun,
    Other(T),
}

impl<T> WeekdayToken<T> {
    fn map_text<U>(&self, mut f: impl FnMut(&T) -> Option<U>) -> Option<WeekdayToken<U>> {
        Some(match self {
            Self::Mon => WeekdayToken::Mon,
            Self::Tue => WeekdayToken::Tue,
            Self::Wed => WeekdayToken::Wed,
            Self::Thu => WeekdayToken::Thu,
            Self::Fri => WeekdayToken::Fri,
            Self::Sat => WeekdayToken::Sat,
            Self::Sun => WeekdayToken::Sun,
            Self::Other(v) => WeekdayToken::Other(f(v)?),
        })
    }
}

impl WeekdayToken {
    fn render(&self, locale: &str) -> String {
        match self {
            Self::Mon => text(locale, "journal.window.stored.event.rrule.weekday.mon"),
            Self::Tue => text(locale, "journal.window.stored.event.rrule.weekday.tue"),
            Self::Wed => text(locale, "journal.window.stored.event.rrule.weekday.wed"),
            Self::Thu => text(locale, "journal.window.stored.event.rrule.weekday.thu"),
            Self::Fri => text(locale, "journal.window.stored.event.rrule.weekday.fri"),
            Self::Sat => text(locale, "journal.window.stored.event.rrule.weekday.sat"),
            Self::Sun => text(locale, "journal.window.stored.event.rrule.weekday.sun"),
            Self::Other(other) => other.clone(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum MonthToken<T = String> {
    Jan,
    Feb,
    Mar,
    Apr,
    May,
    Jun,
    Jul,
    Aug,
    Sep,
    Oct,
    Nov,
    Dec,
    Other(T),
}

impl<T> MonthToken<T> {
    fn map_text<U>(&self, mut f: impl FnMut(&T) -> Option<U>) -> Option<MonthToken<U>> {
        Some(match self {
            Self::Jan => MonthToken::Jan,
            Self::Feb => MonthToken::Feb,
            Self::Mar => MonthToken::Mar,
            Self::Apr => MonthToken::Apr,
            Self::May => MonthToken::May,
            Self::Jun => MonthToken::Jun,
            Self::Jul => MonthToken::Jul,
            Self::Aug => MonthToken::Aug,
            Self::Sep => MonthToken::Sep,
            Self::Oct => MonthToken::Oct,
            Self::Nov => MonthToken::Nov,
            Self::Dec => MonthToken::Dec,
            Self::Other(v) => MonthToken::Other(f(v)?),
        })
    }
}

impl MonthToken {
    fn render(&self, locale: &str) -> String {
        match self {
            Self::Jan => text(locale, "journal.window.stored.event.rrule.month.jan"),
            Self::Feb => text(locale, "journal.window.stored.event.rrule.month.feb"),
            Self::Mar => text(locale, "journal.window.stored.event.rrule.month.mar"),
            Self::Apr => text(locale, "journal.window.stored.event.rrule.month.apr"),
            Self::May => text(locale, "journal.window.stored.event.rrule.month.may"),
            Self::Jun => text(locale, "journal.window.stored.event.rrule.month.jun"),
            Self::Jul => text(locale, "journal.window.stored.event.rrule.month.jul"),
            Self::Aug => text(locale, "journal.window.stored.event.rrule.month.aug"),
            Self::Sep => text(locale, "journal.window.stored.event.rrule.month.sep"),
            Self::Oct => text(locale, "journal.window.stored.event.rrule.month.oct"),
            Self::Nov => text(locale, "journal.window.stored.event.rrule.month.nov"),
            Self::Dec => text(locale, "journal.window.stored.event.rrule.month.dec"),
            Self::Other(other) => other.clone(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct EventRecurrence<T = String> {
    pub frequency: RecurrenceFrequency,
    pub interval: u64,
    #[serde(default = "Vec::new", skip_serializing_if = "Vec::is_empty")]
    pub by_day: Vec<WeekdayToken<T>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub by_month_day: Option<T>,
    #[serde(default = "Vec::new", skip_serializing_if = "Vec::is_empty")]
    pub by_month: Vec<MonthToken<T>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub count: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub until: Option<T>,
}

fn map_opt<T, U>(value: &Option<T>, f: &mut impl FnMut(&T) -> Option<U>) -> Option<Option<U>> {
    match value {
        Some(v) => Some(Some(f(v)?)),
        None => Some(None),
    }
}

impl<T> EventRecurrence<T> {
    pub(crate) fn map_text<U>(
        &self,
        mut f: impl FnMut(&T) -> Option<U>,
    ) -> Option<EventRecurrence<U>> {
        let by_day = self
            .by_day
            .iter()
            .map(|token| token.map_text(&mut f))
            .collect::<Option<Vec<_>>>()?;
        let by_month_day = map_opt(&self.by_month_day, &mut f)?;
        let by_month = self
            .by_month
            .iter()
            .map(|token| token.map_text(&mut f))
            .collect::<Option<Vec<_>>>()?;
        let until = map_opt(&self.until, &mut f)?;
        Some(EventRecurrence {
            frequency: self.frequency,
            interval: self.interval,
            by_day,
            by_month_day,
            by_month,
            count: self.count,
            until,
        })
    }
}

impl EventRecurrence {
    #[must_use]
    pub fn render(&self, locale: &str) -> String {
        let locale = packaged_locale(Some(locale));
        let interval_str = self.interval.to_string();
        let mut description = if self.interval == 1 {
            text(
                locale,
                match self.frequency {
                    RecurrenceFrequency::Daily => "journal.window.stored.event.rrule.daily",
                    RecurrenceFrequency::Weekly => "journal.window.stored.event.rrule.weekly",
                    RecurrenceFrequency::Monthly => "journal.window.stored.event.rrule.monthly",
                    RecurrenceFrequency::Yearly => "journal.window.stored.event.rrule.yearly",
                },
            )
        } else {
            strings(
                locale,
                match self.frequency {
                    RecurrenceFrequency::Daily => "journal.window.stored.event.rrule.every.days",
                    RecurrenceFrequency::Weekly => "journal.window.stored.event.rrule.every.weeks",
                    RecurrenceFrequency::Monthly => {
                        "journal.window.stored.event.rrule.every.months"
                    }
                    RecurrenceFrequency::Yearly => "journal.window.stored.event.rrule.every.years",
                },
                [("interval", &interval_str)],
            )
        };
        if !self.by_day.is_empty() {
            let days = self
                .by_day
                .iter()
                .map(|day| day.render(locale))
                .collect::<Vec<_>>()
                .join(", ");
            description = strings(
                locale,
                "journal.window.stored.event.rrule.on.days",
                [("base", &description), ("days", &days)],
            );
        }
        if let Some(days) = &self.by_month_day {
            description = strings(
                locale,
                "journal.window.stored.event.rrule.on.monthday",
                [("base", &description), ("days", days)],
            );
        }
        if !self.by_month.is_empty() {
            let months = self
                .by_month
                .iter()
                .map(|month| month.render(locale))
                .collect::<Vec<_>>()
                .join(", ");
            description = strings(
                locale,
                "journal.window.stored.event.rrule.in.months",
                [("base", &description), ("months", &months)],
            );
        }
        if let Some(count) = self.count {
            let count_str = count.to_string();
            description = render(
                locale,
                "journal.window.stored.event.rrule.count",
                &[
                    ("base", Value::String(&description)),
                    ("count", Value::String(&count_str)),
                    (
                        "quantity",
                        Value::Cardinal(i64::try_from(count).unwrap_or(i64::MAX)),
                    ),
                ],
            );
        }
        if let Some(until) = &self.until {
            description = strings(
                locale,
                "journal.window.stored.event.rrule.until",
                [("base", &description), ("until", until)],
            );
        }
        description
    }
}

/// Render the all-day or multi-day span label used in calendar import previews.
#[must_use]
pub fn render_event_preview_days(locale: &str, days: i64) -> String {
    let locale = packaged_locale(Some(locale));
    if days == 1 {
        text(locale, "journal.window.stored.event.preview.all.day")
    } else {
        let days_str = days.to_string();
        render(
            locale,
            "journal.window.stored.event.preview.days",
            &[
                ("days", Value::String(&days_str)),
                ("quantity", Value::Cardinal(days)),
            ],
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Copy<T = String> {
    // --- PDF Document transcript ---
    DocumentType,
    DocumentPages {
        count: usize,
    },
    DocumentDate {
        date: T,
        source: T,
    },
    DocumentExtraction {
        #[serde(skip_serializing_if = "Option::is_none")]
        engine: Option<T>,
        text_layer: u64,
        model_extracted: u64,
        unavailable: u64,
        pages: usize,
        image_described: u64,
        model_calls: u64,
    },
    DocumentWorkerWarnings,
    DocumentPageHeading {
        page: usize,
    },
    DocumentModelExtracted {
        page: usize,
        keep_raster: bool,
    },
    DocumentImageDescription {
        page: usize,
        keep_raster: bool,
    },
    DocumentPageTextUnavailable {
        reason: T,
        raster: RasterNote,
    },
    DocumentImageDescriptionUnavailable {
        reason: T,
        raster: RasterNote,
    },

    // --- Imported Image / Screenshot transcript ---
    ImageType,
    ImageFormat {
        format: T,
    },
    ImageDimensions {
        width: u32,
        height: u32,
    },
    ImageDate {
        date: T,
    },
    ImageCaption,
    ImageDescriptionHeader,
    ImageDescriptionUnavailable {
        reason: T,
    },

    // --- ICS Calendar event transcript ---
    EventAllDay {
        date: T,
        days: i64,
    },
    EventTimedDuration {
        when: T,
        minutes: i64,
    },
    EventRepeats {
        recurrence: EventRecurrence<T>,
    },
    EventWhere {
        location: T,
    },
    EventWith {
        names: T,
    },
}

impl<T> Copy<T> {
    pub(crate) fn map_text<U>(&self, mut f: impl FnMut(&T) -> Option<U>) -> Option<Copy<U>> {
        Some(match self {
            Self::DocumentType => Copy::DocumentType,
            Self::DocumentPages { count } => Copy::DocumentPages { count: *count },
            Self::DocumentDate { date, source } => Copy::DocumentDate {
                date: f(date)?,
                source: f(source)?,
            },
            Self::DocumentExtraction {
                engine,
                text_layer,
                model_extracted,
                unavailable,
                pages,
                image_described,
                model_calls,
            } => Copy::DocumentExtraction {
                engine: map_opt(engine, &mut f)?,
                text_layer: *text_layer,
                model_extracted: *model_extracted,
                unavailable: *unavailable,
                pages: *pages,
                image_described: *image_described,
                model_calls: *model_calls,
            },
            Self::DocumentWorkerWarnings => Copy::DocumentWorkerWarnings,
            Self::DocumentPageHeading { page } => Copy::DocumentPageHeading { page: *page },
            Self::DocumentModelExtracted { page, keep_raster } => Copy::DocumentModelExtracted {
                page: *page,
                keep_raster: *keep_raster,
            },
            Self::DocumentImageDescription { page, keep_raster } => {
                Copy::DocumentImageDescription {
                    page: *page,
                    keep_raster: *keep_raster,
                }
            }
            Self::DocumentPageTextUnavailable { reason, raster } => {
                Copy::DocumentPageTextUnavailable {
                    reason: f(reason)?,
                    raster: *raster,
                }
            }
            Self::DocumentImageDescriptionUnavailable { reason, raster } => {
                Copy::DocumentImageDescriptionUnavailable {
                    reason: f(reason)?,
                    raster: *raster,
                }
            }
            Self::ImageType => Copy::ImageType,
            Self::ImageFormat { format } => Copy::ImageFormat { format: f(format)? },
            Self::ImageDimensions { width, height } => Copy::ImageDimensions {
                width: *width,
                height: *height,
            },
            Self::ImageDate { date } => Copy::ImageDate { date: f(date)? },
            Self::ImageCaption => Copy::ImageCaption,
            Self::ImageDescriptionHeader => Copy::ImageDescriptionHeader,
            Self::ImageDescriptionUnavailable { reason } => {
                Copy::ImageDescriptionUnavailable { reason: f(reason)? }
            }
            Self::EventAllDay { date, days } => Copy::EventAllDay {
                date: f(date)?,
                days: *days,
            },
            Self::EventTimedDuration { when, minutes } => Copy::EventTimedDuration {
                when: f(when)?,
                minutes: *minutes,
            },
            Self::EventRepeats { recurrence } => Copy::EventRepeats {
                recurrence: recurrence.map_text(f)?,
            },
            Self::EventWhere { location } => Copy::EventWhere {
                location: f(location)?,
            },
            Self::EventWith { names } => Copy::EventWith { names: f(names)? },
        })
    }
}

fn page_raster_path(page: usize) -> String {
    format!("pages/page-{page:04}.png")
}

impl Copy {
    #[must_use]
    pub fn render(&self, locale: &str) -> String {
        let locale = packaged_locale(Some(locale));
        match self {
            Self::DocumentType => text(locale, "journal.window.stored.document.type"),
            Self::DocumentPages { count } => {
                let count_str = count.to_string();
                strings(
                    locale,
                    "journal.window.stored.document.pages",
                    [("count", &count_str)],
                )
            }
            Self::DocumentDate { date, source } => strings(
                locale,
                "journal.window.stored.document.date",
                [("date", date), ("source", source)],
            ),
            Self::DocumentExtraction {
                engine,
                text_layer,
                model_extracted,
                unavailable,
                pages,
                image_described,
                model_calls,
            } => {
                let engine_str = match engine.as_deref() {
                    Some(name) => name.to_owned(),
                    None => text(locale, "journal.window.stored.document.engine.unknown"),
                };
                let text_layer_str = text_layer.to_string();
                let model_extracted_str = model_extracted.to_string();
                let unavailable_str = unavailable.to_string();
                let pages_count_str = pages.to_string();
                let pages_phrase = render(
                    locale,
                    "journal.window.stored.document.pages.count",
                    &[
                        ("count", Value::String(&pages_count_str)),
                        (
                            "quantity",
                            Value::Cardinal(i64::try_from(*pages).unwrap_or(i64::MAX)),
                        ),
                    ],
                );
                let image_described_str = image_described.to_string();
                let model_calls_str = model_calls.to_string();
                let calls_phrase = render(
                    locale,
                    "journal.window.stored.document.calls.count",
                    &[
                        ("count", Value::String(&model_calls_str)),
                        (
                            "quantity",
                            Value::Cardinal(i64::try_from(*model_calls).unwrap_or(i64::MAX)),
                        ),
                    ],
                );
                strings(
                    locale,
                    "journal.window.stored.document.extraction",
                    [
                        ("engine", &engine_str),
                        ("text_layer", &text_layer_str),
                        ("model_extracted", &model_extracted_str),
                        ("unavailable", &unavailable_str),
                        ("pages", &pages_phrase),
                        ("image_described", &image_described_str),
                        ("model_calls", &calls_phrase),
                    ],
                )
            }
            Self::DocumentWorkerWarnings => {
                text(locale, "journal.window.stored.document.worker.warnings")
            }
            Self::DocumentPageHeading { page } => {
                let page_str = page.to_string();
                strings(
                    locale,
                    "journal.window.stored.document.page.heading",
                    [("page", &page_str)],
                )
            }
            Self::DocumentModelExtracted { page, keep_raster } => {
                if *keep_raster {
                    let path = page_raster_path(*page);
                    strings(
                        locale,
                        "journal.window.stored.document.marker.model.extracted.raster",
                        [("path", &path)],
                    )
                } else {
                    text(
                        locale,
                        "journal.window.stored.document.marker.model.extracted",
                    )
                }
            }
            Self::DocumentImageDescription { page, keep_raster } => {
                if *keep_raster {
                    let path = page_raster_path(*page);
                    strings(
                        locale,
                        "journal.window.stored.document.marker.image.description.raster",
                        [("path", &path)],
                    )
                } else {
                    text(
                        locale,
                        "journal.window.stored.document.marker.image.description",
                    )
                }
            }
            Self::DocumentPageTextUnavailable { reason, raster } => match raster {
                RasterNote::Preserved { page } => {
                    let path = page_raster_path(*page);
                    strings(
                        locale,
                        "journal.window.stored.document.marker.page.text.unavailable.raster",
                        [("reason", reason), ("path", &path)],
                    )
                }
                RasterNote::Stripped => strings(
                    locale,
                    "journal.window.stored.document.marker.page.text.unavailable.stripped",
                    [("reason", reason)],
                ),
                RasterNote::NoneProduced => strings(
                    locale,
                    "journal.window.stored.document.marker.page.text.unavailable.no.raster",
                    [("reason", reason)],
                ),
            },
            Self::DocumentImageDescriptionUnavailable { reason, raster } => match raster {
                RasterNote::Preserved { page } => {
                    let path = page_raster_path(*page);
                    strings(
                        locale,
                        "journal.window.stored.document.marker.image.description.unavailable.raster",
                        [("reason", reason), ("path", &path)],
                    )
                }
                RasterNote::Stripped => strings(
                    locale,
                    "journal.window.stored.document.marker.image.description.unavailable.stripped",
                    [("reason", reason)],
                ),
                RasterNote::NoneProduced => strings(
                    locale,
                    "journal.window.stored.document.marker.image.description.unavailable.no.raster",
                    [("reason", reason)],
                ),
            },
            Self::ImageType => text(locale, "journal.window.stored.image.type"),
            Self::ImageFormat { format } => strings(
                locale,
                "journal.window.stored.image.format",
                [("format", format)],
            ),
            Self::ImageDimensions { width, height } => {
                let width_str = width.to_string();
                let height_str = height.to_string();
                strings(
                    locale,
                    "journal.window.stored.image.dimensions",
                    [("width", &width_str), ("height", &height_str)],
                )
            }
            Self::ImageDate { date } => {
                strings(locale, "journal.window.stored.image.date", [("date", date)])
            }
            Self::ImageCaption => text(locale, "journal.window.stored.image.caption"),
            Self::ImageDescriptionHeader => {
                text(locale, "journal.window.stored.image.description.header")
            }
            Self::ImageDescriptionUnavailable { reason } => strings(
                locale,
                "journal.window.stored.image.description.unavailable",
                [("reason", reason)],
            ),
            Self::EventAllDay { date, days } => {
                if *days == 1 {
                    strings(
                        locale,
                        "journal.window.stored.event.all.day.single",
                        [("date", date)],
                    )
                } else {
                    let days_str = days.to_string();
                    render(
                        locale,
                        "journal.window.stored.event.all.day.multi",
                        &[
                            ("date", Value::String(date)),
                            ("days", Value::String(&days_str)),
                            ("quantity", Value::Cardinal(*days)),
                        ],
                    )
                }
            }
            Self::EventTimedDuration { when, minutes } => {
                let minutes_str = minutes.to_string();
                strings(
                    locale,
                    "journal.window.stored.event.timed.duration",
                    [("when", when), ("minutes", &minutes_str)],
                )
            }
            Self::EventRepeats { recurrence } => {
                let rule = recurrence.render(locale);
                strings(
                    locale,
                    "journal.window.stored.event.repeats",
                    [("rule", &rule)],
                )
            }
            Self::EventWhere { location } => strings(
                locale,
                "journal.window.stored.event.where",
                [("location", location)],
            ),
            Self::EventWith { names } => strings(
                locale,
                "journal.window.stored.event.with",
                [("names", names)],
            ),
        }
    }
}
