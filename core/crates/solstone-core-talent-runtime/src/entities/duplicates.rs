// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Duplicate discovery between saved entities, without a model.
//!
//! The daily review compares the names one facet detected over its window.
//! This pass looks across the whole registry for what that review can't see:
//! two saved entities that hold one email address or one alias, and two saved
//! entities whose names are variants of each other but share no facet. It only
//! suggests. Each pair becomes an open row in the duplicate review and the
//! owner decides; nothing is merged here.
//!
//! Every comparison goes through an index keyed by the shared value (an email,
//! an alias, the start of a name word), so a large registry is never compared
//! pair by pair.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::Path;

use serde_json::Value;
use solstone_core_entity::DiscoveredMergeCandidate;
use solstone_core_entity_matching::{is_name_variant_match, normalize_resolution_query};

use super::review::{FORM_WORDS, narrows};

/// The pair's entities hold one email address.
pub const SAME_EMAIL: &str = "same-email";
/// One entity's alias is the other's name or alias.
pub const SHARED_ALIAS: &str = "shared-alias";
/// The names are variants of each other.
pub const NAME_VARIANT: &str = "name-variant";

/// Shorter aliases ("Al", "J") name too many people to pair on.
const MIN_ALIAS_CHARS: usize = 3;
/// An email or alias more saved entities hold than this belongs to a group
/// (a shared inbox, a team's name), not to one person or thing.
const MAX_HOLDERS: usize = 3;
/// The most entities compared under one name word. A word more names carry
/// is left to the other words of those names.
const MAX_NAME_WORD: usize = 48;
/// The leading characters of a name word that group names for comparison;
/// a name variant shares at least this much of one word.
const NAME_WORD_KEY: usize = 4;

/// What one pass found and recorded.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct DuplicateDiscovery {
    /// Pairs found, before those already suggested or answered were left out.
    pub found: usize,
    /// New open suggestions written.
    pub recorded: usize,
}

/// A saved entity as discovery reads it.
struct Saved {
    id: String,
    name: String,
    entity_type: String,
    /// The facets it is attached to, not detached from.
    facets: BTreeSet<String>,
    emails: Vec<String>,
    akas: Vec<String>,
}

/// Find duplicate pairs among the journal's saved entities and record each new
/// one as an open suggestion first surfaced on `day`. A pair that already has a
/// row (open, answered or resolved, in any facet), that the owner kept separate
/// as speakers, or whose speaker suggestion they dismissed, is never suggested.
/// The owner's own entity, hidden entities and entities in the trash or merged
/// away are never paired.
pub fn record_duplicate_candidates(
    journal: &Path,
    day: &str,
) -> Result<DuplicateDiscovery, String> {
    let saved = saved_entities(journal)?;
    let (pairs, compared) = find_pairs(&saved);
    let found = pairs.len();
    log::debug!(
        "entity duplicates: {} saved entities, {compared} name comparisons",
        saved.len()
    );
    let dismissed = dismissed_speaker_pairs(journal)?;
    let mut discovered = Vec::new();
    for ((left, right), (basis, summary)) in pairs {
        let (a, b) = (&saved[left], &saved[right]);
        if dismissed.contains(&ordered(&a.id, &b.id)) {
            continue;
        }
        if solstone_core_speaker_resolve::keep_separate::find_assertion(journal, &a.id, &b.id)
            .map_err(|error| error.to_string())?
            .is_some()
        {
            continue;
        }
        let (source, target) = if fuller(b, a) { (a, b) } else { (b, a) };
        let Some(facet) = filing_facet(source, target) else {
            continue;
        };
        discovered.push(DiscoveredMergeCandidate {
            facet,
            source: source.name.clone(),
            source_slug: source.id.clone(),
            target: target.name.clone(),
            target_slug: target.id.clone(),
            basis: basis.to_owned(),
            summary,
        });
    }
    let recorded =
        solstone_core_entity::record_discovered_merge_candidates(journal, day, &discovered)
            .map_err(|error| error.to_string())?;
    if recorded > 0 {
        log::info!("entity duplicates: {found} pairs found, {recorded} new suggestions");
    }
    Ok(DuplicateDiscovery { found, recorded })
}

