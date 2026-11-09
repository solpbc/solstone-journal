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

const NO_MEANS_STRAVA: &str = "if you say no, new workouts aren't listed on the activities page, no people or things come from them, and agents limited to some facets won't see them. search and the body app still show them.";
const NO_MEANS_BODY: &str = "if you say no, new workouts and sleep aren't listed on the activities page, and agents limited to some facets won't see them. search and the body app still show them.";

fn question(ask: &BodyFacetAsk) -> Option<String> {
    match ask {
        BodyFacetAsk::Chosen(_) | BodyFacetAsk::Declined => None,
        BodyFacetAsk::AskExisting(facet) => Some(format!(
            "file your workouts, sleep and other body data under the facet you named {facet}?"
        )),
        BodyFacetAsk::AskMakeBody => {
            Some("make a body facet for your workouts, sleep and other body data?".to_owned())
        }
        BodyFacetAsk::AskUnmute => Some(
            "your body facet is muted. turn it back on for your workouts, sleep and other body data?"
                .to_owned(),
        ),
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
        (("workout", "workouts"), "moves", NO_MEANS_STRAVA),
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
        (("record", "records"), "can move up to", NO_MEANS_BODY),
        &|facet| {
            let (count, from) = crate::body_activity::listed_elsewhere(journal, stream, facet);
            (count > 0).then(|| (count, from.into_iter().collect()))
        },
        input,
        output,
    )
}

type ListedCount<'a> = &'a dyn Fn(&str) -> Option<(usize, Vec<String>)>;

fn ask_facet(
    journal: &Path,
    ((one, many), moves_verb, no_means): ((&str, &str), &str, &str),
    listed: ListedCount<'_>,
    input: &mut dyn BufRead,
    output: &mut dyn Write,
) -> (ImportFacet, Option<usize>) {
    let Ok((ask, _)) = body_facet_ask(journal) else {
        return (ImportFacet::Unattended, None);
    };
    let Some(text) = question(&ask) else {
        return (ImportFacet::Unattended, None);
    };
    let moves = listed(would_file_under(&ask));
    let shown = moves.as_ref().map_or(0, |(count, _)| *count);
    let _ = writeln!(output, "{no_means}");
    if let Some((count, from)) = &moves {
        let _ = writeln!(
            output,
            "a yes {} {count} {} already listed on the activities page, from {}.",
            moves_verb,
            if *count == 1 { one } else { many },
            from.join(", ")
        );
    }
    let _ = write!(output, "{text} [y/n] ");
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
            let _ = writeln!(output, "your answer couldn't be saved: {error}");
            return (ImportFacet::Unattended, None);
        }
    };
    if let Err(error) = record_body_facet(journal, &choice) {
        let _ = writeln!(output, "your answer couldn't be saved: {error}");
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
