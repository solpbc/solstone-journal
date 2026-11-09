// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;

use serde_json::json;

use super::*;

fn journal(facets: &[&str]) -> tempfile::TempDir {
    let journal = tempfile::tempdir().unwrap();
    let root = journal.path();
    fs::create_dir_all(root.join("config")).unwrap();
    fs::write(
        root.join("config/journal.json"),
        json!({"identity": {"timezone": "UTC"}}).to_string(),
    )
    .unwrap();
    for facet in facets {
        solstone_core_facets::create_facet(root, facet, facet, "", "", "", None).unwrap();
    }
    journal
}

fn ask(root: &Path, answer: &str) -> (ImportFacet, String) {
    let mut output = Vec::new();
    let facet = ask_strava_facet(
        root,
        &root.join("no-download.csv"),
        &mut answer.as_bytes(),
        &mut output,
    );
    (facet.0, String::from_utf8(output).unwrap())
}

#[test]
fn a_yes_says_how_many_it_was_shown_and_a_no_answer_shows_none() {
    let journal = tempfile::tempdir().unwrap();
    let root = journal.path();
    fs::create_dir_all(root.join("config")).unwrap();
    fs::write(root.join("config/journal.json"), "{}").unwrap();
    let mut output = Vec::new();
    let (_, shown) = ask_strava_facet(
        root,
        &root.join("no-download.csv"),
        &mut b"y\n".as_slice(),
        &mut output,
    );
    assert_eq!(shown, Some(0));
    let mut output = Vec::new();
    let (_, shown) = ask_strava_facet(
        root,
        &root.join("no-download.csv"),
        &mut b"".as_slice(),
        &mut output,
    );
    assert_eq!(shown, None);
}

#[test]
fn a_yes_makes_the_facet_remembers_it_and_files_this_import_under_it() {
    let journal = journal(&["work"]);
    let root = journal.path();
    let (facet, said) = ask(root, "y\n");
    assert_eq!(facet, ImportFacet::Facet("body".to_owned()));
    assert!(said.contains("make a body facet"), "{said}");
    assert!(said.contains("if you say no"), "{said}");
    assert!(root.join("facets/body/facet.json").is_file());
    assert_eq!(
        solstone_core_facets::body_facet_ask(root).unwrap().0,
        BodyFacetAsk::Chosen("body".to_owned())
    );
}

#[test]
fn a_yes_on_a_muted_body_facet_turns_it_back_on() {
    let journal = journal(&["body", "work"]);
    let root = journal.path();
    solstone_core_facets::set_facet_muted(root, "body", true).unwrap();
    let (facet, said) = ask(root, "yes\n");
    assert!(said.contains("muted"), "{said}");
    assert_eq!(facet, ImportFacet::Facet("body".to_owned()));
    let inventory = solstone_core_facets::observe_declared_facet_inventory(root).unwrap();
    assert!(inventory.enabled.iter().any(|facet| facet == "body"));
}

#[test]
fn a_no_is_remembered_and_the_run_goes_on_with_no_facet() {
    let journal = journal(&["work"]);
    let root = journal.path();
    let (facet, _) = ask(root, "no\n");
    assert_eq!(facet, ImportFacet::Unattended);
    assert_eq!(
        solstone_core_facets::body_facet_ask(root).unwrap().0,
        BodyFacetAsk::Declined
    );
    assert!(!root.join("facets/body").exists());
}

#[test]
fn anything_else_or_end_of_input_is_no_answer_and_nothing_is_written() {
    for answer in ["maybe\n", ""] {
        let journal = journal(&["work"]);
        let root = journal.path();
        let (facet, _) = ask(root, answer);
        assert_eq!(facet, ImportFacet::Unattended, "{answer:?}");
        assert_eq!(
            solstone_core_facets::body_facet_ask(root).unwrap().0,
            BodyFacetAsk::AskMakeBody,
            "{answer:?}"
        );
    }
}

#[test]
fn a_remembered_answer_means_no_question() {
    let journal = journal(&["body"]);
    let root = journal.path();
    record_body_facet(
        root,
        &solstone_core_journal_config::BodyFacetChoice::Facet("body".to_owned()),
    )
    .unwrap();
    let (facet, said) = ask(root, "y\n");
    assert_eq!(facet, ImportFacet::Unattended);
    assert!(said.is_empty(), "{said}");
}

#[test]
fn a_named_existing_facet_is_the_one_a_yes_files_under() {
    let journal = journal(&["health"]);
    let root = journal.path();
    let (facet, said) = ask(root, "y\n");
    assert_eq!(facet, ImportFacet::Facet("health".to_owned()));
    assert!(said.contains("health"), "{said}");
}
