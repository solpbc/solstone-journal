// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The owner commands that bring Apple Health and Oura into the journal.
//!
//! Flags are checked first, then the facet question, then everything else: a
//! refused flag asks nothing and writes nothing, and a terminal left waiting on
//! the question holds no lock.

use std::io::IsTerminal as _;
use std::path::Path;

use jiff::civil::Date;
use jiff::{Span, Timestamp};
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

const FIRST_OURA_DAY: (i16, i8, i8) = (2015, 1, 1);

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

use solstone_core_import_sources::body_status::apple_go_ahead_words as apple_confirm_words;

fn apple_whole_words() -> String {
    format!(
        "{}\n",
        solstone_core_cli_copy::text("journal.cli.importer.body.apple.whole")
    )
}

fn already_ran_words() -> String {
    format!(
        "{}\n",
        solstone_core_cli_copy::text("journal.cli.importer.body.apple.already.ran")
    )
}

/// What a start that names an import is told when nothing about it is an Apple
/// Health card's save.
pub(crate) fn card_only_words() -> String {
    format!(
        "{}\n",
        solstone_core_cli_copy::text("journal.cli.importer.body.apple.card.only")
    )
}

/// A page start names the import it runs under. Accept it only when that import
/// was made by the Apple Health card for this export and has never run; refuse otherwise
/// before anything else is checked, so a start aimed at an import that already
/// ran never changes it.
pub(crate) fn accept_page_import(
    journal: &Path,
    import_id: &str,
    media: &str,
) -> Result<(), CliRun> {
    let refuse = |words: String| failure("", &words, 1);
    if solstone_core_import::validate_timestamp(import_id).is_err() {
        return Err(refuse(format!(
            "{}\n",
            solstone_core_cli_copy::text("journal.cli.importer.body.apple.unknown.import")
        )));
    }
    let dir = journal.join("imports").join(import_id);
    let Ok(Some(metadata)) = solstone_core_import::metadata::read_provenance(journal, import_id)
    else {
        return Err(refuse(format!(
            "{}\n",
            solstone_core_cli_copy::text("journal.cli.importer.body.apple.unknown.import")
        )));
    };
    // Only an import the Apple Health card made: a hand-typed start aimed at
    // another source's import must never write into it or remove its upload.
    if metadata
        .get("source_hint")
        .and_then(serde_json::Value::as_str)
        != Some("apple_health")
    {
        return Err(refuse(card_only_words()));
    }
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
        return Err(refuse(already_ran_words()));
    }
    let inside = match (Path::new(media).canonicalize(), dir.canonicalize()) {
        (Ok(media), Ok(dir)) => media.starts_with(dir),
        _ => false,
    };
    if !inside {
        return Err(refuse(format!(
            "{}\n",
            solstone_core_cli_copy::text("journal.cli.importer.body.apple.wrong.export")
        )));
    }
    Ok(())
}

/// After a page start: a refusal before its attempt was admitted says why on its
/// import, and the uploaded export goes, whatever happened, as Strava's does.
pub(crate) fn settle_page_import(
    journal: &Path,
    import_id: &str,
    failed_with: Option<&str>,
    refusal_code: Option<&'static str>,
) {
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
        let code = refusal_code.unwrap_or_else(|| {
            if solstone_core_import_sources::body_status::code_for_error_text(words) == "disk_full"
            {
                "no_room"
            } else {
                "not_started"
            }
        });
        solstone_core_import_sources::body_status::record(journal, import_id, code, 1);
    }
    // This run is over, so its own attempt goes. The upload goes too unless a
    // run still holds the import (a command-line run of the same export that
    // bound it while this one waited); the page's sweep removes it once that
    // run ends.
    solstone_core_import::metadata::release_attempt(journal, import_id);
    if solstone_core_import::metadata::attempt_holder(journal, import_id)
        == solstone_core_import::metadata::AttemptHolder::Held
    {
        return;
    }
    if let Err(error) = solstone_core_import::snapshot::discard_download(journal, import_id) {
        log::warn!("could not remove the Apple Health export for {import_id}: {error}");
    }
}

