// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![allow(clippy::disallowed_methods, clippy::disallowed_types)]

use std::fs;
use std::path::Path;

use solstone_core_journal_config::{BodyFacetChoice, BodyFacetOffer, get_journal_config_path};
use solstone_core_journal_config_write::record_body_facet;

use crate::{
    body_facet_offer, create_facet, default_for_unattended, delete_facet, set_facet_muted,
};

fn make_facet(journal: &Path, facet: &str) {
    create_facet(journal, facet, facet, "Description", "blue", "💼", None).unwrap();
}

fn journal_with(facets: &[&str]) -> tempfile::TempDir {
    let temporary = tempfile::tempdir().unwrap();
    for facet in facets {
        make_facet(temporary.path(), facet);
    }
    temporary
}

fn offer(journal: &Path) -> BodyFacetOffer {
    body_facet_offer(journal).unwrap().offer
}

fn remember(journal: &Path, choice: BodyFacetChoice) {
    record_body_facet(journal, &choice).unwrap();
}

fn facet(name: &str) -> BodyFacetChoice {
    BodyFacetChoice::Facet(name.to_owned())
}

fn write_config(journal: &Path, bytes: &[u8]) {
    let path = get_journal_config_path(journal);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, bytes).unwrap();
}

#[test]
fn never_answered_offers_body_before_health() {
    let journal = journal_with(&["health", "body", "work"]);
    assert_eq!(
        offer(journal.path()),
        BodyFacetOffer::AskExisting("body".to_owned())
    );
    assert_eq!(default_for_unattended(journal.path()).unwrap(), None);
}

#[test]
fn never_answered_offers_health_when_there_is_no_body() {
    let journal = journal_with(&["health", "work"]);
    assert_eq!(
        offer(journal.path()),
        BodyFacetOffer::AskExisting("health".to_owned())
    );
}

#[test]
fn never_answered_without_body_or_health_offers_to_make_body() {
    let journal = journal_with(&["work"]);
    assert_eq!(offer(journal.path()), BodyFacetOffer::AskMakeBody);
    assert_eq!(default_for_unattended(journal.path()).unwrap(), None);
}

#[test]
fn a_muted_body_is_not_offered() {
    let journal = journal_with(&["body", "health", "work"]);
    set_facet_muted(journal.path(), "body", true).unwrap();
    assert_eq!(
        offer(journal.path()),
        BodyFacetOffer::AskExisting("health".to_owned())
    );
}

#[test]
fn a_remembered_enabled_facet_is_chosen_and_used_unattended() {
    let journal = journal_with(&["body", "work"]);
    remember(journal.path(), facet("work"));
    assert_eq!(
        offer(journal.path()),
        BodyFacetOffer::Chosen("work".to_owned())
    );
    assert_eq!(
        default_for_unattended(journal.path()).unwrap(),
        Some("work".to_owned())
    );
}

#[test]
fn a_remembered_facet_that_is_muted_asks_again() {
    let journal = journal_with(&["body", "running", "work"]);
    remember(journal.path(), facet("running"));
    set_facet_muted(journal.path(), "running", true).unwrap();
    assert_eq!(
        offer(journal.path()),
        BodyFacetOffer::AskExisting("body".to_owned())
    );
    assert_eq!(default_for_unattended(journal.path()).unwrap(), None);

    let journal = journal_with(&["running", "work"]);
    remember(journal.path(), facet("running"));
    set_facet_muted(journal.path(), "running", true).unwrap();
    assert_eq!(offer(journal.path()), BodyFacetOffer::AskMakeBody);
    assert_eq!(default_for_unattended(journal.path()).unwrap(), None);
}

#[test]
fn a_remembered_facet_that_is_deleted_asks_again() {
    let journal = journal_with(&["health", "running", "work"]);
    remember(journal.path(), facet("running"));
    assert!(delete_facet(journal.path(), "running").unwrap());
    assert_eq!(
        offer(journal.path()),
        BodyFacetOffer::AskExisting("health".to_owned())
    );
    assert_eq!(default_for_unattended(journal.path()).unwrap(), None);

    let journal = journal_with(&["running", "work"]);
    remember(journal.path(), facet("running"));
    assert!(delete_facet(journal.path(), "running").unwrap());
    assert_eq!(offer(journal.path()), BodyFacetOffer::AskMakeBody);
}

#[test]
fn declined_stays_declined_when_body_exists() {
    let journal = journal_with(&["work"]);
    remember(journal.path(), BodyFacetChoice::None);
    assert_eq!(offer(journal.path()), BodyFacetOffer::Declined);
    make_facet(journal.path(), "body");
    assert_eq!(offer(journal.path()), BodyFacetOffer::Declined);
    assert_eq!(default_for_unattended(journal.path()).unwrap(), None);
}

#[test]
fn a_made_body_facet_remembered_reads_back_as_chosen() {
    let journal = journal_with(&["work"]);
    assert_eq!(offer(journal.path()), BodyFacetOffer::AskMakeBody);
    make_facet(journal.path(), "body");
    remember(journal.path(), facet("body"));
    assert_eq!(
        offer(journal.path()),
        BodyFacetOffer::Chosen("body".to_owned())
    );
    assert_eq!(
        default_for_unattended(journal.path()).unwrap(),
        Some("body".to_owned())
    );
}

#[test]
fn a_malformed_value_reads_as_never_answered_is_reported_and_not_rewritten() {
    let journal = journal_with(&["body", "work"]);
    let bytes = br#"{"imports":{"body_facet":{"choice":"facet","facet":["work"]}}}"#;
    write_config(journal.path(), bytes);
    let read = body_facet_offer(journal.path()).unwrap();
    assert_eq!(read.offer, BodyFacetOffer::AskExisting("body".to_owned()));
    assert!(read.malformed.is_some());
    assert_eq!(default_for_unattended(journal.path()).unwrap(), None);
    assert_eq!(
        fs::read(get_journal_config_path(journal.path())).unwrap(),
        bytes
    );

    let journal = journal_with(&["work"]);
    write_config(journal.path(), br#"{"imports":{"body_facet":"none"}}"#);
    let read = body_facet_offer(journal.path()).unwrap();
    assert_eq!(read.offer, BodyFacetOffer::AskMakeBody);
    assert!(read.malformed.is_some());
}

#[test]
fn a_valid_value_is_not_reported() {
    let journal = journal_with(&["work"]);
    remember(journal.path(), BodyFacetChoice::None);
    assert_eq!(body_facet_offer(journal.path()).unwrap().malformed, None);
}

#[test]
fn reading_the_offer_never_writes_the_config() {
    let journal = journal_with(&["work"]);
    body_facet_offer(journal.path()).unwrap();
    default_for_unattended(journal.path()).unwrap();
    assert!(!get_journal_config_path(journal.path()).exists());
}
