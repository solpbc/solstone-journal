// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The days each detected name appears on, across every facet.
//!
//! A hide in any facet keeps a name out everywhere here: this read crosses
//! facets, so a name the owner hid in one must not come back through another.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde_json::Value;
use solstone_core_entity::hidden_names::{HiddenKeys, facet_hidden_keys, hidden_name_key};
use solstone_core_entity_matching::{EntityNameCandidate, ExclusionKeys};
use solstone_core_journal_io::{DirEntryKind, contained_path, list_dir_entries};

use super::declaration::observe_facet_declaration;
use super::detected_entities::read_detected_entities;
use super::detected_entity_activity::detected_days;
use super::error::{FacetEntityWriteError, FacetStoreError};
use super::map::{list_declared_facet_names, list_facet_directories};

/// One detected name and the days it was found on, oldest first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetectedNameDays {
    pub entity_type: String,
    /// The name as the first row read in the range spells it.
    pub name: String,
    pub days: Vec<String>,
}

/// Every detected name with a day in `from_day..=to_day`, across the facets
/// that aren't muted, in type then name order.
///
/// Left out: a name any facet hides (by the owner, or through a hidden
/// entity), anything that names a blocked entity or the journal's principal
/// (name, alias, email or id, as detections are matched elsewhere), and
/// `also_left_out`, for names a caller knows the owner hid somewhere else.
/// Rows that differ only in case or spacing are one name, whatever the case
/// of their type. When any facet's hides, any entity's identity or a facet's
/// day list can't be read, nothing is listed and the error says so.
pub fn detected_name_days(
    journal_root: &Path,
    from_day: &str,
    to_day: &str,
    also_left_out: &[String],
) -> Result<Vec<DetectedNameDays>, FacetEntityWriteError> {
    // Hides are read from every facet folder, muted or undeclared included.
    let mut hides: Vec<HiddenKeys> = Vec::new();
    for facet in list_facet_directories(journal_root)? {
        hides.push(facet_hidden_keys(journal_root, &facet)?);
    }
    // An identity that is there and can't be read might be the principal or
    // a blocked entity: nothing is listed until it can be.
    require_readable_identities(journal_root)?;
    let mut left_out = ExclusionKeys::default();
    for entity in solstone_core_entity::every_journal_entity(journal_root)? {
        if entity.is_blocked() || entity.is_principal() {
            let names = entity.resolution_entity();
            left_out.add_candidate(&EntityNameCandidate {
                id: names.id,
                name: names.name,
                aka: names.aka,
                emails: names.emails,
            });
        }
    }
    for name in also_left_out {
        left_out.add_candidate(&EntityNameCandidate {
            id: None,
            name: name.clone(),
            aka: Vec::new(),
            emails: Vec::new(),
        });
    }

    let mut found: BTreeMap<(String, String), (String, String, BTreeSet<String>)> = BTreeMap::new();
    for facet in list_declared_facet_names(journal_root)? {
        let muted = observe_facet_declaration(journal_root, &facet)?
            .is_some_and(|declaration| declaration.muted == Some(true));
        if muted {
            continue;
        }
        for day in detected_days(journal_root, &facet)? {
            if day.as_str() < from_day || day.as_str() > to_day {
                continue;
            }
            for row in read_detected_entities(journal_root, &facet, &day)? {
                let name = row
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .trim();
                if name.is_empty() {
                    continue;
                }
                if left_out.contains(name) || hides.iter().any(|keys| keys.is_hidden(name)) {
                    continue;
                }
                let entity_type = row
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                found
                    .entry((entity_type.to_lowercase(), hidden_name_key(name)))
                    .or_insert_with(|| (entity_type, name.to_owned(), BTreeSet::new()))
                    .2
                    .insert(day.clone());
            }
        }
    }
    Ok(found
        .into_iter()
        .map(|(_, (entity_type, name, days))| DetectedNameDays {
            entity_type,
            name,
            days: days.into_iter().collect(),
        })
        .collect())
}

