// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Native tiered entity name matching mirrors
//! `solstone.think.entities.matching.find_matching_entity` for typed candidate
//! records. Python raises `IndexError` from `_first_word_key` at
//! `matching.py:131` for whitespace-only entity names during map building, and
//! at `matching.py:368` for all-whitespace queries in tier 5. Rust never panics
//! for whitespace-only input: a whitespace-only entity name creates no
//! first-word bucket, but still participates in exact/lower/id/email/fuzzy maps
//! and can remain matchable.
//! The typed `EntityNameCandidate` shape makes Python's malformed-input raise
//! paths unrepresentable; any future JSON adapter must accept only string field
//! values and string list members before constructing candidates.
//! Rust `rapidfuzz::fuzz::ratio` returns 0.0..=1.0, so tier 8 multiplies by
//! 100.0 at the scoring site to compare on Python's 0..=100 threshold scale.
//! Slug generation is scoped to the `entity_slug(name)` call path and does not
//! implement general slugify features: `allow_unicode`, custom `replacements`,
//! `stopwords`, or word-boundary truncation.

use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::collections::btree_map::Entry;

use crate::normalize_resolution_query;
use crate::slug::entity_slug;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum MatchTier {
    Exact = 1,
    CaseInsensitive = 2,
    Email = 3,
    Slug = 4,
    FirstWord = 5,
    TokenSubset = 6,
    Prefix = 7,
    Fuzzy = 8,
}

