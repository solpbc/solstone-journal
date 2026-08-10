// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![allow(clippy::disallowed_methods, clippy::disallowed_types)]

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use serde_json::{Value, json};

use solstone_core_entity_matching::{EntityNameCandidate, MatchTier, find_matching_entity};

use crate::resolution::collect_low_confidence_candidates;
use crate::{
    AmbiguityChoiceEntity, AmbiguityChoiceRequest, EntityResolutionEntity, EntityResolutionError,
    EntityResolutionOutcome, hold_entity_trust_lock, record_ambiguity_choice,
    record_entity_resolution, record_entity_resolution_from_name_evidence,
};

static NEXT_TEMP_DIR: AtomicU64 = AtomicU64::new(0);

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new() -> Self {
        let sequence = NEXT_TEMP_DIR.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "solstone-core-entity-resolution-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir_all(&path).unwrap();
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

#[test]
fn corrupt_store_rows_fail_before_the_empty_entities_check() {
    let temporary = TempDir::new();
    let ambiguities = temporary.path().join("entities/ambiguities.jsonl");
    fs::create_dir_all(ambiguities.parent().unwrap()).unwrap();
    fs::write(&ambiguities, "{not valid json}\n").unwrap();

    let result = resolve(
        temporary.path(),
        "Sarah",
        &[],
        journal_scope(),
        origin("first"),
        false,
    );

    assert!(matches!(result, Err(EntityResolutionError::Read(_))));
}

#[test]
fn resolved_choices_fail_loudly_when_absent_or_blocked() {
    let absent_root = TempDir::new();
    let available = vec![entity(Some("sarah_lee"), "Sarah Lee", false)];
    choose_sarah_lee(absent_root.path(), &available);
    let absent = resolve(
        absent_root.path(),
        "Sarah",
        &[entity(Some("sarah_connor"), "Sarah Connor", false)],
        journal_scope(),
        origin("absent"),
        false,
    );
    assert!(matches!(
        absent,
        Err(EntityResolutionError::ResolvedChoiceEntityAbsent { entity_id, .. })
            if entity_id == "sarah_lee"
    ));

    let blocked_root = TempDir::new();
    choose_sarah_lee(blocked_root.path(), &available);
    let blocked = resolve(
        blocked_root.path(),
        "Sarah",
        &[entity(Some("sarah_lee"), "Sarah Lee", true)],
        journal_scope(),
        origin("blocked"),
        false,
    );
    assert!(matches!(
        blocked,
        Err(EntityResolutionError::ResolvedChoiceEntityBlocked { entity_id, .. })
            if entity_id == "sarah_lee"
    ));
}

#[test]
fn high_confidence_tiers_resolve_without_creating_ambiguities() {
    let cases = [
        (
            "Sarah Lee",
            entity(Some("sarah_lee"), "Sarah Lee", false),
            MatchTier::Exact,
        ),
        (
            "SARAH LEE",
            entity(Some("sarah_lee"), "Sarah Lee", false),
            MatchTier::CaseInsensitive,
        ),
        (
            "sarah@example.com",
            entity_with_email("sarah_lee", "Sarah Lee", "sarah@example.com"),
            MatchTier::Email,
        ),
        (
            "sarah-lee",
            entity(None, "Sarah Lee", false),
            MatchTier::Slug,
        ),
    ];

    for (index, (query, candidate, tier)) in cases.into_iter().enumerate() {
        let temporary = TempDir::new();
        let result = resolve(
            temporary.path(),
            query,
            &[candidate],
            journal_scope(),
            origin(&format!("high-{index}")),
            false,
        )
        .unwrap();
        assert_eq!(result.outcome, EntityResolutionOutcome::Resolved);
        assert_eq!(result.entity_index, Some(0));
        assert_eq!(result.tier, Some(tier));
        assert!(!ambiguities_path(temporary.path()).exists());
    }
}

#[test]
fn name_evidence_resolution_ignores_written_id_slug_collisions() {
    let temporary = TempDir::new();
    let entities = [entity(Some("new_person"), "Someone Else", false)];

    let legacy = resolve(
        temporary.path(),
        "New Person",
        &entities,
        journal_scope(),
        origin("legacy"),
        false,
    )
    .unwrap();
    assert_eq!(legacy.outcome, EntityResolutionOutcome::Resolved);
    assert_eq!(legacy.tier, Some(MatchTier::Slug));

    let name_evidence = record_entity_resolution_from_name_evidence(
        temporary.path(),
        "New Person",
        &entities,
        journal_scope(),
        origin("name-evidence"),
        90.0,
        false,
    )
    .unwrap();
    assert_eq!(name_evidence.outcome, EntityResolutionOutcome::NoMatch);
}

#[test]
fn low_confidence_tiers_are_ambiguous_without_matcher_uniqueness_guards() {
    let cases = [
        (
            "Sarah",
            vec![
                entity(Some("sarah_connor"), "Sarah Connor", false),
                entity(Some("sarah_lee"), "Sarah Lee", false),
            ],
            MatchTier::FirstWord,
            true,
            2,
        ),
        (
            "Jones Dilworth",
            vec![
                entity(Some("josh"), "Josh Jones Dilworth", false),
                entity(Some("mary"), "Mary Jones Dilworth", false),
            ],
            MatchTier::TokenSubset,
            true,
            2,
        ),
        (
            "Jona Dilt",
            vec![
                entity(Some("jonathan"), "Jonathan Dilton", false),
                entity(Some("jonas"), "Jonas Diltmore", false),
            ],
            MatchTier::Prefix,
            true,
            2,
        ),
        (
            "Robert Jonson",
            vec![entity(Some("robert-johnson"), "Robert Johnson", false)],
            MatchTier::Fuzzy,
            false,
            1,
        ),
    ];

    for (index, (query, entities, tier, matcher_returns_none, expected_candidates)) in
        cases.into_iter().enumerate()
    {
        let temporary = TempDir::new();
        let matcher_candidates = adapt(&entities);
        if matcher_returns_none {
            assert_eq!(find_matching_entity(query, &matcher_candidates, 90.0), None);
        }

        let result = resolve(
            temporary.path(),
            query,
            &entities,
            journal_scope(),
            origin(&format!("low-{index}")),
            false,
        )
        .unwrap();
        assert_eq!(result.outcome, EntityResolutionOutcome::Ambiguous);
        assert_eq!(result.tier, Some(tier));
        assert_eq!(result.candidates.len(), expected_candidates);
        assert!(result.ambiguity_id.is_some_and(|id| !id.is_empty()));
    }
}

#[test]
fn low_confidence_first_word_uses_unified_unicode_normalization() {
    let entities = vec![
        entity(Some("strasse_atlas"), "STRASSE Atlas", false),
        entity(Some("strasse_beacon"), "STRASSE Beacon", false),
    ];
    let query = "Straße";

    assert_eq!(find_matching_entity(query, &adapt(&entities), 90.0), None);

    let temporary = TempDir::new();
    let result = resolve(
        temporary.path(),
        query,
        &entities,
        journal_scope(),
        origin("unicode-first-word"),
        false,
    )
    .unwrap();

    assert_eq!(result.outcome, EntityResolutionOutcome::Ambiguous);
    assert_eq!(result.tier, Some(MatchTier::FirstWord));
    let candidate_ids: Vec<_> = result
        .candidates
        .iter()
        .map(|candidate| candidate.id.as_str())
        .collect();
    assert_eq!(candidate_ids.len(), 2);
    assert!(candidate_ids.contains(&"strasse_atlas"));
    assert!(candidate_ids.contains(&"strasse_beacon"));
}

#[test]
fn idless_low_confidence_candidates_are_dropped_after_collection() {
    let entities = vec![
        entity(None, "Sarah Connor", false),
        entity(Some("sarah_lee"), "Sarah Lee", false),
    ];
    assert_eq!(find_matching_entity("Sarah", &adapt(&entities), 90.0), None);
    let temporary = TempDir::new();
    let result = resolve(
        temporary.path(),
        "Sarah",
        &entities,
        journal_scope(),
        origin("idless-with-id"),
        false,
    )
    .unwrap();
    assert_eq!(result.outcome, EntityResolutionOutcome::Ambiguous);
    assert_eq!(result.candidates.len(), 1);
    assert_eq!(result.candidates[0].id, "sarah_lee");

    let idless_only = vec![entity(None, "Sarah Connor", false)];
    let no_match = resolve(
        temporary.path(),
        "Sarah",
        &idless_only,
        journal_scope(),
        origin("idless-only"),
        false,
    )
    .unwrap();
    assert_eq!(no_match.outcome, EntityResolutionOutcome::NoMatch);
}

#[test]
fn read_only_ambiguity_does_not_create_locks_or_store_rows() {
    let temporary = TempDir::new();
    let result = resolve(
        temporary.path(),
        "Sarah",
        &[
            entity(Some("sarah_connor"), "Sarah Connor", false),
            entity(Some("sarah_lee"), "Sarah Lee", false),
        ],
        journal_scope(),
        origin("read-only"),
        true,
    )
    .unwrap();

    assert_eq!(result.outcome, EntityResolutionOutcome::Ambiguous);
    assert_eq!(result.ambiguity_id.as_deref(), Some(""));
    assert!(!ambiguities_path(temporary.path()).exists());
    assert!(
        !temporary
            .path()
            .join("health/locks/entity-trust.lock")
            .exists()
    );
}

#[test]
fn mutation_resolution_waits_for_the_outermost_trust_guard() {
    let temporary = TempDir::new();
    let outer = hold_entity_trust_lock(temporary.path()).unwrap();
    let root = temporary.path().to_path_buf();
    let (started_tx, started_rx) = mpsc::channel();
    let (finished_tx, finished_rx) = mpsc::channel();

    let worker = thread::spawn(move || {
        started_tx.send(()).unwrap();
        let result = resolve(
            &root,
            "Sarah",
            &[
                entity(Some("sarah_connor"), "Sarah Connor", false),
                entity(Some("sarah_lee"), "Sarah Lee", false),
            ],
            journal_scope(),
            origin("lock-worker"),
            false,
        );
        finished_tx.send(result).unwrap();
    });

    started_rx.recv().unwrap();
    assert!(
        finished_rx
            .recv_timeout(Duration::from_millis(100))
            .is_err(),
        "resolution completed before the trust guard dropped"
    );
    drop(outer);
    // Liveness, not latency: the claim is that dropping the guard lets the
    // worker through, and the 100ms bound above is what proves the guard held
    // it. This ceiling only has to outlast a loaded machine, and one second
    // does not -- `make ci` runs a whole nested `make ci` alongside this suite.
    let result = finished_rx
        .recv_timeout(Duration::from_secs(60))
        .unwrap()
        .unwrap();
    assert_eq!(result.outcome, EntityResolutionOutcome::Ambiguous);
    worker.join().unwrap();
}

#[test]
fn ranking_deduplicates_by_first_id_and_orders_by_score_name_and_id() {
    let entities = vec![
        entity(Some("shared"), "Sarah Zed", false),
        entity(Some("shared"), "Sarah", false),
        entity(Some("amy"), "Sarah Amy", false),
        entity(Some("zoe"), "Sarah Zoe", false),
    ];
    let (tier, candidates) = collect_low_confidence_candidates("Sarah", &entities, 90.0);

    assert_eq!(tier, Some(MatchTier::FirstWord));
    assert_eq!(
        candidates
            .iter()
            .find(|candidate| candidate.id == "shared")
            .map(|candidate| candidate.name.as_str()),
        Some("Sarah Zed")
    );
    assert_eq!(candidates.len(), 3);
    assert!(candidates.windows(2).all(|pair| {
        pair[0].score > pair[1].score
            || (pair[0].score == pair[1].score
                && (&pair[0].name, &pair[0].id) <= (&pair[1].name, &pair[1].id))
    }));
}

#[test]
fn repeated_observations_delegate_store_updates_and_origin_deduplication() {
    let temporary = TempDir::new();
    let first = resolve(
        temporary.path(),
        "Sarah",
        &[
            entity(Some("sarah_connor"), "Sarah Connor", false),
            entity(Some("sarah_lee"), "Sarah Lee", false),
        ],
        journal_scope(),
        origin("first"),
        false,
    )
    .unwrap();
    let first_row = single_ambiguity_row(temporary.path());

    let second = resolve(
        temporary.path(),
        " SARAH ",
        &[
            entity(Some("sarah_connor"), "Sarah Connor", false),
            entity(Some("sarah_brown"), "Sarah Brown", false),
        ],
        journal_scope(),
        origin("second"),
        false,
    )
    .unwrap();
    let second_row = single_ambiguity_row(temporary.path());

    assert_eq!(first.ambiguity_id, second.ambiguity_id);
    assert_eq!(second_row["first_seen"], first_row["first_seen"]);
    assert!(second_row["last_seen"].as_str().unwrap() >= first_row["last_seen"].as_str().unwrap());
    assert_eq!(second_row["latest_query"], " SARAH ");
    assert_eq!(second_row["observed_tier"], 5);
    assert_eq!(second_row["occurrence_count"], 2);
    assert_eq!(second_row["origins"].as_array().map(Vec::len), Some(2));
    let ids: Vec<_> = second_row["ranked_candidates"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|candidate| candidate["id"].as_str())
        .collect();
    assert!(ids.contains(&"sarah_brown"));

    let third = resolve(
        temporary.path(),
        "Sarah",
        &[
            entity(Some("sarah_connor"), "Sarah Connor", false),
            entity(Some("sarah_brown"), "Sarah Brown", false),
        ],
        journal_scope(),
        origin("second"),
        false,
    )
    .unwrap();
    assert_eq!(third.ambiguity_id, first.ambiguity_id);
    let third_row = single_ambiguity_row(temporary.path());
    assert_eq!(third_row["occurrence_count"], 2);
    assert_eq!(third_row["origins"].as_array().map(Vec::len), Some(2));
}

#[test]
fn true_no_match_does_not_create_an_ambiguity_row() {
    let temporary = TempDir::new();
    let result = resolve(
        temporary.path(),
        "unrelated query",
        &[entity(Some("sarah_lee"), "Sarah Lee", false)],
        journal_scope(),
        origin("none"),
        false,
    )
    .unwrap();

    assert_eq!(result.outcome, EntityResolutionOutcome::NoMatch);
    assert!(!ambiguities_path(temporary.path()).exists());
}

#[test]
fn scope_discriminator_creates_separate_ambiguity_rows() {
    let temporary = TempDir::new();
    let entities = [
        entity(Some("sarah_connor"), "Sarah Connor", false),
        entity(Some("sarah_lee"), "Sarah Lee", false),
    ];

    let journal = resolve(
        temporary.path(),
        "Sarah",
        &entities,
        journal_scope(),
        origin("journal"),
        false,
    )
    .unwrap();
    let facet = resolve(
        temporary.path(),
        "Sarah",
        &entities,
        json!({"kind": "facet", "facet": "work"}),
        origin("facet"),
        false,
    )
    .unwrap();

    assert_ne!(journal.ambiguity_id, facet.ambiguity_id);
    assert_eq!(ambiguity_rows(temporary.path()).len(), 2);
}

fn resolve(
    root: &Path,
    query: &str,
    entities: &[EntityResolutionEntity],
    scope: Value,
    origin: Value,
    read_only: bool,
) -> Result<crate::EntityResolution, EntityResolutionError> {
    record_entity_resolution(root, query, entities, scope, origin, 90.0, read_only)
}

fn choose_sarah_lee(root: &Path, entities: &[EntityResolutionEntity]) {
    let initial = resolve(
        root,
        "Sarah",
        entities,
        journal_scope(),
        origin("observation"),
        false,
    )
    .unwrap();
    assert_eq!(initial.outcome, EntityResolutionOutcome::Ambiguous);
    let eligible: Vec<_> = entities
        .iter()
        .filter_map(|entity| {
            entity.id.as_ref().map(|id| AmbiguityChoiceEntity {
                id: id.clone(),
                blocked: entity.blocked,
            })
        })
        .collect();
    record_ambiguity_choice(
        root,
        &AmbiguityChoiceRequest {
            scope: journal_scope(),
            query: "Sarah".to_owned(),
            entity_id: "sarah_lee".to_owned(),
            origin: None,
        },
        &eligible,
    )
    .unwrap();
}

fn entity(id: Option<&str>, name: &str, blocked: bool) -> EntityResolutionEntity {
    EntityResolutionEntity {
        id: id.map(str::to_owned),
        name: name.to_owned(),
        aka: Vec::new(),
        emails: Vec::new(),
        blocked,
    }
}

fn entity_with_email(id: &str, name: &str, email: &str) -> EntityResolutionEntity {
    EntityResolutionEntity {
        id: Some(id.to_owned()),
        name: name.to_owned(),
        aka: Vec::new(),
        emails: vec![email.to_owned()],
        blocked: false,
    }
}

fn adapt(entities: &[EntityResolutionEntity]) -> Vec<EntityNameCandidate> {
    entities
        .iter()
        .map(|entity| EntityNameCandidate {
            id: entity.id.clone(),
            name: entity.name.clone(),
            aka: entity.aka.clone(),
            emails: entity.emails.clone(),
        })
        .collect()
}

fn journal_scope() -> Value {
    json!({"kind": "journal"})
}

fn origin(name: &str) -> Value {
    json!({"lane": "resolution-tests", "name": name})
}

fn ambiguities_path(root: &Path) -> PathBuf {
    root.join("entities/ambiguities.jsonl")
}

fn ambiguity_rows(root: &Path) -> Vec<Value> {
    fs::read_to_string(ambiguities_path(root))
        .unwrap()
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn single_ambiguity_row(root: &Path) -> Value {
    let mut rows = ambiguity_rows(root);
    assert_eq!(rows.len(), 1);
    rows.pop().unwrap()
}
