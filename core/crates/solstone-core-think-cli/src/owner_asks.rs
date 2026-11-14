// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! How work discovery works the owner's asks to think a corrected day again.
//!
//! Each pass seeds the asked activities before it reads activity work, holds
//! the day's daily work while that work is unfinished, lets the frozen
//! briefing regenerate while the ask thaws it, and then re-makes the week's
//! page. Every step is read from what is already on disk compared with the
//! ask's time, so each one stops on its own once the newer output exists.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::Arc;

use chrono::{Datelike, NaiveDate};
use solstone_core_journal_config::work_priority::{WorkBand, WorkPriority};
use solstone_core_system::daily_coverage::{self, CoverageState};
use solstone_core_system::owner_ask::{self, OwnerAsk, WEEK_BUILT};
use solstone_core_system::partition::canonical_journal_command;
use solstone_core_system::queue::DiscoveredTask;
use solstone_core_system::request::{BusTaskRequest, TaskArgv};
use solstone_core_system::work_intent::OwnerIntent;
use solstone_core_system::work_inventory::{OwnerAskProgress, PendingOwnerAsk};
use solstone_core_system::work_policy::WorkPolicy;

use crate::activity::Seeded;
use crate::activity_work::ActivityRetry;

/// Why an asked activity that was handed to its work was dropped: the work
/// was set aside or paused rather than finished.
pub(crate) const ACTIVITY_SET_ASIDE: &str = "activity_set_aside";
/// How many times one ask counts an attempt to re-make its week's page.
pub(crate) const WEEK_ATTEMPTS: u32 = 3;
/// At most one attempt is counted in this long while the page stays owed.
pub(crate) const WEEK_ATTEMPT_SPACING_MS: i64 = 3_600_000;

#[cfg(test)]
thread_local! {
    /// Lets a test plan a journal as though no ask could be read at all.
    pub(crate) static READER_DISABLED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn read_asks(journal: &Path) -> Result<BTreeMap<String, OwnerAsk>, String> {
    #[cfg(test)]
    if READER_DISABLED.with(std::cell::Cell::get) {
        return Ok(BTreeMap::new());
    }
    owner_ask::read(journal)
}

/// What an ask on one day does to that day's work in this pass.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct DayAsk {
    /// Some ask being worked names this day.
    pub(crate) asked: bool,
    /// The day's daily work waits for its asked activities.
    pub(crate) holds: bool,
    /// The newest such ask's creation time.
    pub(crate) asked_at_ms: Option<i64>,
}

impl DayAsk {
    /// Work the owner is waiting on takes band 2, unless its own band is higher.
    pub(crate) fn band(self, band: WorkBand) -> WorkBand {
        if self.asked && band > WorkBand::Waiting {
            WorkBand::Waiting
        } else {
            band
        }
    }
}

/// The owner's asks as one discovery pass sees them.
#[derive(Default)]
pub(crate) struct AskPass {
    asks: BTreeMap<String, OwnerAsk>,
    /// Asks on a day an owner intent holds, with what they wait on.
    waiting: BTreeMap<String, &'static str>,
    days: BTreeMap<String, DayAsk>,
    unreadable: Option<String>,
    errors: Vec<String>,
    /// Asks this pass finished, kept for its report.
    ended: Vec<OwnerAsk>,
    /// What each ask still waits on after this pass.
    waiting_on: BTreeMap<String, &'static str>,
}

fn intent_hold(intent: Option<&OwnerIntent>) -> Option<&'static str> {
    let intent = intent?;
    if intent.owner_stopped {
        Some("owner_stopped")
    } else if intent.from_scratch {
        Some("from_scratch")
    } else {
        None
    }
}