impl MatchTier {
    pub fn is_high_confidence(self) -> bool {
        self <= MatchTier::Slug
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntityNameCandidate {
    pub id: Option<String>,
    pub name: String,
    pub aka: Vec<String>,
    pub emails: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EntityNameMatch {
    pub candidate_index: usize,
    pub tier: MatchTier,
}

pub fn find_matching_entity(
    detected_name: &str,
    candidates: &[EntityNameCandidate],
    fuzzy_threshold: f64,
) -> Option<EntityNameMatch> {
    if detected_name.is_empty() || candidates.is_empty() {
        return None;
    }

    let detected_lower = normalize_resolution_query(detected_name);
    let detected_slug = entity_slug(detected_name);

    let mut exact_case_map: BTreeMap<String, usize> = BTreeMap::new();
    let mut lower_map: BTreeMap<String, usize> = BTreeMap::new();
    let mut id_map: BTreeMap<String, usize> = BTreeMap::new();
    let mut first_word_map: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    let mut fuzzy_candidates = OrderedFuzzyCandidates::default();
    let mut email_map: BTreeMap<String, usize> = BTreeMap::new();

    for (candidate_index, candidate) in candidates.iter().enumerate() {
        let name = candidate.name.as_str();
        let entity_id = candidate.id.as_deref().unwrap_or("");
        if name.is_empty() {
            continue;
        }

        exact_case_map.insert(name.to_string(), candidate_index);
        lower_map.insert(normalize_resolution_query(name), candidate_index);

        if entity_id.is_empty() {
            let name_slug = entity_slug(name);
            if !name_slug.is_empty() {
                id_map.insert(name_slug, candidate_index);
            }
        } else {
            exact_case_map.insert(entity_id.to_string(), candidate_index);
            lower_map.insert(normalize_resolution_query(entity_id), candidate_index);
            id_map.insert(entity_id.to_string(), candidate_index);
        }

        for aka in &candidate.aka {
            if !aka.is_empty() {
                exact_case_map.insert(aka.clone(), candidate_index);
                lower_map.insert(normalize_resolution_query(aka), candidate_index);
            }
        }

        for email in &candidate.emails {
            if !email.is_empty() {
                email_map.insert(normalize_resolution_query(email), candidate_index);
            }
        }

        if let Some(first_word) = first_word_key(name) {
            first_word_map
                .entry(first_word)
                .or_default()
                .push(candidate_index);
        }

        fuzzy_candidates.insert(name, candidate_index);
        for aka in &candidate.aka {
            if !aka.is_empty() {
                fuzzy_candidates.insert(aka, candidate_index);
            }
        }
    }

    if let Some(candidate_index) = exact_case_map.get(detected_name) {
        return Some(entity_match(*candidate_index, MatchTier::Exact));
    }

    if let Some(candidate_index) = lower_map.get(&detected_lower) {
        return Some(entity_match(*candidate_index, MatchTier::CaseInsensitive));
    }

    if detected_name.contains('@')
        && let Some(candidate_index) = email_map.get(&detected_lower)
    {
        return Some(entity_match(*candidate_index, MatchTier::Email));
    }

    if !detected_slug.is_empty()
        && let Some(candidate_index) = id_map.get(&detected_slug)
    {
        return Some(entity_match(*candidate_index, MatchTier::Slug));
    }

    if char_len(detected_name) >= 3 {
        if let Some(matches) = first_word_map.get(&detected_lower)
            && matches.len() == 1
        {
            return Some(entity_match(matches[0], MatchTier::FirstWord));
        }

        if let Some(detected_first) = detected_name.split_whitespace().next() {
            let detected_first = normalize_resolution_query(detected_first);
            if detected_first != detected_lower
                && char_len(&detected_first) >= 3
                && let Some(matches) = first_word_map.get(&detected_first)
                && matches.len() == 1
            {
                let matched_name = candidates[matches[0]].name.as_str();
                if single_token_first_word_match(&detected_first, matched_name) {
                    return Some(entity_match(matches[0], MatchTier::FirstWord));
                }
            }
        }
    }

    let subset_matches: Vec<usize> = candidates
        .iter()
        .enumerate()
        .filter_map(|(candidate_index, candidate)| {
            if candidate.name.is_empty() {
                return None;
            }
            token_subset_match(
                &detected_lower,
                &normalize_resolution_query(&candidate.name),
            )
            .then_some(candidate_index)
        })
        .collect();
    if subset_matches.len() == 1 {
        return Some(entity_match(subset_matches[0], MatchTier::TokenSubset));
    }

    let prefix_matches: Vec<usize> = candidates
        .iter()
        .enumerate()
        .filter_map(|(candidate_index, candidate)| {
            if candidate.name.is_empty() {
                return None;
            }
            prefix_token_match(
                &detected_lower,
                &normalize_resolution_query(&candidate.name),
            )
            .then_some(candidate_index)
        })
        .collect();
    if prefix_matches.len() == 1 {
        return Some(entity_match(prefix_matches[0], MatchTier::Prefix));
    }

    if char_len(detected_name) >= 4
        && let Some(candidate_index) =
            extract_one_fuzzy(detected_name, &fuzzy_candidates, fuzzy_threshold)
    {
        return Some(entity_match(candidate_index, MatchTier::Fuzzy));
    }

    None
}

fn entity_match(candidate_index: usize, tier: MatchTier) -> EntityNameMatch {
    EntityNameMatch {
        candidate_index,
        tier,
    }
}

pub fn token_subset_match(name_a_lower: &str, name_b_lower: &str) -> bool {
    let tokens_a: Vec<&str> = unique_sorted_tokens(name_a_lower);
    let tokens_b: Vec<&str> = unique_sorted_tokens(name_b_lower);
    let (shorter, longer) = match tokens_a.len().cmp(&tokens_b.len()) {
        Ordering::Greater => (&tokens_b, &tokens_a),
        _ => (&tokens_a, &tokens_b),
    };
    shorter.len() >= 2 && shorter.iter().all(|token| longer.contains(token))
}

pub fn prefix_token_match(name_a_lower: &str, name_b_lower: &str) -> bool {
    let mut sorted_a: Vec<&str> = name_a_lower.split_whitespace().collect();
    let mut sorted_b: Vec<&str> = name_b_lower.split_whitespace().collect();
    sorted_a.sort_unstable();
    sorted_b.sort_unstable();
    if sorted_a.len() != sorted_b.len() {
        return false;
    }
    sorted_a.iter().zip(sorted_b.iter()).all(|(left, right)| {
        left == right
            || (char_len(left) >= 4 && right.starts_with(left))
            || (char_len(right) >= 4 && left.starts_with(right))
    })
}

fn first_word_key(name: &str) -> Option<String> {
    let first_word = normalize_resolution_query(name)
        .split_whitespace()
        .next()?
        .to_owned();
    (char_len(&first_word) >= 3).then_some(first_word)
}

pub fn first_word_match(query_lower: &str, entity_name: &str) -> bool {
    char_len(query_lower) >= 3 && first_word_key(entity_name).as_deref() == Some(query_lower)
}

pub fn single_token_first_word_match(query_first: &str, entity_name: &str) -> bool {
    !entity_name.is_empty()
        && entity_name.split_whitespace().count() == 1
        && first_word_match(query_first, entity_name)
}

fn unique_sorted_tokens(text: &str) -> Vec<&str> {
    let mut tokens: Vec<&str> = text.split_whitespace().collect();
    tokens.sort_unstable();
    tokens.dedup();
    tokens
}

pub fn token_sort(text: &str) -> String {
    let mut tokens: Vec<&str> = text.split_whitespace().collect();
    tokens.sort_unstable();
    tokens.join(" ")
}

fn extract_one_fuzzy(
    query: &str,
    fuzzy_candidates: &OrderedFuzzyCandidates,
    fuzzy_threshold: f64,
) -> Option<usize> {
    let sorted_query = token_sort(query);
    let mut best: Option<(f64, usize)> = None;
    for (candidate, candidate_index) in fuzzy_candidates.iter() {
        let sorted_candidate = token_sort(candidate);
        let score = rapidfuzz::fuzz::ratio(sorted_query.chars(), sorted_candidate.chars()) * 100.0;
        if score >= fuzzy_threshold {
            match best {
                Some((best_score, _)) if score <= best_score => {}
                _ => best = Some((score, candidate_index)),
            }
        }
    }
    best.map(|(_score, candidate_index)| candidate_index)
}

pub fn char_len(text: &str) -> usize {
    text.chars().count()
}

#[derive(Debug, Default)]
struct OrderedFuzzyCandidates {
    keys: Vec<String>,
    values: BTreeMap<String, usize>,
}

impl OrderedFuzzyCandidates {
    fn insert(&mut self, key: &str, candidate_index: usize) {
        match self.values.entry(key.to_string()) {
            Entry::Vacant(entry) => {
                self.keys.push(entry.key().clone());
                entry.insert(candidate_index);
            }
            Entry::Occupied(mut entry) => {
                entry.insert(candidate_index);
            }
        }
    }

    fn iter(&self) -> impl Iterator<Item = (&str, usize)> {
        self.keys.iter().filter_map(|key| {
            self.values
                .get(key)
                .map(|candidate_index| (key.as_str(), *candidate_index))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(
        id: Option<&str>,
        name: &str,
        aka: &[&str],
        emails: &[&str],
    ) -> EntityNameCandidate {
        EntityNameCandidate {
            id: id.map(str::to_string),
            name: name.to_string(),
            aka: aka.iter().map(|item| (*item).to_string()).collect(),
            emails: emails.iter().map(|item| (*item).to_string()).collect(),
        }
    }

    fn assert_match(
        query: &str,
        candidates: &[EntityNameCandidate],
        fuzzy_threshold: f64,
        expected_index: usize,
        expected_id: Option<&str>,
        expected_tier: MatchTier,
    ) {
        let result = find_matching_entity(query, candidates, fuzzy_threshold)
            .expect("query should match one candidate");
        assert_eq!(result.candidate_index, expected_index);
        assert_eq!(
            candidates[result.candidate_index].id.as_deref(),
            expected_id
        );
        assert_eq!(result.tier, expected_tier);
    }

    fn assert_no_match(query: &str, candidates: &[EntityNameCandidate], fuzzy_threshold: f64) {
        assert_eq!(
            find_matching_entity(query, candidates, fuzzy_threshold),
            None
        );
    }

    #[test]
    fn exact_name_matches_tier_1() {
        let candidates = [candidate(
            Some("robert_johnson"),
            "Robert Johnson",
            &[],
            &[],
        )];
        assert_match(
            "Robert Johnson",
            &candidates,
            90.0,
            0,
            Some("robert_johnson"),
            MatchTier::Exact,
        );
    }

    #[test]
    fn exact_id_matches_tier_1() {
        let candidates = [candidate(
            Some("robert_johnson"),
            "Robert Johnson",
            &[],
            &[],
        )];
        assert_match(
            "robert_johnson",
            &candidates,
            90.0,
            0,
            Some("robert_johnson"),
            MatchTier::Exact,
        );
    }

    #[test]
    fn exact_aka_matches_tier_1() {
        let candidates = [candidate(
            Some("robert_johnson"),
            "Robert Johnson",
            &["Bob"],
            &[],
        )];
        assert_match(
            "Bob",
            &candidates,
            90.0,
            0,
            Some("robert_johnson"),
            MatchTier::Exact,
        );
    }

    #[test]
    fn case_insensitive_name_id_and_aka_match_tier_2() {
        let candidates = [candidate(
            Some("robert_johnson"),
            "Robert Johnson",
            &["Bob"],
            &[],
        )];
        assert_match(
            "robert johnson",
            &candidates,
            90.0,
            0,
            Some("robert_johnson"),
            MatchTier::CaseInsensitive,
        );
        assert_match(
            "ROBERT_JOHNSON",
            &candidates,
            90.0,
            0,
            Some("robert_johnson"),
            MatchTier::CaseInsensitive,
        );
        assert_match(
            "bob",
            &candidates,
            90.0,
            0,
            Some("robert_johnson"),
            MatchTier::CaseInsensitive,
        );
    }

    #[test]
    fn unified_normalization_resolves_opaque_unicode_pairs_at_tier_2() {
        let cases = [
            ("Straße Handel", "STRASSE HANDEL"),
            ("ΟΔΥΣΣΕΥΣ", "οδυσσευσ"),
            ("ﬁrefly labs", "firefly labs"),
        ];
        for (query, name) in cases {
            let candidates = [candidate(Some("xx_opaque_identity"), name, &[], &[])];
            assert_match(
                query,
                &candidates,
                90.0,
                0,
                Some("xx_opaque_identity"),
                MatchTier::CaseInsensitive,
            );
        }
    }

    #[test]
    fn email_matches_tier_3_when_query_contains_at() {
        let candidates = [candidate(
            Some("alice"),
            "Alice Example",
            &[],
            &["alice@example.com"],
        )];
        assert_match(
            "alice@example.com",
            &candidates,
            90.0,
            0,
            Some("alice"),
            MatchTier::Email,
        );
        assert_match(
            "ALICE@EXAMPLE.COM",
            &candidates,
            90.0,
            0,
            Some("alice"),
            MatchTier::Email,
        );
    }

    #[test]
    fn email_map_is_gated_by_at_sign() {
        let candidates = [candidate(Some("alice"), "Alice Example", &[], &["handle"])];
        assert_no_match("handle", &candidates, 90.0);
    }

    #[test]
    fn slugified_query_matches_raw_id_tier_4() {
        let candidates = [candidate(Some("some_name"), "Different", &[], &[])];
        assert_match(
            "Some Name",
            &candidates,
            100.0,
            0,
            Some("some_name"),
            MatchTier::Slug,
        );
    }

    #[test]
    fn first_word_short_to_long_matches_tier_5() {
        let candidates = [candidate(Some("javier_garcia"), "Javier Garcia", &[], &[])];
        assert_match(
            "Javier",
            &candidates,
            90.0,
            0,
            Some("javier_garcia"),
            MatchTier::FirstWord,
        );
    }

    #[test]
    fn first_word_long_to_short_matches_single_token_tier_5() {
        let candidates = [candidate(Some("javier"), "Javier", &[], &[])];
        assert_match(
            "Javier Garcia",
            &candidates,
            90.0,
            0,
            Some("javier"),
            MatchTier::FirstWord,
        );
    }

    #[test]
    fn ambiguous_first_word_matches_return_none() {
        let short_to_long = [
            candidate(Some("jg"), "Javier Garcia", &[], &[]),
            candidate(Some("jr"), "Javier Rodriguez", &[], &[]),
        ];
        assert_no_match("Javier", &short_to_long, 100.0);

        let long_to_short = [
            candidate(Some("j"), "Javier", &[], &[]),
            candidate(Some("jr"), "Javier Rodriguez", &[], &[]),
        ];
        assert_no_match("Javier Garcia", &long_to_short, 100.0);
    }

    #[test]
    fn first_word_requires_three_characters() {
        let candidates = [candidate(Some("li_wei"), "Li Wei", &[], &[])];
        assert_no_match("Li", &candidates, 90.0);
    }

    #[test]
    fn token_subset_matches_both_directions_tier_6() {
        let long_name = [candidate(
            Some("josh_jones_dilworth"),
            "Josh Jones Dilworth",
            &[],
            &[],
        )];
        assert_match(
            "Jones Dilworth",
            &long_name,
            100.0,
            0,
            Some("josh_jones_dilworth"),
            MatchTier::TokenSubset,
        );

        let short_name = [candidate(
            Some("jones_dilworth"),
            "Jones Dilworth",
            &[],
            &[],
        )];
        assert_match(
            "Josh Jones Dilworth",
            &short_name,
            100.0,
            0,
            Some("jones_dilworth"),
            MatchTier::TokenSubset,
        );
    }

    #[test]
    fn token_subset_single_token_non_first_token_returns_none_at_default_threshold() {
        let candidates = [candidate(
            Some("josh_jones_dilworth"),
            "Josh Jones Dilworth",
            &[],
            &[],
        )];
        assert_no_match("Dilworth", &candidates, 90.0);
    }

    #[test]
    fn ambiguous_token_subset_returns_none() {
        let candidates = [
            candidate(Some("josh"), "Josh Jones Dilworth", &[], &[]),
            candidate(Some("mary"), "Mary Jones Dilworth", &[], &[]),
        ];
        assert_no_match("Jones Dilworth", &candidates, 100.0);
    }

    #[test]
    fn prefix_matches_both_directions_tier_7() {
        let full = [candidate(
            Some("christopher_dewolfe"),
            "Christopher DeWolfe",
            &[],
            &[],
        )];
        assert_match(
            "Chris DeWolfe",
            &full,
            100.0,
            0,
            Some("christopher_dewolfe"),
            MatchTier::Prefix,
        );

        let short = [candidate(Some("chris_dewolfe"), "Chris DeWolfe", &[], &[])];
        assert_match(
            "Christopher DeWolfe",
            &short,
            100.0,
            0,
            Some("chris_dewolfe"),
            MatchTier::Prefix,
        );
    }

    #[test]
    fn prefix_requires_four_characters() {
        let candidates = [candidate(
            Some("jonathan_smith"),
            "Jonathan Smith",
            &[],
            &[],
        )];
        assert_no_match("Jon Smith", &candidates, 90.0);
        assert_match(
            "Jona Smith",
            &candidates,
            100.0,
            0,
            Some("jonathan_smith"),
            MatchTier::Prefix,
        );
    }

    #[test]
    fn prefix_requires_same_token_count_at_default_threshold() {
        let candidates = [candidate(
            Some("cjd"),
            "Christopher James DeWolfe",
            &[],
            &[],
        )];
        assert_no_match("Chris DeWolfe", &candidates, 90.0);
    }

    #[test]
    fn ambiguous_prefix_returns_none() {
        let candidates = [
            candidate(Some("cd"), "Christopher DeWolfe", &[], &[]),
            candidate(Some("chd"), "Christine DeWolfe", &[], &[]),
        ];
        assert_no_match("Chris DeWolfe", &candidates, 100.0);
    }

    #[test]
    fn fuzzy_typo_matches_tier_8() {
        let candidates = [candidate(
            Some("christopher_dewolfe"),
            "Christopher DeWolfe",
            &[],
            &[],
        )];
        assert_match(
            "Christoph DeWolffe",
            &candidates,
            90.0,
            0,
            Some("christopher_dewolfe"),
            MatchTier::Fuzzy,
        );
    }

    #[test]
    fn fuzzy_below_threshold_returns_none() {
        let candidates = [candidate(
            Some("christopher_dewolfe"),
            "Christopher DeWolfe",
            &[],
            &[],
        )];
        assert_no_match("Christoph DeWolffe", &candidates, 99.0);
    }

    #[test]
    fn fuzzy_cutoff_is_inclusive() {
        let candidates = [candidate(
            Some("christopher_dewolfe"),
            "Christopher DeWolfe",
            &[],
            &[],
        )];
        assert_match(
            "Christoph DeWolffe",
            &candidates,
            91.89189189189189,
            0,
            Some("christopher_dewolfe"),
            MatchTier::Fuzzy,
        );
    }

    #[test]
    fn fuzzy_tie_keeps_first_in_order() {
        let candidates = [
            candidate(Some("first"), "Alicia X", &[], &[]),
            candidate(Some("second"), "Alicia Y", &[], &[]),
        ];
        assert_match(
            "Alicia",
            &candidates,
            50.0,
            0,
            Some("first"),
            MatchTier::Fuzzy,
        );
    }

    #[test]
    fn rapidfuzz_ratio_is_scaled_to_python_percent() {
        let score =
            rapidfuzz::fuzz::ratio("Alicia Johnson".chars(), "Alice Johnson".chars()) * 100.0;
        assert!((score - 88.88888888888889).abs() < 1e-9);
    }

    #[test]
    fn high_confidence_boundary_matches_python_tiers() {
        assert!(MatchTier::Exact.is_high_confidence());
        assert!(MatchTier::CaseInsensitive.is_high_confidence());
        assert!(MatchTier::Email.is_high_confidence());
        assert!(MatchTier::Slug.is_high_confidence());
        assert!(!MatchTier::FirstWord.is_high_confidence());
        assert!(!MatchTier::TokenSubset.is_high_confidence());
        assert!(!MatchTier::Prefix.is_high_confidence());
        assert!(!MatchTier::Fuzzy.is_high_confidence());
    }

    #[test]
    fn id_present_does_not_add_name_slug() {
        let candidates = [candidate(Some("custom_id"), "Some Name", &[], &[])];
        assert_no_match("some_name", &candidates, 100.0);
    }

    #[test]
    fn id_absent_synthetic_name_slug_matches_idless() {
        let candidates = [candidate(None, "Some Name", &[], &[])];
        assert_match("some_name", &candidates, 100.0, 0, None, MatchTier::Slug);
    }

    #[test]
    fn raw_id_not_slugified() {
        let candidates = [candidate(Some("some-id"), "Unrelated", &[], &[])];
        assert_no_match("some id", &candidates, 100.0);
    }

    #[test]
    fn first_word_ignores_aka() {
        let candidates = [candidate(
            Some("robert"),
            "Robert Johnson",
            &["Bob Johnson"],
            &[],
        )];
        assert_no_match("Bob", &candidates, 90.0);
    }

    #[test]
    fn token_subset_uses_name_only_not_aka() {
        let candidates = [candidate(
            Some("r"),
            "Robert Johnson",
            &["Josh Jones Dilworth"],
            &[],
        )];
        assert_no_match("Jones Dilworth", &candidates, 100.0);
    }

    #[test]
    fn prefix_uses_name_only_not_aka() {
        let candidates = [candidate(
            Some("r"),
            "Robert Johnson",
            &["Christopher DeWolfe"],
            &[],
        )];
        assert_no_match("Chris DeWolfe", &candidates, 100.0);
    }

    #[test]
    fn fuzzy_candidates_exclude_id() {
        let candidates = [candidate(
            Some("robert_johnson"),
            "Completely Different",
            &[],
            &[],
        )];
        assert_no_match("robert_johnsn", &candidates, 90.0);
    }

    #[test]
    fn long_to_short_ambiguity_counted_before_single_token_check() {
        let candidates = [
            candidate(Some("javier"), "Javier", &[], &[]),
            candidate(Some("jr"), "Javier Rodriguez", &[], &[]),
        ];
        assert_no_match("Javier Garcia", &candidates, 100.0);
    }

    #[test]
    fn leading_and_trailing_space_queries_match_first_word_after_normalization() {
        let candidates = [candidate(Some("jg"), "Javier Garcia", &[], &[])];
        assert_match(
            " Javier",
            &candidates,
            100.0,
            0,
            Some("jg"),
            MatchTier::FirstWord,
        );
        assert_match(
            "Javier ",
            &candidates,
            100.0,
            0,
            Some("jg"),
            MatchTier::FirstWord,
        );
    }

    #[test]
    fn whitespace_only_query_returns_none() {
        let candidates = [candidate(Some("alice"), "Alice", &[], &[])];
        assert_no_match("   ", &candidates, 90.0);
    }

    #[test]
    fn whitespace_only_entity_name_skips_first_word_but_stays_matchable() {
        let candidates = [candidate(Some("x"), "   ", &[], &[])];
        // Python raises IndexError for all three cases during map building, so
        // these pin Rust's intentional non-panicking divergence, not parity.
        assert_no_match("Bob", &candidates, 90.0);
        assert_match("x", &candidates, 90.0, 0, Some("x"), MatchTier::Exact);
        assert_match("   ", &candidates, 90.0, 0, Some("x"), MatchTier::Exact);
    }

    #[test]
    fn fuzzy_len_under_four_skipped() {
        let candidates = [candidate(Some("alice"), "Alice", &[], &[])];
        assert_no_match("Ali", &candidates, 50.0);
    }

    #[test]
    fn duplicate_exact_last_write_wins() {
        let candidates = [
            candidate(Some("a"), "Alex Doe", &[], &[]),
            candidate(Some("b"), "Alex Doe", &[], &[]),
        ];
        assert_match(
            "Alex Doe",
            &candidates,
            90.0,
            1,
            Some("b"),
            MatchTier::Exact,
        );
    }

    #[test]
    fn duplicate_case_insensitive_last_write_wins() {
        let candidates = [
            candidate(Some("first"), "Alex Doe", &[], &[]),
            candidate(Some("second"), "alex doe", &[], &[]),
        ];
        assert_match(
            "ALEX DOE",
            &candidates,
            90.0,
            1,
            Some("second"),
            MatchTier::CaseInsensitive,
        );
    }

    #[test]
    fn duplicate_id_map_last_write_wins() {
        let candidates = [
            candidate(Some("shared_slug"), "First", &[], &[]),
            candidate(Some("shared_slug"), "Second", &[], &[]),
        ];
        assert_match(
            "shared slug",
            &candidates,
            90.0,
            1,
            Some("shared_slug"),
            MatchTier::Slug,
        );
    }

    #[test]
    fn duplicate_email_map_last_write_wins() {
        let candidates = [
            candidate(Some("first"), "First", &[], &["shared@example.com"]),
            candidate(Some("second"), "Second", &[], &["shared@example.com"]),
        ];
        assert_match(
            "SHARED@EXAMPLE.COM",
            &candidates,
            90.0,
            1,
            Some("second"),
            MatchTier::Email,
        );
    }

    #[test]
    fn duplicate_fuzzy_key_last_write_wins() {
        let candidates = [
            candidate(Some("first"), "Alice Doe", &[], &[]),
            candidate(Some("second"), "Alice Doe", &[], &[]),
        ];
        // Non-default threshold is deliberate: it isolates tier 8 from earlier
        // tiers while preserving Python's duplicate-key last-write behavior.
        assert_match(
            "Alce Doe",
            &candidates,
            80.0,
            1,
            Some("second"),
            MatchTier::Fuzzy,
        );
    }

    #[test]
    fn empty_name_skips_all_maps() {
        let candidates = [candidate(
            Some("id_only"),
            "",
            &["Alias"],
            &["id@example.com"],
        )];
        assert_no_match("id_only", &candidates, 90.0);
        assert_no_match("Alias", &candidates, 90.0);
        assert_no_match("id@example.com", &candidates, 90.0);
    }
}
