// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The owner commands that bring Apple Health and Oura into the journal.
//!
//! Flags are checked first, then the facet question, then everything else: a
//! refused flag asks nothing and writes nothing, and a terminal left waiting on
//! the question holds no lock.

use std::io::IsTerminal as _;
use std::path::Path;

use chrono::{Days, NaiveDate, Utc};
use serde_json::json;
use solstone_core_import_sources::body_stream::{APPLE, BodySource, OURA};

use crate::body_segments::{
    AppleSegmentsRequest, ListedMoves, LiveOuraTransport, OuraImportError, OuraImportRequest,
    import_oura, run_apple_health_segments,
};
use crate::cli_argv::{ImportFacet, failure, success};
use crate::oura_fetch::endpoint_plan_lines;
use crate::strava_filing::{FacetBasis, RunFacet, run_facet};
use solstone_core_import::cli_render::CliRun;

const FIRST_OURA_DAY: (i32, u32, u32) = (2015, 1, 1);

/// What a body import command was given, from its own flags.
pub(crate) struct BodyCommand<'a> {
    pub facet: ImportFacet,
    pub listed_moves_shown: Option<usize>,
    pub scheduled: bool,
    pub json: bool,
    pub confirm: bool,
    pub date_from: Option<&'a str>,
    pub date_to: Option<&'a str>,
    pub window_days: Option<u64>,
    pub force: bool,
    pub save: bool,
}

const APPLE_CONFIRM_WORDS: &str = "saving an Apple Health export needs your go-ahead, so nothing was saved. from a terminal, run: solstone journal importer <your export> --confirm-body-save\n";
const APPLE_WHOLE_WORDS: &str = "an Apple Health import always reads the whole export, and importing it again changes nothing, so it doesn't take --date-from, --date-to or --force. nothing was saved; run it again without them.\n";

/// Save an Apple Health export.
pub(crate) fn save_apple(media: &str, command: &BodyCommand<'_>, journal: &Path) -> CliRun {
    if !command.confirm {
        return failure("", APPLE_CONFIRM_WORDS, 1);
    }
    if command.date_from.is_some() || command.date_to.is_some() || command.force {
        return failure("", APPLE_WHOLE_WORDS, 1);
    }
    let (facet, listed) = match settled_facet(command, &APPLE, journal) {
        Ok(settled) => settled,
        Err(run) => return run,
    };
    let earlier_waiting = sweep_first(journal);
    let timestamp = owner_timestamp(journal);
    let mut request = AppleSegmentsRequest::new(Path::new(media), &timestamp, facet.as_deref());
    request.listed = listed;
    request.earlier_waiting = earlier_waiting;
    match run_apple_health_segments(journal, &request) {
        Ok(outcome) if command.json => success(format!(
            "{}\n",
            json!({
                "schema": "solstone.body.import.result.v2",
                "source": "apple_health",
                "import_id": outcome.import_id,
                "published": outcome.published,
                "finished": outcome.finished,
            })
        )),
        Ok(outcome) => success(format!("{}\n", outcome.summary.trim_end())),
        Err(error) => failure("", &format!("{error}\n"), 1),
    }
}

/// What an Apple export would bring: nothing is read beyond telling it's one.
pub(crate) fn preview_apple(json_output: bool) -> CliRun {
    if json_output {
        return success(format!(
            "{}\n",
            json!({
                "schema": "solstone.body.import.preview.v2",
                "source": "apple_health",
                "mode": "preview",
                "reads": APPLE_READS,
                "page_refusal": APPLE_CONFIRM_WORDS.trim_end(),
            })
        ));
    }
    success(format!(
        "this looks like an Apple Health export. saving it reads {}, and {}.\n",
        APPLE_READS[..APPLE_READS.len() - 1].join(", "),
        APPLE_READS[APPLE_READS.len() - 1]
    ))
}

const APPLE_READS: &[&str] = &[
    "workouts",
    "sleep",
    "mindful time",
    "heart rate and other readings",
    "daily totals like steps and distance",
    "logged entries like food and symptoms",
];

fn parse_day(flag: &str, text: &str) -> Result<NaiveDate, String> {
    NaiveDate::parse_from_str(text, "%Y-%m-%d")
        .or_else(|_| NaiveDate::parse_from_str(text, "%Y%m%d"))
        .map_err(|_| {
            format!(
                "{flag} takes a date like 2026-01-31, and {text} isn't one. nothing was read.\n"
            )
        })
}