/// A think context for one asked day, reading talents from the host's
/// resources when it supplies them.
fn context(
    journal: &Path,
    day: &str,
    now_ms: i64,
    roots: Option<(&Path, &Path)>,
) -> Result<crate::context::ThinkContext, String> {
    let day_dir = journal.join("chronicle").join(day);
    let clock = Arc::new(move || now_ms);
    match roots {
        Some((talent, apps)) => crate::context::ThinkContext::new_with_roots(
            journal,
            day.to_owned(),
            day_dir,
            now_ms,
            clock,
            talent.to_path_buf(),
            apps.to_path_buf(),
        ),
        None => crate::context::ThinkContext::new_with_event_clock(
            journal,
            day.to_owned(),
            day_dir,
            now_ms,
            clock,
        ),
    }
}

impl AskPass {
    /// Read the asks and seed every activity still to seed, before the pass
    /// reads activity work. An unreadable store works no ask.
    pub(crate) fn seed(
        journal: &Path,
        intents: &BTreeMap<String, OwnerIntent>,
        roots: Option<(&Path, &Path)>,
        now_ms: i64,
        checkpoint: &dyn Fn() -> Result<(), String>,
    ) -> Result<Self, String> {
        let mut pass = Self::default();
        let asks = match read_asks(journal) {
            Ok(asks) => asks,
            Err(error) => {
                pass.unreadable = Some(error);
                return Ok(pass);
            }
        };
        let mut contexts = BTreeMap::new();
        for (id, ask) in &asks {
            if let Some(hold) = intent_hold(intents.get(&ask.day)) {
                pass.waiting.insert(id.clone(), hold);
                continue;
            }
            for name in &ask.to_seed {
                checkpoint()?;
                if !contexts.contains_key(&ask.day) {
                    match context(journal, &ask.day, now_ms, roots) {
                        Ok(context) => {
                            contexts.insert(ask.day.clone(), context);
                        }
                        Err(error) => {
                            pass.errors.push(format!("{}: {error}", ask.day));
                            break;
                        }
                    }
                }
                let seeded =
                    crate::activity::seed_refresh(&contexts[&ask.day], &name.activity, &name.facet);
                let dropped = match seeded {
                    Ok(Seeded::Busy) => continue,
                    Ok(Seeded::Recorded) => None,
                    Ok(Seeded::Dropped(reason)) => Some(reason),
                    Err(error) => {
                        pass.errors.push(format!("{}: {error}", ask.day));
                        continue;
                    }
                };
                // Move only this name, against the ask as it is now, so a
                // name or ask added since the read is never lost.
                if let Err(error) = owner_ask::update(journal, id, |current| {
                    let mut next = current.clone();
                    if !next.to_seed.remove(name) {
                        return None;
                    }
                    match dropped {
                        None => {
                            next.seeded.insert(name.clone());
                        }
                        Some(reason) => {
                            *next.dropped.entry(reason.to_owned()).or_default() += 1;
                        }
                    }
                    Some(Some(next))
                }) {
                    pass.errors.push(format!("{}: {error}", ask.day));
                }
            }
        }
        match read_asks(journal) {
            Ok(asks) => pass.asks = asks,
            Err(error) => {
                pass.unreadable = Some(error);
                pass.waiting.clear();
            }
        }
        Ok(pass)
    }

    /// Settle which days the asks hold, from the pass-start snapshot of
    /// activity work. An asked activity whose work was set aside, failed past
    /// its retries or paused, is no longer being run for the ask: it moves to
    /// the ask's dropped activities, so nothing reads it as still updating.
    pub(crate) fn hold(&mut self, journal: &Path, unfinished: &BTreeSet<ActivityRetry>) {
        let mut set_aside = Vec::new();
        for (id, ask) in &self.asks {
            if self.waiting.contains_key(id) {
                continue;
            }
            for name in &ask.seeded {
                let identity = ActivityRetry {
                    day: ask.day.clone(),
                    facet: name.facet.clone(),
                    activity: name.activity.clone(),
                };
                if !unfinished.contains(&identity)
                    && crate::activity_work::has_persisted_disposition(
                        journal,
                        &identity.day,
                        &identity.facet,
                        &identity.activity,
                    )
                    .unwrap_or(false)
                {
                    set_aside.push((id.clone(), name.clone()));
                }
            }
        }
        for (id, name) in set_aside {
            self.apply(journal, &id, |current| {
                let mut next = current.clone();
                if !next.seeded.remove(&name) {
                    return None;
                }
                *next
                    .dropped
                    .entry(ACTIVITY_SET_ASIDE.to_owned())
                    .or_default() += 1;
                Some(Some(next))
            });
        }
        for (id, ask) in &self.asks {
            if self.waiting.contains_key(id) {
                continue;
            }
            let day = self.days.entry(ask.day.clone()).or_default();
            day.asked = true;
            day.asked_at_ms = Some(day.asked_at_ms.unwrap_or(i64::MIN).max(ask.created_at_ms));
            day.holds |= !ask.to_seed.is_empty()
                || ask.seeded.iter().any(|name| {
                    unfinished.contains(&ActivityRetry {
                        day: ask.day.clone(),
                        facet: name.facet.clone(),
                        activity: name.activity.clone(),
                    })
                });
        }
    }

