// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;

use serde_json::{Value, json};
use tempfile::TempDir;

use super::*;

const DAY: &str = "20260110";

/// Save an entity attached to `facets`, with its emails and aliases.
fn save(root: &Path, id: &str, name: &str, facets: &[&str], extra: Value) {
    let mut identity = json!({"id": id, "name": name, "type": "Person"});
    for (key, value) in extra.as_object().unwrap() {
        identity[key] = value.clone();
    }
    let dir = root.join("entities").join(id);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("entity.json"), identity.to_string()).unwrap();
    for facet in facets {
        let link = root.join("facets").join(facet).join("entities").join(id);
        fs::create_dir_all(&link).unwrap();
        fs::write(
            link.join("entity.json"),
            json!({"entity_id": id}).to_string(),
        )
        .unwrap();
    }
}

fn rows(root: &Path) -> Vec<Value> {
    solstone_core_entity::load_merge_candidates(root, None, None).unwrap()
}

/// Each suggestion as `facet:source>target:basis`.
fn suggested(root: &Path) -> BTreeSet<String> {
    rows(root)
        .iter()
        .map(|row| {
            format!(
                "{}:{}>{}:{}",
                row["facet"].as_str().unwrap(),
                row["source_slug"].as_str().unwrap(),
                row["target_slug"].as_str().unwrap(),
                row["evidence"]["basis"].as_str().unwrap()
            )
        })
        .collect()
}

#[test]
fn pairs_across_facets_by_name_email_and_alias_are_suggested_open() {
    let root = TempDir::new().unwrap();
    let root = root.path();
    // A name variant across facets, and one inside a facet (the daily review's).
    save(root, "acme", "Acme", &["work"], json!({"type": "Company"}));
    save(
        root,
        "acme_inc",
        "Acme Inc",
        &["home"],
        json!({"type": "Company"}),
    );
    save(root, "smith_jan", "Smith Jan", &["work"], json!({}));
    save(root, "jan_smith", "Jan Smith", &["work"], json!({}));
    // A first name and a full name narrow, so they are never asked.
    save(root, "dana", "Dana", &["work"], json!({}));
    save(root, "dana_ruiz", "Dana Ruiz", &["home"], json!({}));
    // One email, written differently.
    save(
        root,
        "jo_park",
        "Jo Park",
        &["work"],
        json!({"emails": [" JO@Example.com"]}),
    );
    save(
        root,
        "joanne_park",
        "Joanne Park",
        &["home"],
        json!({"emails": ["jo@example.com"]}),
    );
    // One's alias is the other's name.
    save(
        root,
        "robert_diaz",
        "Robert Diaz",
        &["work"],
        json!({"aka": ["Bobby  D"]}),
    );
    save(root, "bobby_d", "Bobby D", &["work"], json!({}));
    // A short alias is never a reason.
    save(
        root,
        "al_one",
        "Albert One",
        &["work"],
        json!({"aka": ["Al"]}),
    );
    save(
        root,
        "al_two",
        "Alvin Two",
        &["work"],
        json!({"aka": ["Al"]}),
    );

    let report = record_duplicate_candidates(root, DAY).unwrap();
    assert_eq!(
        report,
        DuplicateDiscovery {
            found: 3,
            recorded: 3
        }
    );
    assert_eq!(
        suggested(root),
        BTreeSet::from([
            "work:acme>acme_inc:name-variant".to_owned(),
            "work:jo_park>joanne_park:same-email".to_owned(),
            "work:bobby_d>robert_diaz:shared-alias".to_owned(),
        ])
    );
    for row in rows(root) {
        assert_eq!(row["status"], "open");
        assert_eq!(row["first_surfaced"], DAY);
    }

    // Running again on the same journal adds nothing.
    let again = record_duplicate_candidates(root, "20260111").unwrap();
    assert_eq!(
        again,
        DuplicateDiscovery {
            found: 3,
            recorded: 0
        }
    );
    assert_eq!(rows(root).len(), 3);
}

#[test]
fn an_answered_or_standing_pair_is_never_suggested_again() {
    let root = TempDir::new().unwrap();
    let root = root.path();
    for (id, name) in [("kim_lee", "Kim Lee"), ("kimberly_lee", "Kimberly Lee")] {
        save(
            root,
            id,
            name,
            &["work"],
            json!({"emails": ["kim@example.com"]}),
        );
    }
    for (id, name) in [("pat_ng", "Pat Ng"), ("patricia_ng", "Patricia Ng")] {
        save(
            root,
            id,
            name,
            &["home"],
            json!({"emails": ["pat@example.com"]}),
        );
    }
    for (id, name) in [("sam_oh", "Sam Oh"), ("samuel_oh", "Samuel Oh")] {
        save(
            root,
            id,
            name,
            &["home"],
            json!({"emails": ["sam@example.com"]}),
        );
    }
    // Dismissed in another facet, in the other direction; accepted; open.
    let row = |facet: &str, source: &str, target: &str, status: &str| {
        json!({"facet": facet, "source_slug": source, "target_slug": target,
               "status": status, "evidence": {"basis": "name-variant"}})
        .to_string()
    };
    fs::write(
        root.join("entities/review-candidates.jsonl"),
        [
            row("travel", "kimberly_lee", "kim_lee", "dismissed"),
            row("home", "pat_ng", "patricia_ng", "accepted"),
            row("home", "sam_oh", "samuel_oh", "open"),
        ]
        .join("\n")
            + "\n",
    )
    .unwrap();
    let before = fs::read(root.join("entities/review-candidates.jsonl")).unwrap();

    let report = record_duplicate_candidates(root, DAY).unwrap();
    assert_eq!(
        report,
        DuplicateDiscovery {
            found: 3,
            recorded: 0
        }
    );
    assert_eq!(
        fs::read(root.join("entities/review-candidates.jsonl")).unwrap(),
        before
    );
}

