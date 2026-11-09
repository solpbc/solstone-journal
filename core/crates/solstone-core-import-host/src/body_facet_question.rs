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

const NO_MEANS: &str = "if you say no, what this adds isn't listed with your activities, no people or things come from it, and agents limited to some facets won't see it. search and the body app still show it.";

fn question(ask: &BodyFacetAsk) -> Option<String> {
    match ask {
        BodyFacetAsk::Chosen(_) | BodyFacetAsk::Declined => None,
        BodyFacetAsk::AskExisting(facet) => Some(format!(
            "file what this adds under the facet you named {facet}?"
        )),
        BodyFacetAsk::AskMakeBody => Some("make a body facet for what this adds?".to_owned()),
        BodyFacetAsk::AskUnmute => {
            Some("your body facet is muted. turn it back on for what this adds?".to_owned())
        }
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
) -> ImportFacet {
    let Ok((ask, _)) = body_facet_ask(journal) else {
        return ImportFacet::Unattended;
    };
    let Some(text) = question(&ask) else {
        return ImportFacet::Unattended;
    };
    let moves =
        crate::strava_convert::listed_moves(journal, &download_ids(media), would_file_under(&ask));
    let _ = writeln!(output, "{NO_MEANS}");
    if let Some((count, from)) = &moves {
        let _ = writeln!(
            output,
            "a yes moves {count} {} already listed with your activities, from {}.",
            if *count == 1 { "workout" } else { "workouts" },
            from.join(", ")
        );
    }
    let _ = write!(output, "{text} [y/n] ");
    let _ = output.flush();
    let mut answer = String::new();
    if input.read_line(&mut answer).is_err() {
        return ImportFacet::Unattended;
    }
    let yes = match answer.trim().to_ascii_lowercase().as_str() {
        "y" | "yes" => true,
        "n" | "no" => false,
        _ => return ImportFacet::Unattended,
    };
    let choice = match apply_body_facet_answer(journal, &ask, yes) {
        Ok(Some(choice)) => choice,
        Ok(None) => return ImportFacet::Unattended,
        Err(error) => {
            let _ = writeln!(output, "your answer couldn't be saved: {error}");
            return ImportFacet::Unattended;
        }
    };
    if let Err(error) = record_body_facet(journal, &choice) {
        let _ = writeln!(output, "your answer couldn't be saved: {error}");
        return ImportFacet::Unattended;
    }
    match choice {
        solstone_core_journal_config::BodyFacetChoice::Facet(facet) => ImportFacet::Facet(facet),
        solstone_core_journal_config::BodyFacetChoice::None => ImportFacet::Unattended,
    }
}

#[cfg(test)]
#[path = "body_facet_question_tests.rs"]
mod tests;