    /// Days an ask names, so the pass inspects them.
    pub(crate) fn days(&self) -> impl Iterator<Item = &String> {
        self.days.keys()
    }

    pub(crate) fn day(&self, day: &str) -> DayAsk {
        self.days.get(day).copied().unwrap_or_default()
    }

    /// The store could not be read; nothing was worked.
    pub(crate) fn unreadable(&self) -> Option<String> {
        self.unreadable
            .as_ref()
            .map(|error| format!("owner asks: {error}"))
    }

    /// After every day is inspected: end thaws whose briefing no longer owes
    /// a run, judge each ended week's page, remove finished asks, and return
    /// the week page runs still owed.
    pub(crate) fn finish(
        &mut self,
        journal: &Path,
        roots: Option<(&Path, &Path)>,
        today: NaiveDate,
        policy: WorkPolicy,
        now_ms: i64,
        checkpoint: &dyn Fn() -> Result<(), String>,
    ) -> Result<Vec<DiscoveredTask>, String> {
        let mut briefings = BTreeMap::<String, Briefing>::new();
        let ids = self
            .asks
            .keys()
            .filter(|id| !self.waiting.contains_key(*id))
            .cloned()
            .collect::<Vec<_>>();
        for id in &ids {
            checkpoint()?;
            let ask = self.asks[id].clone();
            let held = self.day(&ask.day).holds;
            if !briefings.contains_key(&ask.day) {
                let briefing = briefing(journal, &ask.day, roots).unwrap_or_else(|error| {
                    self.errors.push(format!("{}: {error}", ask.day));
                    Briefing::unknown()
                });
                briefings.insert(ask.day.clone(), briefing);
            }
            let briefing = &briefings[&ask.day];
            // A delete's ask keeps a capped briefing pending: the old one stays
            // out of search until a run on new evidence makes one.
            let capped_trash = ask.from_trash() && briefing.capped;
            if ask.thaw && !ask.thaw_done && !held && !briefing.owed && !capped_trash {
                let capped = briefing.capped;
                let read_at = ask.created_at_ms;
                let ended = self.apply(journal, id, |current| {
                    // A later change merged into the ask reopened it: not this.
                    (current.thaw && !current.thaw_done && current.created_at_ms == read_at).then(
                        || {
                            let mut next = current.clone();
                            next.thaw_done = true;
                            next.thaw_capped = capped;
                            Some(next)
                        },
                    )
                });
                if ended && ask.from_trash() {
                    requeue(
                        journal,
                        &format!("chronicle/{}/talents/morning_briefing.json", ask.day),
                    );
                }
            }
        }
        let settled = |ask: &OwnerAsk, days: &BTreeMap<String, DayAsk>| {
            !days.get(&ask.day).is_some_and(|day| day.holds)
                && (!ask.thaw || ask.thaw_done)
                && !briefings[&ask.day].owed
        };
        let mut weeks = BTreeMap::<NaiveDate, Vec<String>>::new();
        for id in &ids {
            let Some(ask) = self.asks.get(id) else {
                continue;
            };
            if ask.week.ended.is_none() {
                weeks
                    .entry(week_start(&ask.day)?)
                    .or_default()
                    .push(id.clone());
            }
        }
        let mut tasks = Vec::new();
        for (sunday, members) in weeks {
            checkpoint()?;
            let saturday = sunday + chrono::Duration::days(6);
            if saturday >= today {
                for id in &members {
                    self.waiting_on.insert(id.clone(), "week_not_ended");
                }
                continue;
            }
            if members
                .iter()
                .any(|id| !settled(&self.asks[id], &self.days))
            {
                continue;
            }
            let reference = members
                .iter()
                .map(|id| {
                    let ask = &self.asks[id];
                    ask.created_at_ms
                        .max(briefings[&ask.day].committed_at_ms.unwrap_or(i64::MIN))
                })
                .max()
                .expect("a week has an ask");
            let start = sunday.format("%Y%m%d").to_string();
            let trash = members.iter().any(|id| self.asks[id].from_trash());
            let owed = match week_page(journal, &start, reference, trash) {
                Page::Ended(reason) => {
                    for id in &members {
                        let read_at = self.asks[id].created_at_ms;
                        let ended = self.apply(journal, id, |current| {
                            (current.week.ended.is_none() && current.created_at_ms == read_at).then(
                                || {
                                    let mut next = current.clone();
                                    next.week.ended = Some(reason.to_owned());
                                    Some(next)
                                },
                            )
                        });
                        if ended && self.asks.get(id).is_some_and(OwnerAsk::from_trash) {
                            requeue(journal, &format!("reflections/weekly/{start}.md"));
                        }
                    }
                    false
                }
                Page::Owed => {
                    let mut owed = false;
                    for id in &members {
                        let read = self.asks[id].week.clone();
                        let from_trash = self.asks[id].from_trash();
                        let due = read
                            .counted_at_ms
                            .is_none_or(|at| now_ms - at >= week_spacing(&read, from_trash));
                        if !due {
                            owed = true;
                            continue;
                        }
                        // An attempt counts only when the lane can make the page.
                        if !policy.permits(WorkBand::Waiting) {
                            owed = true;
                            continue;
                        }
                        // A delete's ask never gives up: it backs off instead.
                        let exhausted = !from_trash && read.attempts >= WEEK_ATTEMPTS;
                        self.apply(journal, id, |current| {
                            (current.week == read).then(|| {
                                let mut next = current.clone();
                                if exhausted {
                                    next.week.ended = Some("attempts_exhausted".into());
                                } else {
                                    next.week.attempts += 1;
                                    next.week.counted_at_ms = Some(now_ms);
                                }
                                Some(next)
                            })
                        });
                        owed |= !exhausted;
                    }
                    owed
                }
            };
            if !owed {
                continue;
            }
            for id in &members {
                self.waiting_on.insert(id.clone(), "week");
            }
            let band = WorkBand::Waiting;
            if !policy.permits(band) {
                continue;
            }
            let saturday = saturday.format("%Y%m%d").to_string();
            let mut priority = solstone_core_system::priority::classify(
                &canonical_journal_command(["think", "--day", &saturday, "--weekly"]),
                today,
            );
            priority.band = band;
            tasks.push(week_task(&start, &saturday, priority));
        }
        for id in &ids {
            let Some(ask) = self.asks.get(id).cloned() else {
                continue;
            };
            if ask.finished() {
                if self.apply(journal, id, |current| (*current == ask).then_some(None)) {
                    self.ended.push(ask);
                }
                continue;
            }
            let waiting_on = if self.day(&ask.day).holds {
                "activities"
            } else if (ask.thaw && !ask.thaw_done) || briefings[&ask.day].owed {
                "briefing"
            } else {
                self.waiting_on.get(id).copied().unwrap_or("week")
            };
            self.waiting_on.insert(id.clone(), waiting_on);
        }
        Ok(tasks)
    }

