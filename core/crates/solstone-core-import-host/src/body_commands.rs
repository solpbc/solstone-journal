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
    /// The import a page start runs under, which `run_import` has accepted.
    pub import_id: Option<&'a str>,
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

/// Where an owner on this platform can type the command.
const WHERE_TO_RUN: &str = if cfg!(target_os = "macos") {
    "in the journal app's journal menu, choose open admin terminal, and run"
} else {
    "from a terminal on the computer your journal runs on, run"
};

fn apple_confirm_words() -> String {
    format!(
        "saving an Apple Health export needs your go-ahead, so nothing from it was brought in. {WHERE_TO_RUN}: solstone journal importer <your export> --confirm-body-save\n"
    )
}
const APPLE_WHOLE_WORDS: &str = "an Apple Health import always reads the whole export, and importing it again changes nothing, so it doesn't take --date-from, --date-to or --force. nothing was saved; run it again without them.\n";

/// What a page start of an import that already ran is told; the page shows
/// its own words for it.
const ALREADY_RAN: &str = "this import has already run, so nothing was brought in. start a new one from the Apple Health card.\n";

/// A page start names the import it runs under. Accept it only when that import
/// was made by the page for this export and has never run; refuse otherwise
/// before anything else is checked, so a start aimed at an import that already
/// ran never changes it.
pub(crate) fn accept_page_import(
    journal: &Path,
    import_id: &str,
    media: &str,
) -> Result<(), CliRun> {
    let refuse = |words: &str| failure("", words, 1);
    if solstone_core_import::validate_timestamp(import_id).is_err() {
        return Err(refuse(
            "--import-id names no import, so nothing was brought in.\n",
        ));
    }
    let dir = journal.join("imports").join(import_id);
    let Ok(Some(metadata)) = solstone_core_import::metadata::read_provenance(journal, import_id)
    else {
        return Err(refuse(
            "--import-id names no import, so nothing was brought in.\n",
        ));
    };
    let has_attempt = !matches!(
        solstone_core_import::metadata::read_attempt_facts(&metadata),
        solstone_core_import::metadata::AttemptRead::Absent
    );
    if has_attempt
        || crate::body_segments::holds_a_run(journal, import_id)
        || dir
            .join(solstone_core_import_sources::body_status::STOP_FILE)
            .exists()
    {
        return Err(refuse(ALREADY_RAN));
    }
    let inside = match (Path::new(media).canonicalize(), dir.canonicalize()) {
        (Ok(media), Ok(dir)) => media.starts_with(dir),
        _ => false,
    };
    if !inside {
        solstone_core_import_sources::body_status::record(journal, import_id, "not_this_import", 1);
        return Err(refuse(
            "that import doesn't hold this export, so nothing was brought in.\n",
        ));
    }
    Ok(())
}

/// After a page start: a refusal before its attempt was admitted says why on its
/// import, and the uploaded export goes, whatever happened, as Strava's does.
pub(crate) fn settle_page_import(journal: &Path, import_id: &str, failed_with: Option<&str>) {
    let stop = solstone_core_import_sources::body_status::read(journal, import_id);
    let no_attempt = matches!(
        solstone_core_import::metadata::read_provenance(journal, import_id)
            .ok()
            .flatten()
            .map(|metadata| solstone_core_import::metadata::read_attempt_facts(&metadata)),
        Some(solstone_core_import::metadata::AttemptRead::Absent) | None
    );
    if let Some(words) = failed_with
        && no_attempt
        && stop.is_none()
    {
        let code = if words.contains("preview again") {
            "preview_again"
        } else if words.trim_end() == apple_confirm_words().trim_end() {
            "go_ahead"
        } else if words.trim_end() == APPLE_WHOLE_WORDS.trim_end() {
            "whole_export"
        } else if words.contains(crate::cli_argv::FACET_REFUSAL_MARKER) {
            "facet"
        } else if solstone_core_import_sources::body_status::code_for_error_text(words)
            == "disk_full"
        {
            "disk_full"
        } else {
            "not_started"
        };
        solstone_core_import_sources::body_status::record(journal, import_id, code, 1);
    }
    // An import another run has taken (a command-line run of the same export
    // bound it while this one waited) keeps what it holds: this run ended
    // nothing there.
    let latest = match solstone_core_import::metadata::read_provenance(journal, import_id)
        .ok()
        .flatten()
        .map(|metadata| solstone_core_import::metadata::read_attempt_facts(&metadata))
    {
        Some(solstone_core_import::metadata::AttemptRead::Present(facts)) => facts.generation,
        _ => 0,
    };
    let ours = failed_with.is_none()
        || latest == 0
        || solstone_core_import_sources::body_status::read(journal, import_id)
            .is_some_and(|(_, generation)| generation == latest);
    if !ours {
        return;
    }
    if let Err(error) = solstone_core_import::snapshot::discard_download(journal, import_id) {
        log::warn!("could not remove the Apple Health export for {import_id}: {error}");
    }
}