fn saved_entities(journal: &Path) -> Result<Vec<Saved>, String> {
    let mut facets: HashMap<String, BTreeSet<String>> = HashMap::new();
    for link in solstone_core_facets::plan_facet_entity_relationships(journal)
        .map_err(|error| error.to_string())?
        .relationships
    {
        if !link.is_detached {
            facets
                .entry(link.entity_id)
                .or_default()
                .insert(link.facet_dir);
        }
    }
    let strings = |value: &Value, field: &str| -> Vec<String> {
        value
            .get(field)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(str::to_owned)
            .collect()
    };
    Ok(solstone_core_entity::load_all_journal_entities(journal)
        .map_err(|error| error.to_string())?
        .into_iter()
        .filter(|entity| !entity.is_principal() && !entity.is_blocked())
        .filter_map(|entity| {
            let name = entity.value.get("name").and_then(Value::as_str)?.trim();
            if name.is_empty() {
                return None;
            }
            Some(Saved {
                name: name.to_owned(),
                entity_type: entity.entity_type().unwrap_or_default().trim().to_owned(),
                facets: facets.remove(&entity.id).unwrap_or_default(),
                emails: strings(&entity.value, "emails"),
                akas: strings(&entity.value, "aka"),
                id: entity.id.clone(),
            })
        })
        .collect())
}

type Pairs = BTreeMap<(usize, usize), (&'static str, String)>;

/// Every pair found, keyed by the two entities' places in `saved`, with the
/// strongest basis that found it: an email, then an alias, then the names;
/// and how many name pairs were compared.
fn find_pairs(saved: &[Saved]) -> (Pairs, usize) {
    let mut pairs = Pairs::new();

    let mut emails: HashMap<String, Vec<(usize, &str)>> = HashMap::new();
    for (index, entity) in saved.iter().enumerate() {
        for email in &entity.emails {
            let key = email.to_lowercase();
            if key.contains('@') {
                emails.entry(key).or_default().push((index, email));
            }
        }
    }
    for holders in emails.into_values() {
        for (left, right, written) in holder_pairs(&holders) {
            pairs
                .entry((left, right))
                .or_insert((SAME_EMAIL, written.to_owned()));
        }
    }

    // A name enters the index too, so one entity's alias meets the other's
    // name; two bare names meeting are left to the name comparison below.
    let mut aliases: HashMap<String, Vec<(usize, &str, bool)>> = HashMap::new();
    for (index, entity) in saved.iter().enumerate() {
        let names = std::iter::once((entity.name.as_str(), false))
            .chain(entity.akas.iter().map(|aka| (aka.as_str(), true)));
        for (written, is_alias) in names {
            let key = normalize_resolution_query(written);
            if key.chars().count() >= MIN_ALIAS_CHARS {
                aliases
                    .entry(key)
                    .or_default()
                    .push((index, written, is_alias));
            }
        }
    }
    for holders in aliases.into_values() {
        let entities = holders
            .iter()
            .map(|(index, written, _)| (*index, *written))
            .collect::<Vec<_>>();
        for (left, right, _) in holder_pairs(&entities) {
            let alias = holders
                .iter()
                .find(|(index, _, is_alias)| *is_alias && (*index == left || *index == right));
            if let Some((_, written, _)) = alias
                && same_kind(&saved[left], &saved[right])
            {
                pairs
                    .entry((left, right))
                    .or_insert((SHARED_ALIAS, (*written).to_owned()));
            }
        }
    }

    let mut words: HashMap<String, Vec<usize>> = HashMap::new();
    for (index, entity) in saved.iter().enumerate() {
        // A pair across facets: an entity attached nowhere has none to cross.
        if entity.facets.is_empty() {
            continue;
        }
        let mut keys = HashSet::new();
        for word in normalize_resolution_query(&entity.name).split_whitespace() {
            if FORM_WORDS.contains(&word) {
                continue;
            }
            let key = word.chars().take(NAME_WORD_KEY).collect::<String>();
            if keys.insert(key.clone()) {
                words.entry(key).or_default().push(index);
            }
        }
    }
    let mut compared = HashSet::new();
    for holders in words.values() {
        if holders.len() > MAX_NAME_WORD {
            continue;
        }
        for (place, &left) in holders.iter().enumerate() {
            for &right in &holders[place + 1..] {
                let key = (left.min(right), left.max(right));
                if pairs.contains_key(&key) || !compared.insert(key) {
                    continue;
                }
                let (a, b) = (&saved[key.0], &saved[key.1]);
                if a.facets.is_disjoint(&b.facets)
                    && same_kind(a, b)
                    && is_name_variant_match(&a.name, &b.name)
                    && !narrows(&a.name, &b.name)
                {
                    pairs.insert(key, (NAME_VARIANT, String::new()));
                }
            }
        }
    }
    (pairs, compared.len())
}

/// The distinct entity pairs among one value's holders, each with the value as
/// the first of them wrote it; none when too many hold it.
fn holder_pairs<'a>(holders: &[(usize, &'a str)]) -> Vec<(usize, usize, &'a str)> {
    let mut entities: Vec<(usize, &str)> = Vec::new();
    for &(index, written) in holders {
        if !entities.iter().any(|(held, _)| *held == index) {
            entities.push((index, written));
        }
    }
    if entities.len() < 2 || entities.len() > MAX_HOLDERS {
        return Vec::new();
    }
    let mut pairs = Vec::new();
    for (place, &(left, written)) in entities.iter().enumerate() {
        for &(right, _) in &entities[place + 1..] {
            pairs.push((left.min(right), left.max(right), written));
        }
    }
    pairs
}