    /// Change one ask against what it now holds; keep this pass's copy in step.
    fn apply(
        &mut self,
        journal: &Path,
        id: &str,
        change: impl FnOnce(&OwnerAsk) -> Option<Option<OwnerAsk>>,
    ) -> bool {
        let mut result = None;
        let applied = owner_ask::update(journal, id, |current| {
            let next = change(current);
            result = next.clone();
            next
        });
        match applied {
            Ok(true) => {
                match result.flatten() {
                    Some(next) => self.asks.insert(id.to_owned(), next),
                    None => self.asks.remove(id),
                };
                true
            }
            Ok(false) => false,
            Err(error) => {
                self.errors.push(format!("owner ask {id}: {error}"));
                false
            }
        }
    }

    /// Errors this pass met while working asks.
    pub(crate) fn errors(&self) -> &[String] {
        &self.errors
    }

    /// The report health shows.
    pub(crate) fn progress(&self) -> Option<OwnerAskProgress> {
        if self.asks.is_empty() && self.ended.is_empty() && self.unreadable.is_none() {
            return None;
        }
        let mut progress = OwnerAskProgress {
            unreadable: self.unreadable.is_some(),
            ..OwnerAskProgress::default()
        };
        for (id, ask) in &self.asks {
            progress.pending.push(PendingOwnerAsk {
                id: id.clone(),
                day: ask.day.clone(),
                waiting_on: self
                    .waiting
                    .get(id)
                    .or_else(|| self.waiting_on.get(id))
                    .copied()
                    .unwrap_or(if ask.to_seed.is_empty() {
                        "briefing"
                    } else {
                        "activities"
                    })
                    .to_owned(),
            });
        }
        for ask in self.asks.values().chain(&self.ended) {
            for (reason, count) in &ask.dropped {
                *progress.dropped.entry(reason.clone()).or_default() += count;
            }
            if let Some(reason) = ask.week.ended.as_ref().filter(|r| *r != WEEK_BUILT) {
                *progress.week_unfinished.entry(reason.clone()).or_default() += 1;
            }
            progress.thaw_capped += u64::from(ask.thaw_capped);
        }
        Some(progress)
    }
}