#[test]
fn a_pair_kept_apart_as_speakers_is_not_suggested() {
    let root = TempDir::new().unwrap();
    let root = root.path();
    for (id, name) in [("ivy_wu", "Ivy Wu"), ("ivy_w", "Ivy W")] {
        save(
            root,
            id,
            name,
            &["work"],
            json!({"emails": ["ivy@example.com"]}),
        );
    }
    for (id, name) in [("max_ito", "Max Ito"), ("maxwell_ito", "Maxwell Ito")] {
        save(
            root,
            id,
            name,
            &["work"],
            json!({"emails": ["max@example.com"]}),
        );
    }
    solstone_core_speaker_resolve::keep_separate::record_keep_separate_assertion(
        root, "ivy_wu", "ivy_w", "owner", None, 1,
    )
    .unwrap();
    fs::create_dir_all(root.join("speakers")).unwrap();
    fs::write(
        root.join("speakers/review-candidates.jsonl"),
        json!({"source_id": "maxwell_ito", "target_id": "max_ito", "status": "dismissed"})
            .to_string()
            + "\n",
    )
    .unwrap();

    let report = record_duplicate_candidates(root, DAY).unwrap();
    assert_eq!(
        report,
        DuplicateDiscovery {
            found: 2,
            recorded: 0
        }
    );
    assert!(rows(root).is_empty());
}

#[test]
fn trashed_hidden_and_owner_entities_are_never_paired() {
    let root = TempDir::new().unwrap();
    let root = root.path();
    let shared = json!({"emails": ["one@example.com"]});
    save(root, "lou_ma", "Lou Ma", &["work"], shared.clone());
    // The owner, a hidden entity, and one whose files the trash holds.
    save(
        root,
        "me",
        "Me Myself",
        &["work"],
        json!({"emails": ["one@example.com"], "is_principal": true}),
    );
    save(
        root,
        "lou_hidden",
        "Lou Hidden",
        &["work"],
        json!({"emails": ["one@example.com"], "blocked": true}),
    );
    save(root, "lou_m", "Lou M", &["home"], shared);
    fs::create_dir_all(root.join(".trash/item/files/entities")).unwrap();
    fs::rename(
        root.join("entities/lou_m"),
        root.join(".trash/item/files/entities/lou_m"),
    )
    .unwrap();

    let report = record_duplicate_candidates(root, DAY).unwrap();
    assert_eq!(report, DuplicateDiscovery::default());
    assert!(rows(root).is_empty());
}

#[test]
fn a_pair_with_no_facet_to_file_under_is_left_out() {
    let root = TempDir::new().unwrap();
    let root = root.path();
    save(
        root,
        "eve_li",
        "Eve Li",
        &[],
        json!({"emails": ["eve@example.com"]}),
    );
    save(
        root,
        "evelyn_li",
        "Evelyn Li",
        &[],
        json!({"emails": ["eve@example.com"]}),
    );
    let report = record_duplicate_candidates(root, DAY).unwrap();
    assert_eq!(
        report,
        DuplicateDiscovery {
            found: 1,
            recorded: 0
        }
    );
    assert!(!root.join("entities/review-candidates.jsonl").exists());
}

/// A large registry is compared through its indexes: names that share no word
/// are never compared, a word on too many names is left to the others, and a
/// value too many entities hold is no reason.
#[test]
fn discovery_compares_only_names_that_share_a_word() {
    let root = TempDir::new().unwrap();
    let root = root.path();
    for index in 0..300 {
        let id = format!("member_{index}");
        let facet = if index % 2 == 0 { "work" } else { "home" };
        save(
            root,
            &id,
            &format!("Member {index}"),
            &[facet],
            json!({"emails": ["team@example.com"]}),
        );
    }
    save(
        root,
        "orbit",
        "Orbit",
        &["work"],
        json!({"type": "Project"}),
    );
    save(
        root,
        "orbit_ltd",
        "Orbit Ltd",
        &["home"],
        json!({"type": "Project"}),
    );

    let saved = saved_entities(root).unwrap();
    assert_eq!(saved.len(), 302);
    let (pairs, compared) = find_pairs(&saved);
    let pairs = pairs
        .iter()
        .map(|((left, right), (basis, _))| {
            (saved[*left].id.as_str(), saved[*right].id.as_str(), *basis)
        })
        .collect::<Vec<_>>();
    assert_eq!(pairs.len(), 1, "{pairs:?}");
    let (left, right, basis) = pairs[0];
    assert_eq!(
        BTreeSet::from([left, right]),
        BTreeSet::from(["orbit", "orbit_ltd"])
    );
    assert_eq!(basis, NAME_VARIANT);
    // "member" is on every name, so it groups nothing; only the numbers
    // sharing a leading four digits and the one real pair are compared.
    assert!(compared < 50, "{compared}");
}