/// Both of one type, or neither typed.
fn same_kind(a: &Saved, b: &Saved) -> bool {
    a.entity_type.eq_ignore_ascii_case(&b.entity_type)
}

/// Whether `a`'s name is the fuller of the two, so it is kept: more words,
/// then more characters, then the earlier id.
fn fuller(a: &Saved, b: &Saved) -> bool {
    let size = |entity: &Saved| {
        (
            entity.name.split_whitespace().count(),
            entity.name.chars().count(),
        )
    };
    size(a).cmp(&size(b)).then_with(|| b.id.cmp(&a.id)).is_gt()
}

/// The facet a suggestion is filed under: one both are attached to, else the
/// source's, else the target's; always a facet one of them is in.
fn filing_facet(source: &Saved, target: &Saved) -> Option<String> {
    source
        .facets
        .intersection(&target.facets)
        .next()
        .or_else(|| source.facets.iter().next())
        .or_else(|| target.facets.iter().next())
        .cloned()
}

fn ordered(a: &str, b: &str) -> (String, String) {
    if a <= b {
        (a.to_owned(), b.to_owned())
    } else {
        (b.to_owned(), a.to_owned())
    }
}

/// Speaker suggestions the owner dismissed, by entity pair.
fn dismissed_speaker_pairs(journal: &Path) -> Result<HashSet<(String, String)>, String> {
    let rows =
        solstone_core_speaker_resolve::speaker_review_candidates::load_candidates_strict(journal)
            .map_err(|error| error.to_string())?;
    Ok(rows
        .iter()
        .filter(|row| row.get("status").and_then(Value::as_str) == Some("dismissed"))
        .filter_map(|row| {
            Some(ordered(
                row.get("source_id")?.as_str()?,
                row.get("target_id")?.as_str()?,
            ))
        })
        .collect())
}

#[cfg(test)]
#[path = "duplicates_tests.rs"]
mod tests;