/// Save an Apple Health export, returning any typed early-refusal code for page runs.
pub(crate) fn save_apple(
    media: &str,
    command: &BodyCommand<'_>,
    journal: &Path,
) -> (CliRun, Option<&'static str>) {
    if !command.confirm {
        return (failure("", &apple_confirm_words(), 1), Some("go_ahead"));
    }
    if command.date_from.is_some() || command.date_to.is_some() || command.force {
        return (failure("", &apple_whole_words(), 1), Some("whole_export"));
    }
    let (facet, listed) = match settled_facet(command, &APPLE, journal) {
        Ok(settled) => settled,
        Err((run, code)) => return (run, code),
    };
    let earlier_waiting = sweep_first(journal);
    let timestamp = owner_timestamp(journal);
    let mut request = AppleSegmentsRequest::new(Path::new(media), &timestamp, facet.as_deref());
    request.import_id = command.import_id;
    request.listed = listed;
    request.earlier_waiting = earlier_waiting;
    let run = match run_apple_health_segments(journal, &request) {
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
    };
    (run, None)
}

/// A count as a person reads a large one: 2,000,000.
fn grouped(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
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
            return if json_output {
                let words = solstone_core_import_sources::body_status::words(code);
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
                let words = solstone_core_import_sources::body_status::words_localized(
                    solstone_core_cli_copy::cli_locale(),
                    code,
                );
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
    let locale = solstone_core_cli_copy::cli_locale();
    let reads = [
        "journal.cli.importer.body.apple.reads.workouts",
        "journal.cli.importer.body.apple.reads.sleep",
        "journal.cli.importer.body.apple.reads.mindful",
        "journal.cli.importer.body.apple.reads.readings",
        "journal.cli.importer.body.apple.reads.daily",
        "journal.cli.importer.body.apple.reads.logged",
    ]
    .map(|id| solstone_core_cli_copy::render_for(locale, id, &[]));
    let head = reads[..reads.len() - 1].join(", ");
    let tail = &reads[reads.len() - 1];
    let mut text = format!(
        "{}\n",
        solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.importer.body.apple.preview.intro",
            [("head", &head), ("tail", &tail)],
        )
    );
    if let Some((first, last)) = &preview.span {
        let count = |n: u64, id: &str| {
            let formatted = grouped(n);
            solstone_core_cli_copy::render_for(
                locale,
                id,
                &[
                    (
                        "quantity",
                        solstone_core_cli_copy::Value::Cardinal(
                            i64::try_from(n).unwrap_or(i64::MAX),
                        ),
                    ),
                    ("count", solstone_core_cli_copy::Value::String(&formatted)),
                ],
            )
        };
        let workouts = count(preview.workouts, "journal.cli.importer.count.workout");
        let nights = count(preview.nights, "journal.cli.importer.count.night");
        let mindful = count(
            preview.mindful_sessions,
            "journal.cli.importer.count.mindful.session",
        );
        let readings = count(preview.readings, "journal.cli.importer.count.reading");
        let daily = count(
            preview.daily_totals,
            "journal.cli.importer.count.daily.total",
        );
        let logged = count(preview.logged, "journal.cli.importer.count.logged.entry");
        text.push_str(&format!(
            "{}\n",
            solstone_core_cli_copy::display_for(
                locale,
                "journal.cli.importer.body.apple.preview.span",
                [
                    ("first", &first),
                    ("last", &last),
                    ("workouts", &workouts),
                    ("nights", &nights),
                    ("mindful", &mindful),
                    ("readings", &readings),
                    ("daily", &daily),
                    ("logged", &logged),
                ],
            )
        ));
    }
    if let Some((count, _)) = listed {
        let count_str = count.to_string();
        text.push_str(&format!(
            "{}\n",
            solstone_core_cli_copy::render_for(
                locale,
                "journal.cli.importer.body.apple.preview.moves",
                &[
                    (
                        "quantity",
                        solstone_core_cli_copy::Value::Cardinal(
                            i64::try_from(count).unwrap_or(i64::MAX),
                        ),
                    ),
                    ("count", solstone_core_cli_copy::Value::String(&count_str)),
                ],
            )
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

fn parse_day(flag: &str, text: &str) -> Result<Date, String> {
    Date::strptime("%Y-%m-%d", text)
        .ok()
        .or_else(|| solstone_core_time::parse_day(text))
        .ok_or_else(|| {
            format!(
                "{}\n",
                solstone_core_cli_copy::display(
                    "journal.cli.importer.body.oura.bad.date",
                    [("flag", &flag), ("text", &text)],
                )
            )
        })
}

/// The days an Oura run was asked for, or `None` for the days it doesn't hold.
fn oura_range(command: &BodyCommand<'_>, today: Date) -> Result<Option<(Date, Date)>, String> {
    if command.window_days == Some(0) {
        return Err(format!(
            "{}\n",
            solstone_core_cli_copy::text("journal.cli.importer.body.oura.window.zero")
        ));
    }
    if command.window_days.is_some() && (command.date_from.is_some() || command.date_to.is_some()) {
        return Err(format!(
            "{}\n",
            solstone_core_cli_copy::text("journal.cli.importer.body.oura.window.conflict")
        ));
    }
    let (year, month, day) = FIRST_OURA_DAY;
    let floor = Date::new(year, month, day).map_err(|_| "the first day isn't a day")?;
    if let Some(days) = command.window_days {
        let from = i64::try_from(days.saturating_sub(1))
            .ok()
            .and_then(|days| Span::new().try_days(days).ok())
            .and_then(|span| today.checked_sub(span).ok())
            .map_or(floor, |from| from.max(floor));
        return Ok(Some((from, today)));
    }
    match (command.date_from, command.date_to) {
        (None, None) => Ok(None),
        (None, Some(_)) => Err(format!(
            "{}\n",
            solstone_core_cli_copy::text("journal.cli.importer.body.oura.date.to.needs.from")
        )),
        (Some(from), to) => {
            let from = parse_day("--date-from", from)?;
            let to = to.map_or(Ok(today), |to| parse_day("--date-to", to))?;
            if from > today || to > today {
                return Err(format!(
                    "{}\n",
                    solstone_core_cli_copy::text("journal.cli.importer.body.oura.range.future")
                ));
            }
            Ok(Some((from, to)))
        }
    }
}

/// Bring Oura up to date, or with no `--save` say what that would ask for.
pub(crate) fn sync_oura(command: &BodyCommand<'_>, journal: &Path) -> CliRun {
    let zone = solstone_core_journal_config::owner_zone(journal);
    let today = Timestamp::now().to_zoned_datetime(zone.clone()).date();
    let range = match oura_range(command, today) {
        Ok(range) => range,
        Err(words) => return failure("", &words, 1),
    };
    if !command.save {
        return match endpoint_plan_lines(journal, range, today, &zone) {
            Ok(lines) => success(lines),
            Err(error) => failure("", &format!("{error}\n"), 1),
        };
    }
    let (facet, listed) = match settled_facet(command, &OURA, journal) {
        Ok(settled) => settled,
        Err((run, _)) => return run,
    };
    let earlier_waiting = sweep_first(journal);
    let timestamp = owner_timestamp(journal);
    let mut transport = LiveOuraTransport::default();
    let mut request = OuraImportRequest::new(&timestamp, facet.as_deref(), &mut transport);
    request.range = range;
    request.scheduled = command.scheduled;
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
    let counts = crate::strava_convert::sweep(journal, Timestamp::now());
    counts.waiting + counts.skipped_locked
}

fn owner_timestamp(journal: &Path) -> String {
    Timestamp::now()
        .to_zoned_datetime(solstone_core_journal_config::owner_zone(journal))
        .strftime("%Y%m%d_%H%M%S")
        .to_string()
}

type SettledFacet = (Option<String>, ListedMoves);
type SettledFacetError = (CliRun, Option<&'static str>);

/// The facet this run files under, asking first when an owner is there and
/// nothing has been answered, and what it may do with what's already listed.
fn settled_facet(
    command: &BodyCommand<'_>,
    source: &BodySource,
    journal: &Path,
) -> Result<SettledFacet, SettledFacetError> {
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
    let RunFacet { facet, basis } = run_facet(journal, &given)
        .map_err(|words| (failure("", &format!("{words}\n"), 1), Some("facet")))?;
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
