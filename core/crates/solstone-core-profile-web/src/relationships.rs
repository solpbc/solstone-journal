// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Declared-facet relationship descriptions for profile reads.

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::Value;
use solstone_core_facets::{
    enrich_relationship_with_journal, list_declared_facet_names, read_facet_entity_link,
};

use crate::error::{ProfileError, ProfileResult};
use crate::resolution::ResolvedEntity;

/// One facet relationship as the profile reports it.
///
/// `detached` is reported, never filtered on. The owner detaching an entity from
/// a facet is a status this crate surfaces so the caller can act on it; it is not
/// grounds for the backend to withhold the relationship. Founder ruling
/// 2026-09-03.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FacetRelationship {
    pub(crate) description: String,
    pub(crate) detached: bool,
}

pub(crate) fn load_facet_descriptions(
    journal_root: &Path,
    target: &ResolvedEntity,
) -> ProfileResult<BTreeMap<String, FacetRelationship>> {
    let facets = list_declared_facet_names(journal_root).map_err(ProfileError::internal)?;
    let mut descriptions = BTreeMap::new();
    for facet in facets {
        let relationship = read_facet_entity_link(journal_root, &facet, &target.entity_id)
            .map_err(ProfileError::internal)?;
        let Some(relationship) = relationship else {
            continue;
        };
        let detached = relationship.value().get("detached") == Some(&Value::Bool(true));
        let enriched =
            enrich_relationship_with_journal(relationship.value(), Some(&target.entity.value));
        descriptions.insert(
            facet,
            FacetRelationship {
                description: string_field(&enriched, "description"),
                detached,
            },
        );
    }
    Ok(descriptions)
}

/// The subset of `descriptions` whose relationship the owner has detached.
pub(crate) fn detached_facets(descriptions: &BTreeMap<String, FacetRelationship>) -> Vec<String> {
    descriptions
        .iter()
        .filter(|(_, relationship)| relationship.detached)
        .map(|(facet, _)| facet.clone())
        .collect()
}

pub(crate) fn selected_facets(
    descriptions: &BTreeMap<String, FacetRelationship>,
    requested: Option<&[String]>,
) -> Vec<String> {
    match requested {
        None => descriptions.keys().cloned().collect(),
        Some(requested) => requested
            .iter()
            .filter(|facet| descriptions.contains_key(facet.as_str()))
            .cloned()
            .collect(),
    }
}

pub(crate) fn description_for(
    descriptions: &BTreeMap<String, FacetRelationship>,
    requested: Option<&[String]>,
) -> Option<String> {
    let selected = selected_facets(descriptions, requested);
    // The one-line summary is a derived convenience, and a concatenated string
    // cannot carry per-part status, so it is built from attached relationships
    // only. Nothing is withheld: every facet is still reported in `facets`, and
    // every detached one is named in `detached_facets`, so a caller wanting a
    // different summary can build it from the reported data.
    let descriptions = selected
        .iter()
        .filter_map(|facet| descriptions.get(facet))
        .filter(|relationship| !relationship.detached)
        .map(|relationship| relationship.description.as_str())
        .filter(|description| !description.is_empty())
        .collect::<Vec<_>>();
    (!descriptions.is_empty()).then(|| descriptions.join(" | "))
}

fn string_field(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{description_for, detached_facets, load_facet_descriptions, selected_facets};
    use crate::resolution::resolve_target;
    use crate::test_support::{journal, write_json};

    /// A detached relationship is REPORTED, not withheld: it stays in `facets`,
    /// it is named in `detached_facets`, and only the derived one-line summary
    /// leaves it out. The owner-facing promise this guards is the detach
    /// confirmation copy: "stays in your journal and you can re-attach it
    /// anytime."
    #[test]
    fn a_detached_relationship_is_reported_with_status_not_filtered_out() {
        let temporary = journal();
        write_json(
            temporary.path(),
            "entities/pat/entity.json",
            json!({"id":"pat","name":"Pat"}),
        );
        for (facet, detached, description) in [
            ("work", false, "Work colleague"),
            ("personal", true, "Old neighbor"),
        ] {
            write_json(
                temporary.path(),
                &format!("facets/{facet}/facet.json"),
                json!({"name":facet}),
            );
            write_json(
                temporary.path(),
                &format!("facets/{facet}/entities/pat/entity.json"),
                json!({"entity_id":"pat","description":description,"detached":detached}),
            );
        }
        let target = resolve_target(temporary.path(), "pat")
            .expect("resolution")
            .expect("target");
        let descriptions =
            load_facet_descriptions(temporary.path(), &target).expect("descriptions");

        // reported, both of them
        assert_eq!(
            selected_facets(&descriptions, None),
            vec!["personal", "work"]
        );
        // and the status says which is which
        assert_eq!(detached_facets(&descriptions), vec!["personal".to_owned()]);
        // the derived summary carries only the attached one
        assert_eq!(
            description_for(&descriptions, None),
            Some("Work colleague".to_owned())
        );
    }

    #[test]
    fn a_detached_relationship_is_still_reported_when_explicitly_requested() {
        let temporary = journal();
        write_json(
            temporary.path(),
            "entities/pat/entity.json",
            json!({"id":"pat","name":"Pat"}),
        );
        write_json(
            temporary.path(),
            "facets/personal/facet.json",
            json!({"name":"personal"}),
        );
        write_json(
            temporary.path(),
            "facets/personal/entities/pat/entity.json",
            json!({"entity_id":"pat","description":"Old neighbor","detached":true}),
        );
        let target = resolve_target(temporary.path(), "pat")
            .expect("resolution")
            .expect("target");
        let descriptions =
            load_facet_descriptions(temporary.path(), &target).expect("descriptions");
        let requested = vec!["personal".to_owned()];

        // asking for it by name still lists it -- the backend withholds nothing
        assert_eq!(
            selected_facets(&descriptions, Some(&requested)),
            vec!["personal"]
        );
        assert_eq!(detached_facets(&descriptions), vec!["personal".to_owned()]);
        // but the unlabelled summary does not silently assert the relationship
        assert_eq!(description_for(&descriptions, Some(&requested)), None);
    }

    #[test]
    fn preserves_requested_csv_order_duplicates_and_blank_descriptions() {
        let temporary = journal();
        write_json(
            temporary.path(),
            "entities/pat/entity.json",
            json!({"id":"pat","name":"Pat"}),
        );
        for (facet, muted, description) in [
            ("work", false, "Work colleague"),
            ("quiet", true, ""),
            ("personal", false, "Neighbor"),
        ] {
            write_json(
                temporary.path(),
                &format!("facets/{facet}/facet.json"),
                json!({"name":facet,"muted":muted}),
            );
            write_json(
                temporary.path(),
                &format!("facets/{facet}/entities/pat/entity.json"),
                json!({"entity_id":"pat","description":description}),
            );
        }
        let target = resolve_target(temporary.path(), "pat")
            .expect("resolution")
            .expect("target");
        let descriptions =
            load_facet_descriptions(temporary.path(), &target).expect("descriptions");
        let requested = vec![
            "personal".to_owned(),
            "missing".to_owned(),
            "work".to_owned(),
            "personal".to_owned(),
            "quiet".to_owned(),
        ];

        assert_eq!(
            selected_facets(&descriptions, Some(&requested)),
            vec!["personal", "work", "personal", "quiet"]
        );
        assert_eq!(
            description_for(&descriptions, Some(&requested)),
            Some("Neighbor | Work colleague | Neighbor".to_owned())
        );
    }
}