/// How long after one counted week page attempt the next may count: an hour,
/// and for a delete's ask, which never gives up, doubling up to a day.
fn week_spacing(step: &solstone_core_system::owner_ask::WeekStep, from_trash: bool) -> i64 {
    if !from_trash {
        return WEEK_ATTEMPT_SPACING_MS;
    }
    let doublings = step.attempts.saturating_sub(1).min(5);
    (WEEK_ATTEMPT_SPACING_MS << doublings).min(24 * WEEK_ATTEMPT_SPACING_MS)
}

/// Queue a file a delete's ask kept out of search, now that it no longer does:
/// a file indexed while hidden is otherwise not read again until it changes.
fn requeue(journal: &Path, rel: &str) {
    let path = journal.join(rel);
    if path.exists()
        && let Err(error) = solstone_core_indexer_store::todo::enqueue_path(journal, &path)
    {
        log::warn!("owner ask: search update for {rel} could not be queued: {error}");
    }
}

/// The Sunday that starts the week holding `day`.
fn week_start(day: &str) -> Result<NaiveDate, String> {
    let day = NaiveDate::parse_from_str(day, "%Y%m%d").map_err(|error| error.to_string())?;
    Ok(day - chrono::Duration::days(i64::from(day.weekday().num_days_from_sunday())))
}

/// What a day's morning briefing says about an ask on that day.
struct Briefing {
    owed: bool,
    /// Its latest attempt reached its failure cap on the current evidence.
    capped: bool,
    committed_at_ms: Option<i64>,
}

impl Briefing {
    fn unknown() -> Self {
        Self {
            owed: true,
            capped: false,
            committed_at_ms: None,
        }
    }
}