/// Save an Apple Health export.
pub(crate) fn save_apple(media: &str, command: &BodyCommand<'_>, journal: &Path) -> CliRun {
    if !command.confirm {
        return failure("", &apple_confirm_words(), 1);
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
    request.import_id = command.import_id;
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

/// What an Apple export holds, counted in one pass that stages and writes
/// nothing. With a facet named, it also says how many records already listed
/// elsewhere a run could move: an upper bound, because what an export touches
/// isn't known until it's read in full. A refused export answers in words with
/// `--json` (exit 0), so the page shows them and can't start it.
pub(crate) fn preview_apple(
    media: &str,
    journal: &Path,
    facet: &ImportFacet,
    json_output: bool,
) -> CliRun {
    let preview = match solstone_core_import_sources::apple_health::preview_export(Path::new(media))
    {
        Ok(preview) => preview,
        Err(error) => {
            let code =
                solstone_core_import_sources::body_status::code_for_source_stage(error.stage());
            let words = solstone_core_import_sources::body_status::words(code);
            return if json_output {
                success(format!(
                    "{}\n",
                    json!({
                        "schema": "solstone.import.size.v1",
                        "source": "apple_health",
                        "mode": "preview",
                        "refusal": code,
                        "page_refusal": words,
                    })
                ))
            } else {
                failure("", &format!("{words}\n"), 1)
            };
        }
    };
    let listed = match facet {
        ImportFacet::Facet(name) => {
            let (count, from) =
                crate::body_activity::listed_elsewhere(journal, APPLE.records().stream, name);
            (count > 0).then(|| (count, from.into_iter().collect::<Vec<_>>()))
        }
        _ => None,
    };
    if json_output {
        let mut size = json!({
            "schema": "solstone.import.size.v1",
            "source": "apple_health",
            "mode": "preview",
            "recordings": 0,
            "days": 0,
            "skipped": 0,
            "other_units": ["Apple Health records", preview.read()],
            "apple_health": preview,
            "reads": APPLE_READS,
            "go_ahead": apple_confirm_words().trim_end(),
        });
        if let Some((count, from)) = listed {
            size["listed_moves"] = json!({"count": count, "from": from, "upper_bound": true});
        }
        return success(format!("{size}\n"));
    }
    let mut text = format!(
        "this looks like an Apple Health export. saving it reads {}, and {}.\n",
        APPLE_READS[..APPLE_READS.len() - 1].join(", "),
        APPLE_READS[APPLE_READS.len() - 1]
    );
    if let Some((first, last)) = &preview.span {
        text.push_str(&format!(
            "it runs from {first} to {last}: {} workouts, {} nights put together from sleep stages, {} mindful sessions, {} readings, {} daily totals and {} logged entries.\n",
            preview.workouts,
            preview.nights,
            preview.mindful_sessions,
            preview.readings,
            preview.daily_totals,
            preview.logged
        ));
    }
    if let Some((count, _)) = listed {
        text.push_str(&format!(
            "a yes can move up to {count} {} already listed under another facet.\n",
            if count == 1 { "record" } else { "records" }
        ));
    }
    success(text)
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