/// Refuses when a folder under `entities/` holds an identity that is there
/// and can't be read as one.
fn require_readable_identities(journal_root: &Path) -> Result<(), FacetEntityWriteError> {
    use solstone_core_entity::IdentityObservation;

    let directory = contained_path(journal_root, "entities")
        .map_err(|error| FacetEntityWriteError::FacetStore(error.into()))?;
    if !directory.is_dir() {
        return Ok(());
    }
    for entry in list_dir_entries(&directory).map_err(FacetStoreError::from)? {
        if entry.kind != DirEntryKind::Directory {
            continue;
        }
        let folder = entry.name.to_string_lossy().into_owned();
        match solstone_core_entity::observe_entity_identity(journal_root, &folder)? {
            IdentityObservation::Present(_) | IdentityObservation::Absent => {}
            IdentityObservation::Malformed(_) | IdentityObservation::Unreadable(_) => {
                return Err(FacetEntityWriteError::IdentityMapLoser {
                    entity_id: folder.clone(),
                    entity_dir: folder,
                });
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use super::detected_name_days;
    use crate::{create_facet, hide_detected_name, set_facet_muted};

    fn facet(root: &Path, name: &str) {
        create_facet(root, name, name, "", "", "", None).unwrap();
    }

    fn detect(root: &Path, facet: &str, day: &str, rows: &[(&str, &str)]) {
        let folder = root.join("facets").join(facet).join("entities");
        fs::create_dir_all(&folder).unwrap();
        let lines: String = rows
            .iter()
            .map(|(entity_type, name)| {
                format!(
                    "{}\n",
                    serde_json::json!({"type": entity_type, "name": name, "description": ""})
                )
            })
            .collect();
        fs::write(folder.join(format!("{day}.jsonl")), lines).unwrap();
    }

    fn entity(root: &Path, id: &str, identity: serde_json::Value) {
        let folder = root.join("entities").join(id);
        fs::create_dir_all(&folder).unwrap();
        fs::write(folder.join("entity.json"), identity.to_string()).unwrap();
    }

    fn names(root: &Path) -> Vec<(String, Vec<String>)> {
        detected_name_days(root, "20250101", "20251231", &[])
            .unwrap()
            .into_iter()
            .map(|found| (found.name, found.days))
            .collect()
    }

    #[test]
    fn days_are_merged_across_facets_sorted_and_bounded() {
        let root = tempfile::tempdir().unwrap();
        facet(root.path(), "work");
        facet(root.path(), "home");
        detect(root.path(), "work", "20250310", &[("Person", "Ada Wong")]);
        detect(root.path(), "home", "20250102", &[("Person", "ada  wong")]);
        detect(root.path(), "home", "20250310", &[("Person", "Ada Wong")]);
        detect(root.path(), "home", "20241231", &[("Person", "Ada Wong")]);
        detect(root.path(), "home", "20260101", &[("Person", "Ada Wong")]);
        assert_eq!(
            names(root.path()),
            [(
                "ada  wong".to_owned(),
                vec!["20250102".to_owned(), "20250310".to_owned()]
            )]
        );
    }

    #[test]
    fn a_name_hidden_in_one_facet_is_left_out_of_every_facet() {
        let root = tempfile::tempdir().unwrap();
        facet(root.path(), "work");
        facet(root.path(), "home");
        detect(
            root.path(),
            "work",
            "20250310",
            &[("Person", "Ada Wong"), ("Person", "Ben Okri")],
        );
        detect(root.path(), "home", "20250311", &[("Person", "Ada Wong")]);
        hide_detected_name(root.path(), "work", "Ada Wong").unwrap();
        assert_eq!(
            names(root.path()),
            [("Ben Okri".to_owned(), vec!["20250310".to_owned()])]
        );
    }

    #[test]
    fn a_blocked_entity_and_the_principal_are_left_out_by_name_and_alias() {
        let root = tempfile::tempdir().unwrap();
        facet(root.path(), "work");
        entity(
            root.path(),
            "cy",
            serde_json::json!({"id":"cy","name":"Cy Young","type":"Person","aka":["Cyclone"],"blocked":true}),
        );
        entity(
            root.path(),
            "me",
            serde_json::json!({"id":"me","name":"Dana Owner","type":"Person","aka":["Dee"],"is_principal":true}),
        );
        detect(
            root.path(),
            "work",
            "20250310",
            &[
                ("Person", "Cy Young"),
                ("Person", "Cyclone"),
                ("Person", "Dana Owner"),
                ("Person", "Dee"),
                ("Person", "Ben Okri"),
            ],
        );
        assert_eq!(
            names(root.path()),
            [("Ben Okri".to_owned(), vec!["20250310".to_owned()])]
        );
    }

    #[test]
    fn a_muted_facet_adds_no_days_but_its_hides_still_hold() {
        let root = tempfile::tempdir().unwrap();
        facet(root.path(), "work");
        facet(root.path(), "home");
        detect(
            root.path(),
            "work",
            "20250310",
            &[("Person", "Ada Wong"), ("Person", "Ben Okri")],
        );
        detect(
            root.path(),
            "home",
            "20250311",
            &[("Person", "Ben Okri"), ("Person", "Eve Hart")],
        );
        hide_detected_name(root.path(), "home", "Ada Wong").unwrap();
        set_facet_muted(root.path(), "home", true).unwrap();
        assert_eq!(
            names(root.path()),
            [("Ben Okri".to_owned(), vec!["20250310".to_owned()])]
        );
    }

    #[test]
    fn damaged_hides_list_nothing() {
        let root = tempfile::tempdir().unwrap();
        facet(root.path(), "work");
        facet(root.path(), "home");
        detect(root.path(), "work", "20250310", &[("Person", "Ben Okri")]);
        hide_detected_name(root.path(), "home", "Ada Wong").unwrap();
        let hidden = fs::read_dir(root.path().join("facets/home"))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .find(|path| {
                path.file_name()
                    .is_some_and(|name| name.to_string_lossy().contains("hidden"))
            })
            .expect("the hide wrote a file in the facet");
        fs::write(hidden, "{").unwrap();
        assert!(detected_name_days(root.path(), "20250101", "20251231", &[]).is_err());
    }

    #[test]
    fn a_blocked_entity_is_left_out_by_email_and_id_too() {
        let root = tempfile::tempdir().unwrap();
        facet(root.path(), "work");
        entity(
            root.path(),
            "cy_young",
            serde_json::json!({"id":"cy_young","name":"Cy Young","type":"Person","emails":["cy@example.org"],"blocked":true}),
        );
        detect(
            root.path(),
            "work",
            "20250310",
            &[
                ("Person", "cy@example.org"),
                ("Person", "cy_young"),
                ("Person", "Ben Okri"),
            ],
        );
        assert_eq!(
            names(root.path()),
            [("Ben Okri".to_owned(), vec!["20250310".to_owned()])]
        );
    }

    #[test]
    fn an_identity_that_cannot_be_read_lists_nothing() {
        let root = tempfile::tempdir().unwrap();
        facet(root.path(), "work");
        detect(root.path(), "work", "20250310", &[("Person", "Ben Okri")]);
        let folder = root.path().join("entities/me");
        fs::create_dir_all(&folder).unwrap();
        fs::write(folder.join("entity.json"), "{\"name\":\"Dana Ow").unwrap();
        assert!(detected_name_days(root.path(), "20250101", "20251231", &[]).is_err());
    }

    #[test]
    fn a_name_the_caller_knows_is_hidden_elsewhere_is_left_out() {
        let root = tempfile::tempdir().unwrap();
        facet(root.path(), "work");
        detect(
            root.path(),
            "work",
            "20250310",
            &[("Place", "Blue Door Clinic"), ("Place", "The Bakery")],
        );
        let found = detected_name_days(
            root.path(),
            "20250101",
            "20251231",
            &["blue door clinic".to_owned()],
        )
        .unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "The Bakery");
    }

    #[test]
    fn one_name_under_two_spellings_of_its_type_is_one_row() {
        let root = tempfile::tempdir().unwrap();
        facet(root.path(), "work");
        detect(root.path(), "work", "20250310", &[("Person", "Ben Okri")]);
        detect(root.path(), "work", "20250311", &[("person", "Ben Okri")]);
        assert_eq!(
            names(root.path()),
            [(
                "Ben Okri".to_owned(),
                vec!["20250310".to_owned(), "20250311".to_owned()]
            )]
        );
    }
}
