// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The keys a detected name is matched on to tell whether an entity covers it.

use std::collections::HashSet;

use crate::matcher::EntityNameCandidate;
use crate::normalize::normalize_resolution_query;
use crate::slug::entity_slug;

/// The high-confidence keys of a set of entities (the matcher's tiers 1-4:
/// exact, case-insensitive, email and slug), indexed once so that many
/// detected names can be checked against them cheaply.
///
/// A detected name is covered when it equals a name, alias or id exactly,
/// equals one after normalization, is an email address equal to one of the
/// entities' emails after normalization, or has the slug of an entity's id
/// (or of its name when it has no id). The heuristic tiers (first word, token
/// subset, prefix, fuzzy) never cover a detected name: they are a guess, and a
/// guess must not take a name out of the owner's sight.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExclusionKeys {
    exact: HashSet<String>,
    normalized: HashSet<String>,
    email: HashSet<String>,
    slug: HashSet<String>,
}

/// One detected name's keys, computed once and checked against many sets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NameProbe<'a> {
    name: &'a str,
    normalized: String,
    slug: String,
}

impl<'a> NameProbe<'a> {
    pub fn new(name: &'a str) -> Self {
        Self {
            name,
            normalized: normalize_resolution_query(name),
            slug: entity_slug(name),
        }
    }

    /// The name as given.
    pub fn name(&self) -> &str {
        self.name
    }

    /// The name's `normalize_resolution_query` key.
    pub fn normalized(&self) -> &str {
        &self.normalized
    }
}

impl ExclusionKeys {
    /// The keys of every candidate. A candidate with an empty name adds none.
    pub fn from_candidates(candidates: &[EntityNameCandidate]) -> Self {
        let mut keys = Self::default();
        for candidate in candidates {
            keys.add_candidate(candidate);
        }
        keys
    }

    /// Add one candidate's keys. A candidate with an empty name adds none.
    pub fn add_candidate(&mut self, candidate: &EntityNameCandidate) {
        if candidate.name.is_empty() {
            return;
        }
        self.add_exact(&candidate.name);
        for alias in &candidate.aka {
            if !alias.is_empty() {
                self.add_exact(alias);
            }
        }
        match candidate.id.as_deref().filter(|id| !id.is_empty()) {
            Some(id) => {
                self.add_exact(id);
                self.slug.insert(id.to_owned());
            }
            None => {
                let slug = entity_slug(&candidate.name);
                if !slug.is_empty() {
                    self.slug.insert(slug);
                }
            }
        }
        for email in &candidate.emails {
            if !email.is_empty() {
                self.email.insert(normalize_resolution_query(email));
            }
        }
    }

    fn add_exact(&mut self, value: &str) {
        self.exact.insert(value.to_owned());
        self.normalized.insert(normalize_resolution_query(value));
    }

    /// Whether no candidate added a key.
    pub fn is_empty(&self) -> bool {
        self.exact.is_empty() && self.slug.is_empty() && self.email.is_empty()
    }

    /// Whether these keys cover the detected `name`.
    pub fn contains(&self, name: &str) -> bool {
        if self.exact.contains(name) {
            return true;
        }
        self.contains_probe(&NameProbe::new(name))
    }

    /// Whether these keys cover the probed name.
    pub fn contains_probe(&self, probe: &NameProbe<'_>) -> bool {
        self.exact.contains(probe.name)
            || self.normalized.contains(&probe.normalized)
            || (probe.name.contains('@') && self.email.contains(&probe.normalized))
            || self.slug.contains(&probe.slug)
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
            id: id.map(str::to_owned),
            name: name.to_owned(),
            aka: aka.iter().map(|value| (*value).to_owned()).collect(),
            emails: emails.iter().map(|value| (*value).to_owned()).collect(),
        }
    }

    #[test]
    fn each_high_confidence_key_covers_a_name_and_a_guess_does_not() {
        let keys = ExclusionKeys::from_candidates(&[candidate(
            Some("robert_smith"),
            "Robert Smith",
            &["Bobby"],
            &["Bob@Example.com"],
        )]);
        assert!(keys.contains("Robert Smith"));
        assert!(keys.contains("robert  SMITH"));
        assert!(keys.contains("bobby"));
        assert!(keys.contains("bob@example.com"));
        assert!(keys.contains("robert_smith"));
        assert!(keys.contains("Robert-Smith"));
        assert!(!keys.contains("Robert"));
        assert!(!keys.contains("Rob Smith"));
        assert!(!keys.contains("bob@example.org"));
    }

    #[test]
    fn an_email_key_only_covers_an_email_shaped_name() {
        let keys = ExclusionKeys::from_candidates(&[candidate(None, "Ada", &[], &["ada at work"])]);
        assert!(!keys.contains("ADA AT WORK"));
    }

    #[test]
    fn a_nameless_candidate_adds_no_key() {
        let keys = ExclusionKeys::from_candidates(&[candidate(Some("ghost"), "", &[], &[])]);
        assert!(keys.is_empty());
        assert!(!keys.contains("ghost"));
    }

    #[test]
    fn a_candidate_without_an_id_is_covered_by_its_name_slug() {
        let keys = ExclusionKeys::from_candidates(&[candidate(None, "Grace Hopper", &[], &[])]);
        assert!(keys.contains("grace_hopper"));
        let probe = NameProbe::new("Grace  Hopper");
        assert!(keys.contains_probe(&probe));
        assert_eq!(probe.normalized(), "grace hopper");
    }
}
