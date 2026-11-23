// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The facet question at a terminal: the same question the import card asks,
//! and every word of it, here.

use std::collections::BTreeSet;
use std::io::{BufRead, Write};
use std::path::Path;

use solstone_core_facets::{BodyFacetAsk, apply_body_facet_answer, body_facet_ask};
use solstone_core_import_sources::strava;
use solstone_core_journal_config::PREFERRED_BODY_FACET;
use solstone_core_journal_config_write::record_body_facet;

use crate::cli_argv::ImportFacet;

fn question(locale: &str, ask: &BodyFacetAsk) -> Option<String> {
    match ask {
        BodyFacetAsk::Chosen(_) | BodyFacetAsk::Declined => None,
        BodyFacetAsk::AskExisting(facet) => Some(solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.importer.body.facet.ask.existing",
            [("facet", facet)],
        )),
        BodyFacetAsk::AskMakeBody => Some(solstone_core_cli_copy::render_for(
            locale,
            "journal.cli.importer.body.facet.ask.make.body",
            &[],
        )),
        BodyFacetAsk::AskUnmute => Some(solstone_core_cli_copy::render_for(
            locale,
            "journal.cli.importer.body.facet.ask.unmute",
            &[],
        )),
    }
}

fn would_file_under(ask: &BodyFacetAsk) -> &str {
    match ask {
        BodyFacetAsk::AskExisting(facet) => facet,
        _ => PREFERRED_BODY_FACET,
    }
}

fn download_ids(media: &Path) -> BTreeSet<String> {
    strava::read_workouts(media, strava::DEFAULT_LIST_CAP)
        .map(|read| {
            read.workouts
                .iter()
                .map(|workout| workout.activity_id.to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// Ask, once, and do what the answer means. A yes files this import under the
/// facet, an answer that may move what's already listed, and says how many
/// first. Anything but yes or no, or end of input, is no answer.
pub(crate) fn ask_strava_facet(
    journal: &Path,
    media: &Path,
    input: &mut dyn BufRead,
    output: &mut dyn Write,
) -> (ImportFacet, Option<usize>) {
    let ids = download_ids(media);
    ask_facet(
        journal,
        "journal.cli.importer.body.facet.no.means.strava",
        "journal.cli.importer.body.facet.moves.strava",
        &|facet| crate::strava_convert::listed_moves(journal, &ids, facet),
        input,
        output,
    )
}

/// [`ask_strava_facet`] for any other body import: what a yes could move is
/// every record of that stream filed elsewhere.
pub(crate) fn ask_body_facet(
    journal: &Path,
    stream: &str,
    input: &mut dyn BufRead,
    output: &mut dyn Write,
) -> (ImportFacet, Option<usize>) {
    ask_facet(
        journal,
        "journal.cli.importer.body.facet.no.means.body",
        "journal.cli.importer.body.facet.moves.body",
        &|facet| {
            let (count, from) = crate::body_activity::listed_elsewhere(journal, stream, facet);
            (count > 0).then(|| {
                let titles = from
                    .iter()
                    .map(|facet| crate::strava_convert::facet_title(journal, facet))
                    .collect();
                (count, titles)
            })
        },
        input,
        output,
    )
}

type ListedCount<'a> = &'a dyn Fn(&str) -> Option<(usize, Vec<String>)>;

fn ask_facet(
    journal: &Path,
    no_means_id: &str,
    moves_id: &str,
    listed: ListedCount<'_>,
    input: &mut dyn BufRead,
    output: &mut dyn Write,
) -> (ImportFacet, Option<usize>) {
    let locale = solstone_core_cli_copy::cli_locale();
    let Ok((ask, _)) = body_facet_ask(journal) else {
        return (ImportFacet::Unattended, None);
    };
    let Some(text) = question(locale, &ask) else {
        return (ImportFacet::Unattended, None);
    };
    let moves = listed(would_file_under(&ask));
    let shown = moves.as_ref().map_or(0, |(count, _)| *count);
    let _ = writeln!(
        output,
        "{}",
        solstone_core_cli_copy::render_for(locale, no_means_id, &[])
    );
    if let Some((count, from)) = &moves {
        let count_str = count.to_string();
        let from_words = solstone_core_import_sources::apple_health::facets_words(from);
        let _ = writeln!(
            output,
            "{}",
            solstone_core_cli_copy::render_for(
                locale,
                moves_id,
                &[
                    (
                        "quantity",
                        solstone_core_cli_copy::Value::Cardinal(
                            i64::try_from(*count).unwrap_or(i64::MAX),
                        ),
                    ),
                    ("count", solstone_core_cli_copy::Value::String(&count_str)),
                    ("from", solstone_core_cli_copy::Value::String(&from_words)),
                ],
            )
        );
    }
    let _ = write!(
        output,
        "{} ",
        solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.importer.body.facet.prompt.yn",
            [("question", &text)],
        )
    );
    let _ = output.flush();
    let mut answer = String::new();
    if input.read_line(&mut answer).is_err() {
        return (ImportFacet::Unattended, None);
    }
    let yes = match answer.trim().to_ascii_lowercase().as_str() {
        "y" | "yes" => true,
        "n" | "no" => false,
        _ => return (ImportFacet::Unattended, None),
    };
    let choice = match apply_body_facet_answer(journal, &ask, yes) {
        Ok(Some(choice)) => choice,
        Ok(None) => return (ImportFacet::Unattended, None),
        Err(error) => {
            let _ = writeln!(
                output,
                "{}",
                solstone_core_cli_copy::display_for(
                    locale,
                    "journal.cli.importer.body.facet.save.failed",
                    [("error", &error)],
                )
            );
            return (ImportFacet::Unattended, None);
        }
    };
    if let Err(error) = record_body_facet(journal, &choice) {
        let _ = writeln!(
            output,
            "{}",
            solstone_core_cli_copy::display_for(
                locale,
                "journal.cli.importer.body.facet.save.failed",
                [("error", &error)],
            )
        );
        return (ImportFacet::Unattended, None);
    }
    match choice {
        solstone_core_journal_config::BodyFacetChoice::Facet(facet) => {
            (ImportFacet::Facet(facet), Some(shown))
        }
        solstone_core_journal_config::BodyFacetChoice::None => (ImportFacet::Unattended, None),
    }
}

#[cfg(test)]
#[path = "body_facet_question_tests.rs"]
mod tests;