/// The days an Oura run was asked for, or `None` for the days it doesn't hold.
fn oura_range(
    command: &BodyCommand<'_>,
    today: NaiveDate,
) -> Result<Option<(NaiveDate, NaiveDate)>, String> {
    if command.window_days == Some(0) {
        return Err("--window-days needs at least one day. nothing was read.\n".to_owned());
    }
    if command.window_days.is_some() && (command.date_from.is_some() || command.date_to.is_some()) {
        return Err("use --window-days or dates, not both. nothing was read.\n".to_owned());
    }
    let (year, month, day) = FIRST_OURA_DAY;
    let floor = NaiveDate::from_ymd_opt(year, month, day).ok_or("the first day isn't a day")?;
    if let Some(days) = command.window_days {
        let from = today
            .checked_sub_days(Days::new(days.saturating_sub(1)))
            .map_or(floor, |from| from.max(floor));
        return Ok(Some((from, today)));
    }
    match (command.date_from, command.date_to) {
        (None, None) => Ok(None),
        (None, Some(_)) => {
            Err("--date-to needs a --date-from to go with it. nothing was read.\n".to_owned())
        }
        (Some(from), to) => {
            let from = parse_day("--date-from", from)?;
            let to = to.map_or(Ok(today), |to| parse_day("--date-to", to))?;
            if from > today || to > today {
                return Err("that range goes past today, and Oura has nothing there yet. nothing was read.\n".to_owned());
            }
            Ok(Some((from, to)))
        }
    }
}

/// Bring Oura up to date, or with no `--save` say what that would ask for.
pub(crate) fn sync_oura(command: &BodyCommand<'_>, journal: &Path) -> CliRun {
    let zone = solstone_core_journal_config::owner_zone(journal);
    let today = Utc::now().with_timezone(&zone).date_naive();
    let range = match oura_range(command, today) {
        Ok(range) => range,
        Err(words) => return failure("", &words, 1),
    };
    if !command.save {
        return match endpoint_plan_lines(journal, range, today, zone) {
            Ok(lines) => success(lines),
            Err(error) => failure("", &format!("{error}\n"), 1),
        };
    }
    let (facet, listed) = match settled_facet(command, &OURA, journal) {
        Ok(settled) => settled,
        Err(run) => return run,
    };
    let earlier_waiting = sweep_first(journal);
    let timestamp = owner_timestamp(journal);
    let mut transport = LiveOuraTransport::default();
    let mut request = OuraImportRequest::new(&timestamp, facet.as_deref(), &mut transport);
    request.range = range;
    request.listed = listed;
    request.earlier_waiting = earlier_waiting;
    match import_oura(journal, request) {
        Ok(outcome) if command.json => success(format!(
            "{}\n",
            json!({
                "schema": "solstone.body.import.result.v2",
                "source": "oura",
                "import_id": outcome.import_id,
                "published": outcome.published,
                "finished": outcome.finished,
            })
        )),
        Ok(outcome) => success(format!("{}\n", outcome.summary.trim_end())),
        Err(OuraImportError::Stop(stop)) => failure("", &format!("{}\n", stop.words()), 1),
        Err(OuraImportError::Failed(error)) => failure("", &format!("{error}\n"), 1),
    }
}

/// Convert earlier Strava records before a new import can sit beside them, and
/// say how many it couldn't get to. A workout the lock held clears at the next
/// start.
fn sweep_first(journal: &Path) -> u64 {
    let counts = crate::strava_convert::sweep(journal, Utc::now());
    counts.waiting + counts.skipped_locked
}

fn owner_timestamp(journal: &Path) -> String {
    Utc::now()
        .with_timezone(&solstone_core_journal_config::owner_zone(journal))
        .format("%Y%m%d_%H%M%S")
        .to_string()
}

/// The facet this run files under, asking first when an owner is there and
/// nothing has been answered, and what it may do with what's already listed.
fn settled_facet(
    command: &BodyCommand<'_>,
    source: &BodySource,
    journal: &Path,
) -> Result<(Option<String>, ListedMoves), CliRun> {
    let (given, shown) = if command.facet != ImportFacet::Unattended
        || command.scheduled
        || command.json
        || !std::io::stdin().is_terminal()
        || !std::io::stdout().is_terminal()
    {
        (command.facet.clone(), command.listed_moves_shown)
    } else {
        crate::body_facet_question::ask_body_facet(
            journal,
            source.records().stream,
            &mut std::io::stdin().lock(),
            &mut std::io::stdout(),
        )
    };
    let RunFacet { facet, basis } =
        run_facet(journal, &given).map_err(|words| failure("", &format!("{words}\n"), 1))?;
    let listed = match (basis, shown) {
        (FacetBasis::Chosen, Some(shown)) => ListedMoves::MoveAtMost(shown),
        (FacetBasis::Chosen, None) => ListedMoves::Move,
        _ => ListedMoves::Keep,
    };
    Ok((facet, listed))
}

#[cfg(test)]
#[path = "body_commands_tests.rs"]
mod tests;
