// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Batch and pairwise helpers built on the canonical entity matcher.

use std::collections::BTreeMap;

use crate::matcher::{
    EntityNameCandidate, EntityNameMatch, find_matching_entity, prefix_token_match,
    token_subset_match,
};
use crate::normalize::normalize_resolution_query;

/// Resolve every non-empty query through the canonical tiered matcher.
///
/// The result retains [`EntityNameMatch`] so callers receive both the candidate
/// position and the confidence tier rather than a bare entity-ID mapping.
pub fn build_name_resolution_map(
    queries: &[String],
    candidates: &[EntityNameCandidate],
    fuzzy_threshold: f64,
) -> BTreeMap<String, EntityNameMatch> {
    queries
        .iter()
        .filter(|query| !query.is_empty())
        .filter_map(|query| {
            find_matching_entity(query, candidates, fuzzy_threshold)
                .map(|entity_match| (query.clone(), entity_match))
        })
        .collect()
}

/// Find the first candidate carrying an email equal under plain lowercase comparison.
pub fn find_entity_by_email(email: &str, candidates: &[EntityNameCandidate]) -> Option<usize> {
    if email.is_empty() {
        return None;
    }

    // Email comparisons intentionally remain plain-lowercased rather than using
    // resolution normalization or full case folding.
    let email_lower = email.to_lowercase();
    candidates.iter().position(|candidate| {
        candidate
            .emails
            .iter()
            .any(|candidate_email| candidate_email.to_lowercase() == email_lower)
    })
}