fn briefing(journal: &Path, day: &str, roots: Option<(&Path, &Path)>) -> Result<Briefing, String> {
    let coverage = match roots {
        Some((talent, apps)) => {
            daily_coverage::read_daily_coverage_with_roots(journal, day, talent, apps)?
        }
        None => daily_coverage::read_daily_coverage(journal, day)?,
    };
    let Some(unit) = coverage
        .units
        .iter()
        .find(|unit| unit.identity.name == "morning_briefing" && unit.identity.facet.is_none())
    else {
        return Ok(Briefing {
            owed: false,
            capped: false,
            committed_at_ms: None,
        });
    };
    let record = solstone_core_journal_io::load_daily_unit_record(journal, &unit.identity)
        .map_err(|error| error.to_string())?;
    Ok(Briefing {
        owed: unit.state.is_owed(),
        capped: unit.state == CoverageState::CurrentDegraded
            && record.as_ref().is_some_and(|record| {
                record.status == solstone_core_journal_io::DailyUnitStatus::Capped
                    && record.evidence_revision == unit.evidence_revision
            }),
        committed_at_ms: record
            .and_then(|record| record.accepted)
            .map(|accepted| accepted.committed_at_ms),
    })
}

enum Page {
    Owed,
    Ended(&'static str),
}

/// Whether the week's page, starting `start`, must be made again after
/// `reference`. A page that cannot be shown or trusted is not re-made by an
/// ask; one whose content is malformed is.
///
/// For a delete's ask (`trash`), a page that is there but can't be judged is
/// made again rather than left: it may still hold what was deleted.
fn week_page(journal: &Path, start: &str, reference: i64, trash: bool) -> Page {
    use solstone_core_home::weekly::WeekJudgment;
    match solstone_core_home::weekly::judge_page(journal, start) {
        Ok(WeekJudgment::Absent) => Page::Ended("page_absent"),
        Err(_) | Ok(WeekJudgment::CantShow | WeekJudgment::Untrusted) if trash => Page::Owed,
        Err(_) => Page::Ended("page_unreadable"),
        Ok(WeekJudgment::CantShow) => Page::Ended("page_unshowable"),
        Ok(WeekJudgment::Untrusted) => Page::Ended("page_untrusted"),
        Ok(WeekJudgment::CouldntCheck) => Page::Owed,
        Ok(WeekJudgment::Page(page)) => {
            if page.generated_at_ms.is_some_and(|built| built >= reference) {
                Page::Ended(WEEK_BUILT)
            } else {
                Page::Owed
            }
        }
    }
}

fn week_reference(start: &str) -> String {
    format!("supervisor-owner-week-{start}")
}

/// Each day a pending ask names, with the queue reference of the run that
/// re-makes that day's week page. An unreadable store names no day.
pub(crate) fn asked_days(journal: &Path) -> BTreeMap<String, String> {
    read_asks(journal)
        .unwrap_or_default()
        .into_values()
        .filter_map(|ask| {
            let start = week_start(&ask.day).ok()?.format("%Y%m%d").to_string();
            Some((ask.day, week_reference(&start)))
        })
        .collect()
}

fn week_task(start: &str, saturday: &str, priority: WorkPriority) -> DiscoveredTask {
    DiscoveredTask {
        request: BusTaskRequest {
            cmd: TaskArgv::from_wire(canonical_journal_command([
                "think", "--day", saturday, "--weekly",
            ]))
            .expect("weekly command"),
            reference: week_reference(start),
            day: Some(saturday.to_owned()),
            scheduler_name: None,
            queue_if_active_cmd_differs: false,
            priority: Some(priority),
            daily_catchup_provenance: None,
        },
    }
}

/// Whether a queued week page run for the week ending `saturday` is still
/// asked for. The pass that offered it judged the page.
pub(crate) fn week_still_owed(journal: &Path, saturday: &str) -> Result<bool, String> {
    let saturday =
        NaiveDate::parse_from_str(saturday, "%Y%m%d").map_err(|error| error.to_string())?;
    let sunday = saturday - chrono::Duration::days(6);
    Ok(read_asks(journal)?.values().any(|ask| {
        ask.week.ended.is_none() && week_start(&ask.day).is_ok_and(|start| start == sunday)
    }))
}
