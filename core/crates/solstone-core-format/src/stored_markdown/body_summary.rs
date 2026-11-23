// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Writer-owned presentation records for `imports/<id>/summary.md` (Apple Health, Oura, Strava).

use serde::{Deserialize, Serialize};
use solstone_core_window_catalog::{Value, packaged_locale, render, strings, text};

fn render_cardinal(locale: &str, id: &str, count: u64, extra: &[(&str, &str)]) -> String {
    let count_text = count.to_string();
    let mut args = Vec::with_capacity(extra.len() + 2);
    args.push(("quantity", Value::Cardinal(count as i64)));
    args.push(("count", Value::String(&count_text)));
    for &(name, value) in extra {
        args.push((name, Value::String(value)));
    }
    render(locale, id, &args)
}

fn render_cardinal_named(locale: &str, id: &str, quantity: u64, extra: &[(&str, &str)]) -> String {
    let mut args = Vec::with_capacity(extra.len() + 1);
    args.push(("quantity", Value::Cardinal(quantity as i64)));
    for &(name, value) in extra {
        args.push((name, Value::String(value)));
    }
    render(locale, id, &args)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KnownMetric {
    Heartrate,
    DailySpo2,
    Vo2Max,
    DailyReadiness,
    DailySleep,
    DailyActivity,
    DailyStress,
    DailyResilience,
    DailyCardiovascularAge,
    SleepTime,
    RestModePeriod,
    RingConfiguration,
    Tags,
    Session,
    Workout,
}

impl KnownMetric {
    #[must_use]
    pub fn from_raw(name: &str) -> Option<Self> {
        Some(match name {
            "heartrate" => Self::Heartrate,
            "daily_spo2" => Self::DailySpo2,
            "vO2_max" => Self::Vo2Max,
            "daily_readiness" => Self::DailyReadiness,
            "daily_sleep" => Self::DailySleep,
            "daily_activity" => Self::DailyActivity,
            "daily_stress" => Self::DailyStress,
            "daily_resilience" => Self::DailyResilience,
            "daily_cardiovascular_age" => Self::DailyCardiovascularAge,
            "sleep_time" => Self::SleepTime,
            "rest_mode_period" => Self::RestModePeriod,
            "ring_configuration" => Self::RingConfiguration,
            "enhanced_tag" | "tag" => Self::Tags,
            "session" => Self::Session,
            "workout" => Self::Workout,
            _ => return None,
        })
    }

    fn key(self) -> &'static str {
        match self {
            Self::Heartrate => "journal.window.stored.body.summary.metric.heartrate",
            Self::DailySpo2 => "journal.window.stored.body.summary.metric.daily.spo2",
            Self::Vo2Max => "journal.window.stored.body.summary.metric.vo2.max",
            Self::DailyReadiness => "journal.window.stored.body.summary.metric.daily.readiness",
            Self::DailySleep => "journal.window.stored.body.summary.metric.daily.sleep",
            Self::DailyActivity => "journal.window.stored.body.summary.metric.daily.activity",
            Self::DailyStress => "journal.window.stored.body.summary.metric.daily.stress",
            Self::DailyResilience => "journal.window.stored.body.summary.metric.daily.resilience",
            Self::DailyCardiovascularAge => {
                "journal.window.stored.body.summary.metric.daily.cardiovascular.age"
            }
            Self::SleepTime => "journal.window.stored.body.summary.metric.sleep.time",
            Self::RestModePeriod => "journal.window.stored.body.summary.metric.rest.mode.period",
            Self::RingConfiguration => {
                "journal.window.stored.body.summary.metric.ring.configuration"
            }
            Self::Tags => "journal.window.stored.body.summary.metric.tags",
            Self::Session => "journal.window.stored.body.summary.metric.session",
            Self::Workout => "journal.window.stored.body.summary.metric.workout",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "metric_kind", rename_all = "snake_case")]
pub enum MetricLabel<T = String> {
    Known { metric: KnownMetric },
    Literal { text: T },
}

impl<T> MetricLabel<T> {
    fn map_text<U>(&self, f: &mut impl FnMut(&T) -> Option<U>) -> Option<MetricLabel<U>> {
        Some(match self {
            Self::Known { metric } => MetricLabel::Known { metric: *metric },
            Self::Literal { text } => MetricLabel::Literal { text: f(text)? },
        })
    }
}

impl MetricLabel {
    #[must_use]
    pub fn from_raw(name: &str, fallback_words: String) -> Self {
        match KnownMetric::from_raw(name) {
            Some(metric) => Self::Known { metric },
            None => Self::Literal {
                text: fallback_words,
            },
        }
    }

    fn render(&self, locale: &str) -> String {
        match self {
            Self::Known { metric } => text(locale, metric.key()),
            Self::Literal { text } => text.clone(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KnownStage {
    EndBeforeStart,
    RowTooLarge,
    Time,
    SpanTooLong,
    Type,
}

impl KnownStage {
    #[must_use]
    pub fn from_raw(stage: &str) -> Option<Self> {
        Some(match stage {
            "end_before_start" => Self::EndBeforeStart,
            "row_too_large" => Self::RowTooLarge,
            "time" | "event_time" => Self::Time,
            "span_too_long" => Self::SpanTooLong,
            "type" => Self::Type,
            _ => return None,
        })
    }

    fn key(self) -> &'static str {
        match self {
            Self::EndBeforeStart => "journal.window.stored.body.summary.stage.end.before.start",
            Self::RowTooLarge => "journal.window.stored.body.summary.stage.row.too.large",
            Self::Time => "journal.window.stored.body.summary.stage.time",
            Self::SpanTooLong => "journal.window.stored.body.summary.stage.span.too.long",
            Self::Type => "journal.window.stored.body.summary.stage.type",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "stage_kind", rename_all = "snake_case")]
pub enum StageLabel<T = String> {
    Known { stage: KnownStage },
    Literal { text: T },
}

impl<T> StageLabel<T> {
    fn map_text<U>(&self, f: &mut impl FnMut(&T) -> Option<U>) -> Option<StageLabel<U>> {
        Some(match self {
            Self::Known { stage } => StageLabel::Known { stage: *stage },
            Self::Literal { text } => StageLabel::Literal { text: f(text)? },
        })
    }
}

impl StageLabel {
    #[must_use]
    pub fn from_raw(stage: &str) -> Self {
        match KnownStage::from_raw(stage) {
            Some(known) => Self::Known { stage: known },
            None => Self::Literal {
                text: stage.replace('_', " "),
            },
        }
    }

    fn render(&self, locale: &str) -> String {
        match self {
            Self::Known { stage } => text(locale, stage.key()),
            Self::Literal { text } => text.clone(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "facets_kind", rename_all = "snake_case")]
pub enum FacetsSpec<T = String> {
    Another,
    EarlierOnly,
    One { facet: T },
    Many { rest: T, last: T },
    WithEarlierOne { facet: T },
    WithEarlierMany { rest: T, last: T },
}

impl<T> FacetsSpec<T> {
    fn map_text<U>(&self, f: &mut impl FnMut(&T) -> Option<U>) -> Option<FacetsSpec<U>> {
        Some(match self {
            Self::Another => FacetsSpec::Another,
            Self::EarlierOnly => FacetsSpec::EarlierOnly,
            Self::One { facet } => FacetsSpec::One { facet: f(facet)? },
            Self::Many { rest, last } => FacetsSpec::Many {
                rest: f(rest)?,
                last: f(last)?,
            },
            Self::WithEarlierOne { facet } => FacetsSpec::WithEarlierOne { facet: f(facet)? },
            Self::WithEarlierMany { rest, last } => FacetsSpec::WithEarlierMany {
                rest: f(rest)?,
                last: f(last)?,
            },
        })
    }
}

impl FacetsSpec {
    #[must_use]
    pub fn from_titles(titles: &[String], unnamed_marker: &str) -> Self {
        let has_unnamed = titles.iter().any(|title| title == unnamed_marker);
        let unique = titles
            .iter()
            .filter(|title| title.as_str() != unnamed_marker)
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .cloned()
            .collect::<Vec<_>>();
        match (has_unnamed, unique.as_slice()) {
            (false, []) => Self::Another,
            (true, []) => Self::EarlierOnly,
            (false, [one]) => Self::One { facet: one.clone() },
            (true, [one]) => Self::WithEarlierOne { facet: one.clone() },
            (false, [rest @ .., last]) => Self::Many {
                rest: rest.join(", "),
                last: last.clone(),
            },
            (true, [rest @ .., last]) => Self::WithEarlierMany {
                rest: rest.join(", "),
                last: last.clone(),
            },
        }
    }

    fn render(&self, locale: &str) -> String {
        match self {
            Self::Another => text(locale, "journal.window.stored.body.summary.facets.another"),
            Self::EarlierOnly => text(
                locale,
                "journal.window.stored.body.summary.facets.earlier.only",
            ),
            Self::One { facet } => strings(
                locale,
                "journal.window.stored.body.summary.facets.one",
                [("facet", facet.as_str())],
            ),
            Self::Many { rest, last } => strings(
                locale,
                "journal.window.stored.body.summary.facets.many",
                [("rest", rest.as_str()), ("last", last.as_str())],
            ),
            Self::WithEarlierOne { facet } => {
                let named = strings(
                    locale,
                    "journal.window.stored.body.summary.facets.one",
                    [("facet", facet.as_str())],
                );
                strings(
                    locale,
                    "journal.window.stored.body.summary.facets.with.earlier",
                    [("named", named.as_str())],
                )
            }
            Self::WithEarlierMany { rest, last } => {
                let named = strings(
                    locale,
                    "journal.window.stored.body.summary.facets.many",
                    [("rest", rest.as_str()), ("last", last.as_str())],
                );
                strings(
                    locale,
                    "journal.window.stored.body.summary.facets.with.earlier",
                    [("named", named.as_str())],
                )
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OuraHoleKind {
    Deferred,
    MovedFar,
    HeldBack,
    Unreadable,
    RecordRefused,
    Converging,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "refusal_kind", rename_all = "snake_case")]
pub enum OuraRefusalCopy<T = String> {
    Named { scopes: T },
    Documented { scope: T },
    NotNamed,
    Said { detail: T },
}

impl<T> OuraRefusalCopy<T> {
    fn map_text<U>(&self, f: &mut impl FnMut(&T) -> Option<U>) -> Option<OuraRefusalCopy<U>> {
        Some(match self {
            Self::Named { scopes } => OuraRefusalCopy::Named { scopes: f(scopes)? },
            Self::Documented { scope } => OuraRefusalCopy::Documented { scope: f(scope)? },
            Self::NotNamed => OuraRefusalCopy::NotNamed,
            Self::Said { detail } => OuraRefusalCopy::Said { detail: f(detail)? },
        })
    }
}

impl OuraRefusalCopy {
    fn render(&self, locale: &str) -> String {
        match self {
            Self::Named { scopes } => strings(
                locale,
                "journal.window.stored.body.summary.oura.refusal.named",
                [("scopes", scopes.as_str())],
            ),
            Self::Documented { scope } => strings(
                locale,
                "journal.window.stored.body.summary.oura.refusal.documented",
                [("scope", scope.as_str())],
            ),
            Self::NotNamed => text(
                locale,
                "journal.window.stored.body.summary.oura.refusal.not.named",
            ),
            Self::Said { detail } => strings(
                locale,
                "journal.window.stored.body.summary.oura.refusal.said",
                [("detail", detail.as_str())],
            ),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StravaWaitReason {
    WaitingForWindow,
    NoCondition,
    LaneUnavailable,
    SettingsUnreadable,
    Stopped,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Copy<T = String> {
    // --- Shared / Apple Health / Oura ---
    Heading {
        source: T,
    },
    CountsBase {
        written: u64,
        unchanged: u64,
        unreadable: u64,
    },
    CountsStayed {
        written: u64,
        unchanged: u64,
        stayed_deleted: u64,
        unreadable: u64,
    },
    HeldBackReading {
        count: u64,
    },
    PlacedZone {
        zone: T,
    },
    FacetChosen {
        facet: T,
    },
    FacetNone,
    SplitRows {
        count: u64,
    },
    RetainedRevisions {
        count: u64,
        facets: T,
    },
    KeptUnfiled,
    TookFacet {
        count: u64,
    },
    KeptFacet {
        count: u64,
    },
    Refiled {
        count: u64,
    },
    ListedMoved {
        count: u64,
        facets: FacetsSpec<T>,
    },
    ListedOver {
        some_moved: bool,
        shown: usize,
        count: u64,
    },
    JoinedHidden {
        count: u64,
    },
    EarlierWaiting {
        count: u64,
    },
    ActivitiesShown {
        count: u64,
    },
    ActivitiesNoFacet {
        count: u64,
    },
    ActivitiesRefreshed {
        count: u64,
    },
    ActivitiesRefused {
        facets: FacetsSpec<T>,
    },
    OverlappingNights {
        count: u64,
    },
    EntriesStayedDeleted {
        count: u64,
    },
    Deferred {
        source: T,
        count: u64,
    },
    MovedFar {
        source: T,
        count: u64,
    },
    Moved {
        source: T,
        count: u64,
    },
    SectionHeldBackHeading,
    SectionHeldBackIntro,
    SectionRead,
    SectionNotRead,
    SectionCouldntBeRead,
    SectionNotSavedYet,
    SectionFromOura,
    SectionNotWrittenInFullHeading,
    SectionNotWrittenInFullIntro,
    MetricCountBullet {
        metric: MetricLabel<T>,
        count: u64,
    },
    StageCountBullet {
        metric: Option<MetricLabel<T>>,
        stage: StageLabel<T>,
        count: u64,
    },
    UnsureBullet {
        endpoint: MetricLabel<T>,
        count: u64,
        days: T,
    },
    ReplacedByLater,
    OuraHoleLine {
        endpoint: MetricLabel<T>,
        place: T,
        reason: OuraHoleKind,
    },
    OuraReadLine {
        endpoint: MetricLabel<T>,
        heartrate: bool,
        count: u64,
        from: T,
        to: T,
    },
    OuraEmptyLine {
        endpoint: MetricLabel<T>,
        heartrate: bool,
        from: T,
        to: T,
    },
    OuraRefusedLine {
        endpoint: MetricLabel<T>,
        why: OuraRefusalCopy<T>,
    },
    OuraRefusedPartWayLine {
        endpoint: MetricLabel<T>,
        heartrate: bool,
        through: T,
        why: OuraRefusalCopy<T>,
    },
    OuraFailedLine {
        endpoint: MetricLabel<T>,
        error: T,
    },
    OuraFailedPartWayLine {
        endpoint: MetricLabel<T>,
        heartrate: bool,
        through: T,
        error: T,
    },
    // --- Strava ---
    StravaOpening {
        new_workouts: u64,
        updated_workouts: u64,
    },
    StravaReplaced {
        count: u64,
    },
    StravaSectionFacet,
    StravaFacetChosen {
        facet: T,
    },
    StravaListedMoved {
        count: u64,
        facets: FacetsSpec<T>,
    },
    StravaListedOver {
        shown: usize,
        count: u64,
    },
    StravaFacetDefault {
        facet: T,
    },
    StravaFacetChosenNone,
    StravaFacetUnset,
    StravaFiled {
        count: u64,
        facet: T,
        moved: u64,
    },
    StravaKeptFiled {
        count: u64,
        facet: T,
    },
    StravaKeptSensed {
        count: u64,
    },
    StravaKeptUnreadable {
        count: u64,
    },
    StravaSectionActivities,
    StravaRecordsWritten {
        count: u64,
    },
    StravaRecordsNoFacet {
        count: u64,
    },
    StravaRecordsRefused,
    StravaConversionWaiting,
    StravaConversionNoWorkout {
        count: u64,
    },
    StravaConversionMovesOpen {
        count: u64,
    },
    StravaSectionPeopleAndGear,
    StravaNamesStoreUnavailable,
    StravaNamesRead {
        count: u64,
    },
    StravaNamesCached {
        count: u64,
    },
    StravaNamesFailed {
        count: u64,
    },
    StravaNamesWaiting {
        count: u64,
        reason: StravaWaitReason,
    },
    StravaNamesElsewhere {
        count: u64,
    },
    StravaNamesSetAside {
        count: u64,
    },
    StravaNamesPromotionUnavailable,
    StravaNamesCreated {
        count: u64,
    },
    StravaNamesNoneCreated,
}

impl<T> Copy<T> {
    pub(crate) fn map_text<U>(&self, mut f: impl FnMut(&T) -> Option<U>) -> Option<Copy<U>> {
        Some(match self {
            Self::Heading { source } => Copy::Heading { source: f(source)? },
            Self::CountsBase {
                written,
                unchanged,
                unreadable,
            } => Copy::CountsBase {
                written: *written,
                unchanged: *unchanged,
                unreadable: *unreadable,
            },
            Self::CountsStayed {
                written,
                unchanged,
                stayed_deleted,
                unreadable,
            } => Copy::CountsStayed {
                written: *written,
                unchanged: *unchanged,
                stayed_deleted: *stayed_deleted,
                unreadable: *unreadable,
            },
            Self::HeldBackReading { count } => Copy::HeldBackReading { count: *count },
            Self::PlacedZone { zone } => Copy::PlacedZone { zone: f(zone)? },
            Self::FacetChosen { facet } => Copy::FacetChosen { facet: f(facet)? },
            Self::FacetNone => Copy::FacetNone,
            Self::SplitRows { count } => Copy::SplitRows { count: *count },
            Self::RetainedRevisions { count, facets } => Copy::RetainedRevisions {
                count: *count,
                facets: f(facets)?,
            },
            Self::KeptUnfiled => Copy::KeptUnfiled,
            Self::TookFacet { count } => Copy::TookFacet { count: *count },
            Self::KeptFacet { count } => Copy::KeptFacet { count: *count },
            Self::Refiled { count } => Copy::Refiled { count: *count },
            Self::ListedMoved { count, facets } => Copy::ListedMoved {
                count: *count,
                facets: facets.map_text(&mut f)?,
            },
            Self::ListedOver {
                some_moved,
                shown,
                count,
            } => Copy::ListedOver {
                some_moved: *some_moved,
                shown: *shown,
                count: *count,
            },
            Self::JoinedHidden { count } => Copy::JoinedHidden { count: *count },
            Self::EarlierWaiting { count } => Copy::EarlierWaiting { count: *count },
            Self::ActivitiesShown { count } => Copy::ActivitiesShown { count: *count },
            Self::ActivitiesNoFacet { count } => Copy::ActivitiesNoFacet { count: *count },
            Self::ActivitiesRefreshed { count } => Copy::ActivitiesRefreshed { count: *count },
            Self::ActivitiesRefused { facets } => Copy::ActivitiesRefused {
                facets: facets.map_text(&mut f)?,
            },
            Self::OverlappingNights { count } => Copy::OverlappingNights { count: *count },
            Self::EntriesStayedDeleted { count } => Copy::EntriesStayedDeleted { count: *count },
            Self::Deferred { source, count } => Copy::Deferred {
                source: f(source)?,
                count: *count,
            },
            Self::MovedFar { source, count } => Copy::MovedFar {
                source: f(source)?,
                count: *count,
            },
            Self::Moved { source, count } => Copy::Moved {
                source: f(source)?,
                count: *count,
            },
            Self::SectionHeldBackHeading => Copy::SectionHeldBackHeading,
            Self::SectionHeldBackIntro => Copy::SectionHeldBackIntro,
            Self::SectionRead => Copy::SectionRead,
            Self::SectionNotRead => Copy::SectionNotRead,
            Self::SectionCouldntBeRead => Copy::SectionCouldntBeRead,
            Self::SectionNotSavedYet => Copy::SectionNotSavedYet,
            Self::SectionFromOura => Copy::SectionFromOura,
            Self::SectionNotWrittenInFullHeading => Copy::SectionNotWrittenInFullHeading,
            Self::SectionNotWrittenInFullIntro => Copy::SectionNotWrittenInFullIntro,
            Self::MetricCountBullet { metric, count } => Copy::MetricCountBullet {
                metric: metric.map_text(&mut f)?,
                count: *count,
            },
            Self::StageCountBullet {
                metric,
                stage,
                count,
            } => Copy::StageCountBullet {
                metric: match metric {
                    Some(m) => Some(m.map_text(&mut f)?),
                    None => None,
                },
                stage: stage.map_text(&mut f)?,
                count: *count,
            },
            Self::UnsureBullet {
                endpoint,
                count,
                days,
            } => Copy::UnsureBullet {
                endpoint: endpoint.map_text(&mut f)?,
                count: *count,
                days: f(days)?,
            },
            Self::ReplacedByLater => Copy::ReplacedByLater,
            Self::OuraHoleLine {
                endpoint,
                place,
                reason,
            } => Copy::OuraHoleLine {
                endpoint: endpoint.map_text(&mut f)?,
                place: f(place)?,
                reason: *reason,
            },
            Self::OuraReadLine {
                endpoint,
                heartrate,
                count,
                from,
                to,
            } => Copy::OuraReadLine {
                endpoint: endpoint.map_text(&mut f)?,
                heartrate: *heartrate,
                count: *count,
                from: f(from)?,
                to: f(to)?,
            },
            Self::OuraEmptyLine {
                endpoint,
                heartrate,
                from,
                to,
            } => Copy::OuraEmptyLine {
                endpoint: endpoint.map_text(&mut f)?,
                heartrate: *heartrate,
                from: f(from)?,
                to: f(to)?,
            },
            Self::OuraRefusedLine { endpoint, why } => Copy::OuraRefusedLine {
                endpoint: endpoint.map_text(&mut f)?,
                why: why.map_text(&mut f)?,
            },
            Self::OuraRefusedPartWayLine {
                endpoint,
                heartrate,
                through,
                why,
            } => Copy::OuraRefusedPartWayLine {
                endpoint: endpoint.map_text(&mut f)?,
                heartrate: *heartrate,
                through: f(through)?,
                why: why.map_text(&mut f)?,
            },
            Self::OuraFailedLine { endpoint, error } => Copy::OuraFailedLine {
                endpoint: endpoint.map_text(&mut f)?,
                error: f(error)?,
            },
            Self::OuraFailedPartWayLine {
                endpoint,
                heartrate,
                through,
                error,
            } => Copy::OuraFailedPartWayLine {
                endpoint: endpoint.map_text(&mut f)?,
                heartrate: *heartrate,
                through: f(through)?,
                error: f(error)?,
            },
            Self::StravaOpening {
                new_workouts,
                updated_workouts,
            } => Copy::StravaOpening {
                new_workouts: *new_workouts,
                updated_workouts: *updated_workouts,
            },
            Self::StravaReplaced { count } => Copy::StravaReplaced { count: *count },
            Self::StravaSectionFacet => Copy::StravaSectionFacet,
            Self::StravaFacetChosen { facet } => Copy::StravaFacetChosen { facet: f(facet)? },
            Self::StravaListedMoved { count, facets } => Copy::StravaListedMoved {
                count: *count,
                facets: facets.map_text(&mut f)?,
            },
            Self::StravaListedOver { shown, count } => Copy::StravaListedOver {
                shown: *shown,
                count: *count,
            },
            Self::StravaFacetDefault { facet } => Copy::StravaFacetDefault { facet: f(facet)? },
            Self::StravaFacetChosenNone => Copy::StravaFacetChosenNone,
            Self::StravaFacetUnset => Copy::StravaFacetUnset,
            Self::StravaFiled {
                count,
                facet,
                moved,
            } => Copy::StravaFiled {
                count: *count,
                facet: f(facet)?,
                moved: *moved,
            },
            Self::StravaKeptFiled { count, facet } => Copy::StravaKeptFiled {
                count: *count,
                facet: f(facet)?,
            },
            Self::StravaKeptSensed { count } => Copy::StravaKeptSensed { count: *count },
            Self::StravaKeptUnreadable { count } => Copy::StravaKeptUnreadable { count: *count },
            Self::StravaSectionActivities => Copy::StravaSectionActivities,
            Self::StravaRecordsWritten { count } => Copy::StravaRecordsWritten { count: *count },
            Self::StravaRecordsNoFacet { count } => Copy::StravaRecordsNoFacet { count: *count },
            Self::StravaRecordsRefused => Copy::StravaRecordsRefused,
            Self::StravaConversionWaiting => Copy::StravaConversionWaiting,
            Self::StravaConversionNoWorkout { count } => {
                Copy::StravaConversionNoWorkout { count: *count }
            }
            Self::StravaConversionMovesOpen { count } => {
                Copy::StravaConversionMovesOpen { count: *count }
            }
            Self::StravaSectionPeopleAndGear => Copy::StravaSectionPeopleAndGear,
            Self::StravaNamesStoreUnavailable => Copy::StravaNamesStoreUnavailable,
            Self::StravaNamesRead { count } => Copy::StravaNamesRead { count: *count },
            Self::StravaNamesCached { count } => Copy::StravaNamesCached { count: *count },
            Self::StravaNamesFailed { count } => Copy::StravaNamesFailed { count: *count },
            Self::StravaNamesWaiting { count, reason } => Copy::StravaNamesWaiting {
                count: *count,
                reason: *reason,
            },
            Self::StravaNamesElsewhere { count } => Copy::StravaNamesElsewhere { count: *count },
            Self::StravaNamesSetAside { count } => Copy::StravaNamesSetAside { count: *count },
            Self::StravaNamesPromotionUnavailable => Copy::StravaNamesPromotionUnavailable,
            Self::StravaNamesCreated { count } => Copy::StravaNamesCreated { count: *count },
            Self::StravaNamesNoneCreated => Copy::StravaNamesNoneCreated,
        })
    }
}

impl Copy {
    pub fn render(&self, locale: &str) -> String {
        let locale = packaged_locale(Some(locale));
        match self {
            Self::Heading { source } => strings(
                locale,
                "journal.window.stored.body.summary.heading",
                [("source", source.as_str())],
            ),
            Self::CountsBase {
                written,
                unchanged,
                unreadable,
            } => {
                let written_text = render_cardinal(
                    locale,
                    "journal.window.stored.body.summary.windows.count",
                    *written,
                    &[],
                );
                let unchanged_text = unchanged.to_string();
                let unreadable_text = unreadable.to_string();
                strings(
                    locale,
                    "journal.window.stored.body.summary.counts.base",
                    [
                        ("written", written_text.as_str()),
                        ("unchanged", unchanged_text.as_str()),
                        ("unreadable", unreadable_text.as_str()),
                    ],
                )
            }
            Self::CountsStayed {
                written,
                unchanged,
                stayed_deleted,
                unreadable,
            } => {
                let written_text = render_cardinal(
                    locale,
                    "journal.window.stored.body.summary.windows.count",
                    *written,
                    &[],
                );
                let unchanged_text = unchanged.to_string();
                let stayed_text = stayed_deleted.to_string();
                let unreadable_text = unreadable.to_string();
                strings(
                    locale,
                    "journal.window.stored.body.summary.counts.stayed",
                    [
                        ("written", written_text.as_str()),
                        ("unchanged", unchanged_text.as_str()),
                        ("stayed_deleted", stayed_text.as_str()),
                        ("unreadable", unreadable_text.as_str()),
                    ],
                )
            }
            Self::HeldBackReading { count } => render_cardinal(
                locale,
                "journal.window.stored.body.summary.held.back.reading",
                *count,
                &[],
            ),
            Self::PlacedZone { zone } => strings(
                locale,
                "journal.window.stored.body.summary.placed.zone",
                [("zone", zone.as_str())],
            ),
            Self::FacetChosen { facet } => strings(
                locale,
                "journal.window.stored.body.summary.facet.chosen",
                [("facet", facet.as_str())],
            ),
            Self::FacetNone => text(locale, "journal.window.stored.body.summary.facet.none"),
            Self::SplitRows { count } => render_cardinal(
                locale,
                "journal.window.stored.body.summary.split.rows",
                *count,
                &[],
            ),
            Self::RetainedRevisions { count, facets } => render_cardinal(
                locale,
                "journal.window.stored.body.summary.retained.revisions",
                *count,
                &[("facets", facets.as_str())],
            ),
            Self::KeptUnfiled => text(locale, "journal.window.stored.body.summary.kept.unfiled"),
            Self::TookFacet { count } => render_cardinal(
                locale,
                "journal.window.stored.body.summary.took.facet",
                *count,
                &[],
            ),
            Self::KeptFacet { count } => render_cardinal(
                locale,
                "journal.window.stored.body.summary.kept.facet",
                *count,
                &[],
            ),
            Self::Refiled { count } => render_cardinal(
                locale,
                "journal.window.stored.body.summary.refiled",
                *count,
                &[],
            ),
            Self::ListedMoved { count, facets } => {
                let facets_text = facets.render(locale);
                render_cardinal(
                    locale,
                    "journal.window.stored.body.summary.listed.moved",
                    *count,
                    &[("facets", facets_text.as_str())],
                )
            }
            Self::ListedOver {
                some_moved,
                shown,
                count,
            } => match (*some_moved, *shown) {
                (true, 0) => render_cardinal(
                    locale,
                    "journal.window.stored.body.summary.listed.over.some.none.shown",
                    *count,
                    &[],
                ),
                (true, shown) => {
                    let shown_text = shown.to_string();
                    render_cardinal(
                        locale,
                        "journal.window.stored.body.summary.listed.over.some.shown",
                        *count,
                        &[("shown", shown_text.as_str())],
                    )
                }
                (false, 0) => render_cardinal(
                    locale,
                    "journal.window.stored.body.summary.listed.over.none.none.shown",
                    *count,
                    &[],
                ),
                (false, shown) => {
                    let shown_text = shown.to_string();
                    render_cardinal(
                        locale,
                        "journal.window.stored.body.summary.listed.over.none.shown",
                        *count,
                        &[("shown", shown_text.as_str())],
                    )
                }
            },
            Self::JoinedHidden { count } => render_cardinal(
                locale,
                "journal.window.stored.body.summary.joined.hidden",
                *count,
                &[],
            ),
            Self::EarlierWaiting { count } => render_cardinal(
                locale,
                "journal.window.stored.body.summary.earlier.waiting",
                *count,
                &[],
            ),
            Self::ActivitiesShown { count } => render_cardinal(
                locale,
                "journal.window.stored.body.summary.activities.shown",
                *count,
                &[],
            ),
            Self::ActivitiesNoFacet { count } => render_cardinal(
                locale,
                "journal.window.stored.body.summary.activities.no.facet",
                *count,
                &[],
            ),
            Self::ActivitiesRefreshed { count } => render_cardinal(
                locale,
                "journal.window.stored.body.summary.activities.refreshed",
                *count,
                &[],
            ),
            Self::ActivitiesRefused { facets } => {
                let facets_text = facets.render(locale);
                strings(
                    locale,
                    "journal.window.stored.body.summary.activities.refused",
                    [("facets", facets_text.as_str())],
                )
            }
            Self::OverlappingNights { count } => {
                let count_text = count.to_string();
                strings(
                    locale,
                    "journal.window.stored.body.summary.overlapping.nights",
                    [("count", count_text.as_str())],
                )
            }
            Self::EntriesStayedDeleted { count } => render_cardinal(
                locale,
                "journal.window.stored.body.summary.entries.stayed.deleted",
                *count,
                &[],
            ),
            Self::Deferred { source, count } => render_cardinal(
                locale,
                "journal.window.stored.body.summary.deferred",
                *count,
                &[("source", source.as_str())],
            ),
            Self::MovedFar { source, count } => render_cardinal(
                locale,
                "journal.window.stored.body.summary.moved.far",
                *count,
                &[("source", source.as_str())],
            ),
            Self::Moved { source, count } => render_cardinal(
                locale,
                "journal.window.stored.body.summary.moved",
                *count,
                &[("source", source.as_str())],
            ),
            Self::SectionHeldBackHeading => text(
                locale,
                "journal.window.stored.body.summary.section.held.back",
            ),
            Self::SectionHeldBackIntro => text(
                locale,
                "journal.window.stored.body.summary.section.held.back.intro",
            ),
            Self::SectionRead => text(locale, "journal.window.stored.body.summary.section.read"),
            Self::SectionNotRead => text(
                locale,
                "journal.window.stored.body.summary.section.not.read",
            ),
            Self::SectionCouldntBeRead => text(
                locale,
                "journal.window.stored.body.summary.section.couldnt.be.read",
            ),
            Self::SectionNotSavedYet => text(
                locale,
                "journal.window.stored.body.summary.section.not.saved.yet",
            ),
            Self::SectionFromOura => text(
                locale,
                "journal.window.stored.body.summary.section.from.oura",
            ),
            Self::SectionNotWrittenInFullHeading => text(
                locale,
                "journal.window.stored.body.summary.section.not.written.in.full",
            ),
            Self::SectionNotWrittenInFullIntro => text(
                locale,
                "journal.window.stored.body.summary.section.not.written.in.full.intro",
            ),
            Self::MetricCountBullet { metric, count } => {
                format!("- {}: {count}", metric.render(locale))
            }
            Self::StageCountBullet {
                metric,
                stage,
                count,
            } => match metric {
                Some(metric) => format!(
                    "- {}: {}: {count}",
                    metric.render(locale),
                    stage.render(locale)
                ),
                None => format!("- {}: {count}", stage.render(locale)),
            },
            Self::UnsureBullet {
                endpoint,
                count,
                days,
            } => {
                let endpoint_text = endpoint.render(locale);
                render_cardinal(
                    locale,
                    "journal.window.stored.body.summary.unsure.bullet",
                    *count,
                    &[
                        ("endpoint", endpoint_text.as_str()),
                        ("days", days.as_str()),
                    ],
                )
            }
            Self::ReplacedByLater => text(locale, "journal.window.stored.body.summary.replaced"),
            Self::OuraHoleLine {
                endpoint,
                place,
                reason,
            } => {
                let endpoint_text = endpoint.render(locale);
                let id = match reason {
                    OuraHoleKind::Deferred => "journal.window.stored.body.summary.hole.deferred",
                    OuraHoleKind::MovedFar => "journal.window.stored.body.summary.hole.moved.far",
                    OuraHoleKind::HeldBack => "journal.window.stored.body.summary.hole.held.back",
                    OuraHoleKind::Unreadable => {
                        "journal.window.stored.body.summary.hole.unreadable"
                    }
                    OuraHoleKind::RecordRefused => {
                        "journal.window.stored.body.summary.hole.record.refused"
                    }
                    OuraHoleKind::Converging => {
                        "journal.window.stored.body.summary.hole.converging"
                    }
                };
                strings(
                    locale,
                    id,
                    [
                        ("endpoint", endpoint_text.as_str()),
                        ("place", place.as_str()),
                    ],
                )
            }
            Self::OuraReadLine {
                endpoint,
                heartrate,
                count,
                from,
                to,
            } => {
                let endpoint_text = endpoint.render(locale);
                let days = strings(
                    locale,
                    if *heartrate {
                        "journal.window.stored.body.summary.oura.days.heartrate"
                    } else {
                        "journal.window.stored.body.summary.oura.days.other"
                    },
                    [("from", from.as_str()), ("to", to.as_str())],
                );
                render_cardinal(
                    locale,
                    if *heartrate {
                        "journal.window.stored.body.summary.oura.read.heartrate"
                    } else {
                        "journal.window.stored.body.summary.oura.read.entries"
                    },
                    *count,
                    &[
                        ("endpoint", endpoint_text.as_str()),
                        ("days", days.as_str()),
                    ],
                )
            }
            Self::OuraEmptyLine {
                endpoint,
                heartrate,
                from,
                to,
            } => {
                let endpoint_text = endpoint.render(locale);
                let days = strings(
                    locale,
                    if *heartrate {
                        "journal.window.stored.body.summary.oura.days.heartrate"
                    } else {
                        "journal.window.stored.body.summary.oura.days.other"
                    },
                    [("from", from.as_str()), ("to", to.as_str())],
                );
                strings(
                    locale,
                    "journal.window.stored.body.summary.oura.empty",
                    [
                        ("endpoint", endpoint_text.as_str()),
                        ("days", days.as_str()),
                    ],
                )
            }
            Self::OuraRefusedLine { endpoint, why } => {
                let endpoint_text = endpoint.render(locale);
                let why_text = why.render(locale);
                strings(
                    locale,
                    "journal.window.stored.body.summary.oura.refused",
                    [
                        ("endpoint", endpoint_text.as_str()),
                        ("why", why_text.as_str()),
                    ],
                )
            }
            Self::OuraRefusedPartWayLine {
                endpoint,
                heartrate,
                through,
                why,
            } => {
                let endpoint_text = endpoint.render(locale);
                let through_text = if *heartrate {
                    through.clone()
                } else {
                    strings(
                        locale,
                        "journal.window.stored.body.summary.oura.through.day",
                        [("day", through.as_str())],
                    )
                };
                let why_text = why.render(locale);
                strings(
                    locale,
                    "journal.window.stored.body.summary.oura.refused.part.way",
                    [
                        ("endpoint", endpoint_text.as_str()),
                        ("through", through_text.as_str()),
                        ("why", why_text.as_str()),
                    ],
                )
            }
            Self::OuraFailedLine { endpoint, error } => {
                let endpoint_text = endpoint.render(locale);
                strings(
                    locale,
                    "journal.window.stored.body.summary.oura.failed",
                    [
                        ("endpoint", endpoint_text.as_str()),
                        ("error", error.as_str()),
                    ],
                )
            }
            Self::OuraFailedPartWayLine {
                endpoint,
                heartrate,
                through,
                error,
            } => {
                let endpoint_text = endpoint.render(locale);
                let through_text = if *heartrate {
                    through.clone()
                } else {
                    strings(
                        locale,
                        "journal.window.stored.body.summary.oura.through.day",
                        [("day", through.as_str())],
                    )
                };
                strings(
                    locale,
                    "journal.window.stored.body.summary.oura.failed.part.way",
                    [
                        ("endpoint", endpoint_text.as_str()),
                        ("through", through_text.as_str()),
                        ("error", error.as_str()),
                    ],
                )
            }
            Self::StravaOpening {
                new_workouts,
                updated_workouts,
            } => match (*new_workouts, *updated_workouts) {
                (0, 0) => text(
                    locale,
                    "journal.window.stored.body.summary.strava.added.none",
                ),
                (added, 0) => render_cardinal(
                    locale,
                    "journal.window.stored.body.summary.strava.added.only",
                    added,
                    &[],
                ),
                (0, updated) => {
                    let updated_text = updated.to_string();
                    render_cardinal_named(
                        locale,
                        "journal.window.stored.body.summary.strava.updated.only",
                        updated,
                        &[("updated", updated_text.as_str())],
                    )
                }
                (added, updated) => {
                    let added_clause = render_cardinal(
                        locale,
                        "journal.window.stored.body.summary.strava.added.count",
                        added,
                        &[],
                    );
                    let updated_text = updated.to_string();
                    render_cardinal_named(
                        locale,
                        "journal.window.stored.body.summary.strava.added.and.updated",
                        updated,
                        &[
                            ("added", added_clause.as_str()),
                            ("updated", updated_text.as_str()),
                        ],
                    )
                }
            },
            Self::StravaReplaced { count } => render_cardinal(
                locale,
                "journal.window.stored.body.summary.strava.replaced",
                *count,
                &[],
            ),
            Self::StravaSectionFacet => text(
                locale,
                "journal.window.stored.body.summary.strava.section.facet",
            ),
            Self::StravaFacetChosen { facet } => strings(
                locale,
                "journal.window.stored.body.summary.strava.facet.chosen",
                [("facet", facet.as_str())],
            ),
            Self::StravaListedMoved { count, facets } => {
                let facets_text = facets.render(locale);
                render_cardinal(
                    locale,
                    "journal.window.stored.body.summary.strava.listed.moved",
                    *count,
                    &[("facets", facets_text.as_str())],
                )
            }
            Self::StravaListedOver { shown, count } => {
                if *shown == 0 {
                    render_cardinal(
                        locale,
                        "journal.window.stored.body.summary.strava.listed.over.none.shown",
                        *count,
                        &[],
                    )
                } else {
                    let shown_text = shown.to_string();
                    render_cardinal(
                        locale,
                        "journal.window.stored.body.summary.strava.listed.over.shown",
                        *count,
                        &[("shown", shown_text.as_str())],
                    )
                }
            }
            Self::StravaFacetDefault { facet } => strings(
                locale,
                "journal.window.stored.body.summary.strava.facet.default",
                [("facet", facet.as_str())],
            ),
            Self::StravaFacetChosenNone => text(
                locale,
                "journal.window.stored.body.summary.strava.facet.chosen.none",
            ),
            Self::StravaFacetUnset => text(
                locale,
                "journal.window.stored.body.summary.strava.facet.unset",
            ),
            Self::StravaFiled {
                count,
                facet,
                moved,
            } => match (*count, *moved) {
                (count, 0) => render_cardinal(
                    locale,
                    "journal.window.stored.body.summary.strava.filed",
                    count,
                    &[("facet", facet.as_str())],
                ),
                (1, 1) => strings(
                    locale,
                    "journal.window.stored.body.summary.strava.filed.one.moved",
                    [("facet", facet.as_str())],
                ),
                (count, moved) => {
                    let moved_text = moved.to_string();
                    render_cardinal(
                        locale,
                        "journal.window.stored.body.summary.strava.filed.some.moved",
                        count,
                        &[("facet", facet.as_str()), ("moved", moved_text.as_str())],
                    )
                }
            },
            Self::StravaKeptFiled { count, facet } => render_cardinal(
                locale,
                "journal.window.stored.body.summary.strava.kept.filed",
                *count,
                &[("facet", facet.as_str())],
            ),
            Self::StravaKeptSensed { count } => render_cardinal(
                locale,
                "journal.window.stored.body.summary.strava.kept.sensed",
                *count,
                &[],
            ),
            Self::StravaKeptUnreadable { count } => render_cardinal(
                locale,
                "journal.window.stored.body.summary.strava.kept.unreadable",
                *count,
                &[],
            ),
            Self::StravaSectionActivities => text(
                locale,
                "journal.window.stored.body.summary.strava.section.activities",
            ),
            Self::StravaRecordsWritten { count } => render_cardinal(
                locale,
                "journal.window.stored.body.summary.strava.records.written",
                *count,
                &[],
            ),
            Self::StravaRecordsNoFacet { count } => render_cardinal(
                locale,
                "journal.window.stored.body.summary.strava.records.no.facet",
                *count,
                &[],
            ),
            Self::StravaRecordsRefused => text(
                locale,
                "journal.window.stored.body.summary.strava.records.refused",
            ),
            Self::StravaConversionWaiting => text(
                locale,
                "journal.window.stored.body.summary.strava.conversion.waiting",
            ),
            Self::StravaConversionNoWorkout { count } => render_cardinal(
                locale,
                "journal.window.stored.body.summary.strava.conversion.no.workout",
                *count,
                &[],
            ),
            Self::StravaConversionMovesOpen { count } => render_cardinal(
                locale,
                "journal.window.stored.body.summary.strava.conversion.moves.open",
                *count,
                &[],
            ),
            Self::StravaSectionPeopleAndGear => text(
                locale,
                "journal.window.stored.body.summary.strava.section.people.and.gear",
            ),
            Self::StravaNamesStoreUnavailable => text(
                locale,
                "journal.window.stored.body.summary.strava.names.store.unavailable",
            ),
            Self::StravaNamesRead { count } => render_cardinal(
                locale,
                "journal.window.stored.body.summary.strava.names.read",
                *count,
                &[],
            ),
            Self::StravaNamesCached { count } => render_cardinal(
                locale,
                "journal.window.stored.body.summary.strava.names.cached",
                *count,
                &[],
            ),
            Self::StravaNamesFailed { count } => render_cardinal(
                locale,
                "journal.window.stored.body.summary.strava.names.failed",
                *count,
                &[],
            ),
            Self::StravaNamesWaiting { count, reason } => {
                let id = match reason {
                    StravaWaitReason::WaitingForWindow => {
                        "journal.window.stored.body.summary.strava.names.waiting.window"
                    }
                    StravaWaitReason::NoCondition => {
                        "journal.window.stored.body.summary.strava.names.waiting.no.condition"
                    }
                    StravaWaitReason::LaneUnavailable => {
                        "journal.window.stored.body.summary.strava.names.waiting.lane"
                    }
                    StravaWaitReason::SettingsUnreadable => {
                        "journal.window.stored.body.summary.strava.names.waiting.settings"
                    }
                    StravaWaitReason::Stopped => {
                        "journal.window.stored.body.summary.strava.names.waiting.stopped"
                    }
                };
                render_cardinal(locale, id, *count, &[])
            }
            Self::StravaNamesElsewhere { count } => render_cardinal(
                locale,
                "journal.window.stored.body.summary.strava.names.elsewhere",
                *count,
                &[],
            ),
            Self::StravaNamesSetAside { count } => render_cardinal(
                locale,
                "journal.window.stored.body.summary.strava.names.set.aside",
                *count,
                &[],
            ),
            Self::StravaNamesPromotionUnavailable => text(
                locale,
                "journal.window.stored.body.summary.strava.names.promotion.unavailable",
            ),
            Self::StravaNamesCreated { count } => render_cardinal(
                locale,
                "journal.window.stored.body.summary.strava.names.created",
                *count,
                &[],
            ),
            Self::StravaNamesNoneCreated => text(
                locale,
                "journal.window.stored.body.summary.strava.names.none.created",
            ),
        }
    }
}