/// Return whether two names are plausible variants under the shared matching rules.
pub fn is_name_variant_match(name_a: &str, name_b: &str) -> bool {
    let normalized_a = normalize_resolution_query(name_a);
    let normalized_b = normalize_resolution_query(name_b);
    if normalized_a.is_empty() || normalized_b.is_empty() {
        return false;
    }

    let first_a = normalized_a
        .split_whitespace()
        .next()
        .expect("non-empty normalized name has a first token");
    let first_b = normalized_b
        .split_whitespace()
        .next()
        .expect("non-empty normalized name has a first token");
    normalized_a == first_b
        || normalized_b == first_a
        || token_subset_match(&normalized_a, &normalized_b)
        || prefix_token_match(&normalized_a, &normalized_b)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use serde::Deserialize;

    use super::{build_name_resolution_map, find_entity_by_email, is_name_variant_match};
    use crate::{EntityNameCandidate, MatchTier};

    const ENTITY_RESOLUTION_MAP_DIVERGENCES_FIXTURE: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/entity_resolution_map_divergences.json"
    ));

    #[derive(Debug, Deserialize)]
    struct ResolutionMapFixture {
        fuzzy_threshold: f64,
        vector_count: usize,
        counts: BTreeMap<String, usize>,
        entries: Vec<ResolutionMapFixtureEntry>,
    }

    #[derive(Debug, Deserialize)]
    struct ResolutionMapFixtureEntry {
        query: String,
        candidates: Vec<ResolutionMapFixtureCandidate>,
        new_door: ResolutionMapFixtureDoor,
    }

    #[derive(Debug, Deserialize)]
    struct ResolutionMapFixtureCandidate {
        id: Option<String>,
        name: String,
        #[serde(default)]
        aka: Vec<String>,
        #[serde(default)]
        emails: Vec<String>,
    }

    #[derive(Debug, Deserialize)]
    struct ResolutionMapFixtureDoor {
        outcome: String,
        tier: Option<u8>,
        entity_id: Option<String>,
    }

    fn candidate(
        id: Option<&str>,
        name: &str,
        aka: &[&str],
        emails: &[&str],
    ) -> EntityNameCandidate {
        EntityNameCandidate {
            id: id.map(str::to_owned),
            name: name.to_owned(),
            aka: aka.iter().map(|value| (*value).to_owned()).collect(),
            emails: emails.iter().map(|value| (*value).to_owned()).collect(),
        }
    }

    #[test]
    fn batch_rejects_multi_token_first_word_only_match() {
        let candidates = [candidate(Some("person_a"), "Person A", &[], &[])];
        let queries = ["Person B".to_owned()];

        let matches = build_name_resolution_map(&queries, &candidates, 90.0);

        assert!(matches.is_empty(), "Person B must not resolve to Person A");
    }

    #[test]
    fn batch_keeps_valid_single_token_exact_match() {
        let candidates = [candidate(Some("person_a"), "Person", &[], &[])];
        let queries = ["Person".to_owned()];

        let entity_match = build_name_resolution_map(&queries, &candidates, 90.0)
            .get("Person")
            .copied()
            .expect("single-token entity should still resolve");

        assert_eq!(entity_match.candidate_index, 0);
        assert_eq!(entity_match.tier, MatchTier::Exact);
    }

    #[test]
    fn batch_preserves_exact_id_over_lowered_name_collision() {
        let candidates = [
            candidate(Some("alice"), "Alpha", &[], &[]),
            candidate(Some("bee"), "ALICE", &[], &[]),
        ];
        let queries = ["alice".to_owned()];

        let entity_match = build_name_resolution_map(&queries, &candidates, 90.0)
            .get("alice")
            .copied()
            .expect("exact ID should resolve");

        assert_eq!(entity_match.candidate_index, 0);
        assert_eq!(entity_match.tier, MatchTier::Exact);
    }

    #[test]
    fn batch_keeps_idless_slug_label_matching() {
        let candidates = [candidate(None, "Alice Chen", &[], &[])];
        let queries = ["alice_chen".to_owned()];

        let entity_match = build_name_resolution_map(&queries, &candidates, 90.0)
            .get("alice_chen")
            .copied()
            .expect("id-less name slug should resolve");

        assert_eq!(entity_match.candidate_index, 0);
        assert_eq!(entity_match.tier, MatchTier::Slug);
    }

    #[test]
    fn batch_retains_match_tier() {
        let candidates = [candidate(Some("robert"), "Robert", &[], &[])];
        let queries = ["Robert".to_owned()];

        let entity_match = build_name_resolution_map(&queries, &candidates, 90.0)
            .get("Robert")
            .copied()
            .expect("exact name should resolve");

        assert_eq!(entity_match.tier, MatchTier::Exact);
    }

    #[test]
    fn batch_resolves_email_at_email_tier() {
        let candidates = [candidate(Some("bob"), "Robert", &[], &["bob@example.com"])];
        let queries = ["BOB@EXAMPLE.COM".to_owned()];

        let entity_match = build_name_resolution_map(&queries, &candidates, 90.0)
            .get("BOB@EXAMPLE.COM")
            .copied()
            .expect("email should resolve");

        assert_eq!(entity_match.candidate_index, 0);
        assert_eq!(entity_match.tier, MatchTier::Email);
    }

    #[test]
    fn email_lookup_matches_case_insensitively_and_returns_first_candidate() {
        let candidates = [
            candidate(Some("first"), "First", &[], &["bob@example.com"]),
            candidate(Some("second"), "Second", &[], &["BOB@example.com"]),
        ];

        assert_eq!(
            find_entity_by_email("Bob@Example.Com", &candidates),
            Some(0)
        );
        assert_eq!(
            find_entity_by_email("missing@example.com", &candidates),
            None
        );
    }

    #[test]
    fn email_lookup_does_not_apply_full_case_folding() {
        let candidates = [candidate(
            Some("strasse"),
            "Strasse",
            &[],
            &["straße@example.com"],
        )];

        assert_eq!(
            find_entity_by_email("STRASSE@example.com", &candidates),
            None
        );
    }

    #[test]
    fn name_variant_matching_uses_full_unicode_case_folding() {
        assert!(is_name_variant_match("Straße", "STRASSE"));
    }

    #[test]
    fn name_variant_matching_accepts_first_word_and_token_subset() {
        assert!(is_name_variant_match("John Smith", "John"));
        assert!(is_name_variant_match(
            "Jones Dilworth",
            "Josh Jones Dilworth"
        ));
    }

    #[test]
    fn name_variant_matching_rejects_unrelated_names() {
        assert!(!is_name_variant_match("Alice Smith", "Bob Jones"));
    }

    #[test]
    fn batch_resolution_map_matches_unified_door_fixture() {
        let fixture: ResolutionMapFixture =
            serde_json::from_str(ENTITY_RESOLUTION_MAP_DIVERGENCES_FIXTURE)
                .expect("parse resolution-map divergence fixture");
        assert_eq!(fixture.entries.len(), fixture.vector_count);
        assert_eq!(fixture.counts.get("total"), Some(&fixture.vector_count));

        for entry in fixture.entries {
            let candidates: Vec<EntityNameCandidate> = entry
                .candidates
                .into_iter()
                .map(|candidate| EntityNameCandidate {
                    id: candidate.id,
                    name: candidate.name,
                    aka: candidate.aka,
                    emails: candidate.emails,
                })
                .collect();
            let queries = [entry.query.clone()];
            let actual = build_name_resolution_map(&queries, &candidates, fixture.fuzzy_threshold)
                .get(&entry.query)
                .copied();

            match entry.new_door.outcome.as_str() {
                "no_match" => assert_eq!(actual, None, "{:?}", entry.query),
                "resolved" => {
                    let actual = actual.expect("resolved fixture entry matches a candidate");
                    assert_eq!(
                        actual.tier as u8,
                        entry
                            .new_door
                            .tier
                            .expect("resolved fixture entry carries a tier"),
                        "{:?}",
                        entry.query
                    );
                    assert_eq!(
                        candidates[actual.candidate_index].id, entry.new_door.entity_id,
                        "{:?}",
                        entry.query
                    );
                }
                outcome => panic!("unknown fixture outcome: {outcome:?}"),
            }
        }
    }
}
