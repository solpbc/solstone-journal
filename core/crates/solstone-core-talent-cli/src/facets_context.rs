// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! `$facets` context: facet declarations plus capped, ranked entity
//! attachments (and a principal role line). Activity lines remain out of
//! scope relative to Python's `facet_summary()` / `facet_summaries()`.

use std::cmp::Reverse;
use std::path::Path;

use serde_json::Value;
use solstone_core_facets::{
    ScopedFacetEntity, list_facet_directories, list_scoped_facet_entities_tolerant,
    load_observations, read_facet_declaration,
};
use solstone_core_journal_config::{is_path_shaped_name, read_journal_config};

const MAX_ENTITIES_PER_FACET: usize = 20;
const MAX_ENTITY_LINE_CHARS: usize = 240;

/// The facet names a talent is allowed to choose from, in the same sense the
/// `$facets` prompt context uses: a facet directory with a readable declaration
/// that is not muted.
///
/// 🔴 Exists because the talent schemas ship a literal `__RUNTIME_FACETS__`
/// placeholder in their `facet` enums and nothing ever replaced it. Measured on the
/// founder's journal 2026-09-01: 55 real facets on disk, and every V2 `sense` run
/// emitted `{"facet": "__RUNTIME_FACETS__"}` because that was the only value its
/// schema permitted. That bogus facet then flowed into `facets.json`, into activity
/// records, and finally into `participation`, which failed 56 times with
/// `facet '__RUNTIME_FACETS__' not found` after running 46,245 times cleanly on V1.
pub(crate) fn enabled_facet_names(journal_root: &Path) -> Vec<String> {
    let Ok(mut facets) = list_facet_directories(journal_root) else {
        return Vec::new();
    };
    facets.sort();
    facets.dedup();
    facets
        .into_iter()
        .filter(|facet| {
            matches!(
                read_facet_declaration(journal_root, facet),
                Ok(Some(declaration)) if declaration.muted != Some(true)
            )
        })
        .collect()
}

/// Replace the `__RUNTIME_FACETS__` placeholder in a talent schema with the owner's
/// real facet names.
///
/// ⚠ When the owner has no enabled facets the enum is **removed** rather than left
/// empty or left holding the placeholder. That matches what the prompt already tells
/// the model in the same situation -- `all_summaries` emits "No facets are defined
/// yet. You are in discovery mode. Name the contexts you observe" -- and a schema
/// that permits no value at all would make discovery mode impossible to satisfy.
pub(crate) fn substitute_runtime_facets(schema: &mut Value, journal_root: &Path) {
    let names = enabled_facet_names(journal_root);
    substitute_runtime_facets_with(schema, &names);
}

fn substitute_runtime_facets_with(schema: &mut Value, names: &[String]) {
    match schema {
        Value::Object(object) => {
            let placeholder = object
                .get("enum")
                .and_then(Value::as_array)
                .is_some_and(|values| {
                    values
                        .iter()
                        .any(|value| value.as_str() == Some(RUNTIME_FACETS_PLACEHOLDER))
                });
            if placeholder {
                if names.is_empty() {
                    object.remove("enum");
                } else {
                    object.insert(
                        "enum".to_owned(),
                        Value::Array(
                            names
                                .iter()
                                .map(|name| Value::String(name.clone()))
                                .collect(),
                        ),
                    );
                }
            }
            for value in object.values_mut() {
                substitute_runtime_facets_with(value, names);
            }
        }
        Value::Array(values) => {
            for value in values {
                substitute_runtime_facets_with(value, names);
            }
        }
        _ => {}
    }
}

/// The token the shipped talent schemas carry in their `facet` enums.
pub(crate) const RUNTIME_FACETS_PLACEHOLDER: &str = "__RUNTIME_FACETS__";

pub(crate) fn resolve_facets(
    journal_root: &Path,
    focused_facet: Option<&str>,
    facet_naming: Option<&str>,
) -> Result<String, String> {
    match focused_facet {
        Some(facet) => focused_summary(journal_root, facet),
        None => Ok(all_summaries(journal_root, facet_naming).unwrap_or_default()),
    }
}

fn focused_summary(journal_root: &Path, facet: &str) -> Result<String, String> {
    let declaration = read_facet_declaration(journal_root, facet)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("facet '{facet}' not found"))?;
    let mut output = format!("## Facet Focus\n# {}", declaration.title);
    if !declaration.color.is_empty() {
        output.push_str(&format!("\n![Color]({})\n", declaration.color));
    }
    if !declaration.description.is_empty() {
        output.push_str(&format!("\n**Description:** {}\n", declaration.description));
    }
    let entities = render_capped_entities(journal_root, facet, true, "");
    if !entities.is_empty() {
        if !output.ends_with('\n') {
            output.push('\n');
        }
        if !output.ends_with("\n\n") {
            output.push('\n');
        }
        output.push_str(&entities);
    }
    Ok(output)
}

fn all_summaries(journal_root: &Path, facet_naming: Option<&str>) -> Result<String, String> {
    let mut facets = list_facet_directories(journal_root).map_err(|error| error.to_string())?;
    facets.sort();
    let mut enabled = Vec::new();
    for facet in facets {
        let declaration = match read_facet_declaration(journal_root, &facet) {
            Ok(Some(declaration)) => declaration,
            Ok(None) | Err(_) => continue,
        };
        if declaration.muted == Some(true) {
            continue;
        }
        enabled.push((facet, declaration));
    }
    if enabled.is_empty() {
        let naming = facet_naming
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_default();
        let naming_sentence = if naming.is_empty() {
            String::new()
        } else {
            format!(" {naming}")
        };
        return Ok(concat!(
            "No facets are defined yet. You are in discovery mode. ",
            "Name the contexts you observe based on what is actually happening ",
            "in this segment."
        )
        .to_owned()
            + &naming_sentence
            + " These names will be used to suggest journal organization to the user.");
    }
    let mut output = String::from("## Available Facets\n");
    for (facet, declaration) in enabled {
        output.push_str(&format!("\n- **{}** (`{facet}`)\n", declaration.title));
        if !declaration.description.is_empty() {
            output.push_str(&format!("  {}\n", declaration.description));
        }
        let entities = render_capped_entities(journal_root, &facet, false, "  ");
        if !entities.is_empty() {
            output.push_str(&entities);
        }
    }
    Ok(output.trim().to_owned())
}

fn render_capped_entities(
    journal_root: &Path,
    facet_dir: &str,
    include_descriptions: bool,
    indent: &str,
) -> String {
    let live = list_scoped_facet_entities_tolerant(journal_root, facet_dir, false, false)
        .unwrap_or_default();
    let mut principals = Vec::new();
    let mut others = Vec::new();
    for entity in live {
        if is_principal_identity(&entity.identity) {
            principals.push(entity);
        } else {
            others.push(entity);
        }
    }
    let mut leftover_principals = principals;
    let role = if include_descriptions {
        let mut role = None;
        leftover_principals.retain(|entity| {
            if role.is_none()
                && let Some(line) = principal_role_line(journal_root, entity, indent)
            {
                role = Some(line);
                return false;
            }
            true
        });
        role
    } else {
        None
    };
    others.extend(leftover_principals);
    let mut ranked = others
        .into_iter()
        .map(|entity| {
            let (count, observed_at) = observation_signal(journal_root, facet_dir, &entity);
            let name = identity_field(&entity.identity, "name").to_lowercase();
            (
                Reverse(count),
                Reverse(observed_at.unwrap_or(0)),
                name,
                entity,
            )
        })
        .collect::<Vec<_>>();
    ranked.sort_by(|left, right| (left.0, left.1, &left.2).cmp(&(right.0, right.1, &right.2)));
    let omitted = ranked.len().saturating_sub(MAX_ENTITIES_PER_FACET);
    ranked.truncate(MAX_ENTITIES_PER_FACET);
    if role.is_none() && ranked.is_empty() {
        return String::new();
    }
    let mut output = String::new();
    if let Some(role) = role {
        output.push_str(&role);
        output.push('\n');
    }
    if !ranked.is_empty() {
        if include_descriptions {
            if !output.is_empty() {
                output.push('\n');
            }
            output.push_str("## Entities\n\n");
        }
        for (_, _, _, entity) in &ranked {
            output.push_str(&format_entity_line(entity, include_descriptions, indent));
            output.push('\n');
        }
        if omitted > 0 {
            output.push_str(&format!("{indent}- _and {omitted} more entities_\n"));
        }
    }
    output
}

fn is_principal_identity(identity: &Value) -> bool {
    identity.get("is_principal").is_some_and(value_is_truthy)
}

fn value_is_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(value) => *value,
        Value::Number(value) => value.as_f64().is_some_and(|value| value != 0.0),
        Value::String(value) => !value.is_empty(),
        Value::Array(value) => !value.is_empty(),
        Value::Object(value) => !value.is_empty(),
    }
}

fn owner_display_name(journal_root: &Path) -> Option<String> {
    let read = read_journal_config(journal_root).ok()?;
    let identity = read.config.as_ref()?.get("identity")?.as_object()?;
    usable_identity_label(identity.get("preferred"))
        .or_else(|| usable_identity_label(identity.get("name")))
}

fn usable_identity_label(value: Option<&Value>) -> Option<String> {
    let value = value.and_then(Value::as_str)?;
    if value.trim().is_empty() || is_path_shaped_name(value) {
        None
    } else {
        Some(value.trim().to_owned())
    }
}

fn principal_role_line(
    journal_root: &Path,
    principal: &ScopedFacetEntity,
    indent: &str,
) -> Option<String> {
    let display = owner_display_name(journal_root)?;
    let description = principal
        .relationship
        .get("description")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    Some(format!("{indent}**{display}'s Role**: {description}"))
}

fn observation_signal(
    journal_root: &Path,
    facet_dir: &str,
    entity: &ScopedFacetEntity,
) -> (usize, Option<i64>) {
    let observations =
        load_observations(journal_root, facet_dir, &entity.relationship_dir).unwrap_or_default();
    let max_observed_at = observations.iter().filter_map(parse_observed_at).max();
    (observations.len(), max_observed_at)
}

fn parse_observed_at(observation: &Value) -> Option<i64> {
    match observation.get("observed_at") {
        Some(Value::Number(value)) => value
            .as_i64()
            .or_else(|| value.as_f64().map(|value| value as i64)),
        Some(Value::String(value)) => value.parse().ok(),
        _ => None,
    }
}

fn identity_field(identity: &Value, key: &str) -> String {
    identity
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

fn display_name(identity: &Value) -> String {
    let name = identity_field(identity, "name");
    let aka = identity
        .get("aka")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    if aka.is_empty() {
        name
    } else {
        format!("{name} ({})", aka.join(", "))
    }
}

fn format_entity_line(
    entity: &ScopedFacetEntity,
    include_descriptions: bool,
    indent: &str,
) -> String {
    if !include_descriptions {
        return format!("{indent}- {}", identity_field(&entity.identity, "name"));
    }
    let type_name = identity_field(&entity.identity, "type");
    let prefix = format!("- **{type_name}**: {}", display_name(&entity.identity));
    let description = entity
        .relationship
        .get("description")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("");
    if description.is_empty() {
        format!("{indent}{prefix}")
    } else {
        format!("{indent}{}", truncate_to_line_budget(&prefix, description))
    }
}

fn truncate_to_line_budget(prefix: &str, description: &str) -> String {
    let separator = " - ";
    let used = prefix.chars().count() + separator.chars().count();
    let budget = MAX_ENTITY_LINE_CHARS.saturating_sub(used);
    let description = description.chars().take(budget).collect::<String>();
    format!("{prefix}{separator}{description}")
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::symlink;

    use super::*;
    use serde_json::json;

    fn declaration(root: &Path, facet: &str, content: &str) {
        let directory = root.join("facets").join(facet);
        fs::create_dir_all(&directory).expect("facet directory");
        fs::write(directory.join("facet.json"), content).expect("declaration");
    }

    fn journal_identity(root: &Path, entity_dir: &str, content: &str) {
        let directory = root.join("entities").join(entity_dir);
        fs::create_dir_all(&directory).expect("entity directory");
        fs::write(directory.join("entity.json"), content).expect("identity");
    }

    fn facet_link(root: &Path, facet: &str, relationship_dir: &str, content: &str) {
        let directory = root
            .join("facets")
            .join(facet)
            .join("entities")
            .join(relationship_dir);
        fs::create_dir_all(&directory).expect("relationship directory");
        fs::write(directory.join("entity.json"), content).expect("link");
    }

    fn observations(root: &Path, facet: &str, relationship_dir: &str, content: &str) {
        let directory = root
            .join("facets")
            .join(facet)
            .join("entities")
            .join(relationship_dir);
        fs::create_dir_all(&directory).expect("relationship directory");
        fs::write(directory.join("observations.jsonl"), content).expect("observations");
    }

    fn attach(
        root: &Path,
        facet: &str,
        journal_dir: &str,
        relationship_dir: &str,
        identity: &str,
        relationship: &str,
    ) {
        journal_identity(root, journal_dir, identity);
        facet_link(root, facet, relationship_dir, relationship);
    }

    fn observation_lines(count: usize, observed_at: i64) -> String {
        (0..count)
            .map(|_| format!(r#"{{"content":"x","observed_at":{observed_at}}}"#))
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn focused_entity_lines(rendered: &str) -> Vec<&str> {
        rendered
            .lines()
            .filter(|line| {
                let trimmed = line.trim_start();
                trimmed.starts_with("- **") && trimmed.contains("**:")
            })
            .collect()
    }

    fn ac2_low_count_names() -> [&'static str; 5] {
        ["aaa", "bbb", "ccc", "ddd", "eee"]
    }

    fn ac2_high_count_names() -> Vec<String> {
        (0..MAX_ENTITIES_PER_FACET)
            .map(|index| format!("u{index:02}"))
            .collect()
    }

    fn write_ac2_entities(root: &Path, facet: &str) {
        for (offset, name) in ac2_low_count_names().into_iter().enumerate() {
            let count = offset + 1;
            let journal_dir = format!("journal_{name}");
            let relationship_dir = format!("link_{name}");
            attach(
                root,
                facet,
                &journal_dir,
                &relationship_dir,
                &format!(r#"{{"id":"{name}","name":"{name}","type":"Person"}}"#),
                &format!(r#"{{"entity_id":"{name}","description":"low-{name}"}}"#),
            );
            observations(
                root,
                facet,
                &relationship_dir,
                &observation_lines(count, 1_000),
            );
        }
        for (offset, name) in ac2_high_count_names().into_iter().enumerate() {
            let count = offset + 6;
            let journal_dir = format!("journal_{name}");
            let relationship_dir = format!("link_{name}");
            attach(
                root,
                facet,
                &journal_dir,
                &relationship_dir,
                &format!(r#"{{"id":"{name}","name":"{name}","type":"Person"}}"#),
                &format!(r#"{{"entity_id":"{name}","description":"high-{name}"}}"#),
            );
            observations(
                root,
                facet,
                &relationship_dir,
                &observation_lines(count, 1_000),
            );
        }
    }

    #[test]
    fn named_all_and_discovery_branches_render_declarations() {
        let root = tempfile::tempdir().expect("root");
        declaration(
            root.path(),
            "work",
            r##"{"title":"Work","description":"Projects","color":"#123"}"##,
        );
        declaration(root.path(), "muted", r#"{"title":"Muted","muted":true}"#);
        let named = resolve_facets(root.path(), Some("work"), None).expect("named");
        assert!(named.contains("## Facet Focus\n# Work"));
        assert!(named.contains("![Color](#123)"));
        assert!(named.contains("**Description:** Projects"));
        let all = resolve_facets(root.path(), None, None).expect("all");
        assert!(all.contains("**Work** (`work`)"));
        assert!(!all.contains("Muted"));

        let empty = tempfile::tempdir().expect("empty root");
        let discovery =
            resolve_facets(empty.path(), None, Some("Use clear names.")).expect("discovery");
        assert!(discovery.contains("No facets are defined yet."));
        assert!(discovery.contains("Use clear names."));
    }

    #[test]
    fn branch_failures_are_swallowed() {
        let root = tempfile::tempdir().expect("root");
        assert_eq!(
            resolve_facets(root.path(), Some("missing"), None),
            Err("facet 'missing' not found".to_owned())
        );
        declaration(root.path(), "bad", "{");
        assert!(
            resolve_facets(root.path(), None, None)
                .expect("discovery after bad declaration")
                .contains("No facets are defined yet.")
        );
        declaration(root.path(), "work", r#"{"title":"Work"}"#);
        let all = resolve_facets(root.path(), None, None).expect("all");
        assert!(all.contains("**Work** (`work`)"));
        assert!(!all.contains("bad"));
        let blocked = root.path().join("facets");
        fs::remove_dir_all(&blocked).expect("remove directory");
        symlink("facets", &blocked).expect("self-referential facets path");
        assert_eq!(
            resolve_facets(root.path(), None, None).expect("self-referential swallow"),
            ""
        );
    }

    /// The placeholder must become the owner's real facets.
    ///
    /// Measured on the founder's journal: 55 facets on disk, and every V2 `sense` run
    /// still emitted `{"facet": "__RUNTIME_FACETS__"}` because that was the only value
    /// its schema allowed.
    #[test]
    fn runtime_facets_placeholder_becomes_the_owners_facets() {
        let mut schema = json!({
            "properties": {
                "facets": {
                    "items": {
                        "properties": {
                            "facet": {"type": "string", "enum": ["__RUNTIME_FACETS__"]},
                            "level": {"type": "string", "enum": ["high", "low"]}
                        }
                    }
                }
            }
        });
        super::substitute_runtime_facets_with(
            &mut schema,
            &["awareness".to_owned(), "ceo".to_owned()],
        );
        assert_eq!(
            schema["properties"]["facets"]["items"]["properties"]["facet"]["enum"],
            json!(["awareness", "ceo"])
        );
        // 🔒 An unrelated enum must be left exactly as it was.
        assert_eq!(
            schema["properties"]["facets"]["items"]["properties"]["level"]["enum"],
            json!(["high", "low"])
        );
    }

    /// ⚠ With no facets the enum is REMOVED, not emptied.
    ///
    /// `all_summaries` tells the model "No facets are defined yet. You are in discovery
    /// mode. Name the contexts you observe" in exactly this case, and an empty enum
    /// would permit no value at all -- making that instruction impossible to satisfy.
    #[test]
    fn discovery_mode_drops_the_facet_enum_entirely() {
        let mut schema = json!({"facet": {"type": "string", "enum": ["__RUNTIME_FACETS__"]}});
        super::substitute_runtime_facets_with(&mut schema, &[]);
        assert!(
            schema["facet"].get("enum").is_none(),
            "an empty enum permits nothing; discovery mode needs a free string"
        );
        assert_eq!(schema["facet"]["type"], json!("string"));
    }

    /// 🔒 Negative twin: a schema without the placeholder is untouched.
    #[test]
    fn a_schema_without_the_placeholder_is_unchanged() {
        let original = json!({
            "properties": {
                "kind": {"type": "string", "enum": ["meeting", "call"]},
                "note": {"type": "string"}
            }
        });
        let mut schema = original.clone();
        super::substitute_runtime_facets_with(&mut schema, &["awareness".to_owned()]);
        assert_eq!(schema, original);
    }

    #[test]
    fn all_summaries_skips_bad_declarations_without_hiding_good_facets() {
        let root = tempfile::tempdir().expect("root");
        declaration(root.path(), "bad", "{");
        declaration(root.path(), "work", r#"{"title":"Work"}"#);
        let all = resolve_facets(root.path(), None, None).expect("all");
        assert!(all.contains("**Work** (`work`)"));
        assert!(!all.contains("bad"));
    }

    // AC1 — focused cap and omitted-count.
    #[test]
    fn ac1_focused_caps_25_entities_and_omits_five() {
        let root = tempfile::tempdir().expect("root");
        declaration(root.path(), "work", r#"{"title":"Work"}"#);
        let long_description = "D".repeat(MAX_ENTITY_LINE_CHARS + 10);
        for index in 1..=25 {
            let name = format!("entity{index:02}");
            attach(
                root.path(),
                "work",
                &format!("journal_{name}"),
                &format!("link_{name}"),
                &format!(r#"{{"id":"{name}","name":"{name}","type":"Person"}}"#),
                &format!(r#"{{"entity_id":"{name}","description":"{long_description}"}}"#),
            );
        }
        let rendered = resolve_facets(root.path(), Some("work"), None).expect("focused");
        let lines = focused_entity_lines(&rendered);
        assert!(
            !lines.is_empty(),
            "expected entity lines, got declaration-only render: {rendered}"
        );
        assert!(
            lines.len() <= MAX_ENTITIES_PER_FACET,
            "kept {} entity lines",
            lines.len()
        );
        assert!(
            rendered.contains(&format!(
                "_and {} more entities_",
                25 - MAX_ENTITIES_PER_FACET
            )),
            "missing omitted-count: {rendered}"
        );
        for line in &lines {
            assert!(
                line.chars().count() <= MAX_ENTITY_LINE_CHARS,
                "line exceeded {} chars: {}",
                MAX_ENTITY_LINE_CHARS,
                line.chars().count()
            );
        }
    }

    // AC2 draft — observations live under relationship dirs, not journal dirs.
    #[test]
    fn ac2_ranking_uses_relationship_dir_observations() {
        let root = tempfile::tempdir().expect("root");
        declaration(root.path(), "work", r#"{"title":"Work"}"#);
        write_ac2_entities(root.path(), "work");
        let rendered = resolve_facets(root.path(), Some("work"), None).expect("focused");
        let lines = focused_entity_lines(&rendered);
        assert!(
            !lines.is_empty(),
            "expected entity lines, got declaration-only render: {rendered}"
        );
        assert!(
            lines.len() <= MAX_ENTITIES_PER_FACET,
            "kept {} entity lines",
            lines.len()
        );
        for name in ac2_low_count_names() {
            assert!(
                !rendered.contains(name),
                "low-count name {name} should be omitted: {rendered}"
            );
        }
        let kept_descriptions: Vec<_> = lines
            .iter()
            .filter_map(|line| line.rsplit_once(" - ").map(|(_, desc)| desc))
            .collect();
        assert!(
            kept_descriptions.len() >= 2 && kept_descriptions[0] != kept_descriptions[1],
            "expected two kept descriptions to differ: {kept_descriptions:?}"
        );
    }

    // AC6 draft — corrupt sibling relationship must not fail compose.
    #[test]
    fn ac6_corrupt_sibling_relationship_is_skipped() {
        let root = tempfile::tempdir().expect("root");
        declaration(root.path(), "work", r#"{"title":"Work"}"#);
        attach(
            root.path(),
            "work",
            "journal_readable",
            "link_readable",
            r#"{"id":"readable","name":"Readable","type":"Person"}"#,
            r#"{"entity_id":"readable","description":"ok"}"#,
        );
        attach(
            root.path(),
            "work",
            "journal_broken",
            "link_broken",
            r#"{"id":"broken","name":"Broken","type":"Person"}"#,
            "{",
        );
        let rendered = resolve_facets(root.path(), Some("work"), None).expect("focused");
        assert!(
            rendered.contains("## Facet Focus\n# Work"),
            "declaration missing: {rendered}"
        );
        assert!(
            rendered.contains("Readable"),
            "readable sibling missing: {rendered}"
        );
    }

    // AC3 — recency, then case-insensitive name.
    #[test]
    fn ac3_ranks_by_recency_then_lowercased_name() {
        let root = tempfile::tempdir().expect("root");
        declaration(root.path(), "work", r#"{"title":"Work"}"#);
        for (name, observed_at) in [
            ("newest", 300),
            ("middle", 200),
            ("oldest", 100),
            ("A", 50),
            ("b", 50),
        ] {
            attach(
                root.path(),
                "work",
                &format!("journal_{name}"),
                &format!("link_{name}"),
                &format!(r#"{{"id":"{name}","name":"{name}","type":"Person"}}"#),
                &format!(r#"{{"entity_id":"{name}","description":"{name}"}}"#),
            );
            observations(
                root.path(),
                "work",
                &format!("link_{name}"),
                &observation_lines(2, observed_at),
            );
        }
        let rendered = resolve_facets(root.path(), Some("work"), None).expect("focused");
        let names = focused_entity_lines(&rendered)
            .into_iter()
            .filter_map(|line| {
                line.split_once("**: ")
                    .map(|(_, rest)| rest.split(" - ").next().unwrap_or(rest))
            })
            .collect::<Vec<_>>();
        assert_eq!(names, ["newest", "middle", "oldest", "A", "b"]);
    }

    // AC4 — under the cap, no omitted-count line.
    #[test]
    fn ac4_under_cap_lists_every_name_without_omitted_count() {
        let root = tempfile::tempdir().expect("root");
        declaration(root.path(), "work", r#"{"title":"Work"}"#);
        for name in ["Ada", "Bea", "Cyd"] {
            attach(
                root.path(),
                "work",
                &format!("journal_{name}"),
                &format!("link_{name}"),
                &format!(r#"{{"id":"{name}","name":"{name}","type":"Person"}}"#),
                &format!(r#"{{"entity_id":"{name}","description":"{name}"}}"#),
            );
        }
        let rendered = resolve_facets(root.path(), Some("work"), None).expect("focused");
        assert!(rendered.contains("Ada"));
        assert!(rendered.contains("Bea"));
        assert!(rendered.contains("Cyd"));
        assert!(!rendered.contains("more entities_"));
    }

    // AC5 — detached and blocked attachments are excluded.
    #[test]
    fn ac5_excludes_detached_and_blocked_entities() {
        let root = tempfile::tempdir().expect("root");
        declaration(root.path(), "work", r#"{"title":"Work"}"#);
        attach(
            root.path(),
            "work",
            "journal_live",
            "link_live",
            r#"{"id":"live","name":"Live","type":"Person"}"#,
            r#"{"entity_id":"live","description":"ok"}"#,
        );
        attach(
            root.path(),
            "work",
            "journal_detached",
            "link_detached",
            r#"{"id":"detached","name":"Detached","type":"Person"}"#,
            r#"{"entity_id":"detached","detached":true,"description":"gone"}"#,
        );
        attach(
            root.path(),
            "work",
            "journal_blocked",
            "link_blocked",
            r#"{"id":"blocked","name":"Blocked","type":"Person","blocked":true}"#,
            r#"{"entity_id":"blocked","description":"no"}"#,
        );
        let rendered = resolve_facets(root.path(), Some("work"), None).expect("focused");
        assert!(rendered.contains("Live"));
        assert!(!rendered.contains("Detached"));
        assert!(!rendered.contains("Blocked"));
    }

    // AC7 — missing facet.json still errors with the facet name.
    #[test]
    fn ac7_missing_declaration_still_errors_with_facet_name() {
        let root = tempfile::tempdir().expect("root");
        fs::create_dir_all(root.path().join("facets").join("ghost")).expect("facet directory");
        let error =
            resolve_facets(root.path(), Some("ghost"), None).expect_err("missing declaration");
        assert!(error.contains("ghost"), "{error}");
    }

    // AC8 — principal role line uses preferred name and is excluded from the list.
    #[test]
    fn ac8_principal_role_line_excludes_principal_from_entity_list() {
        let root = tempfile::tempdir().expect("root");
        declaration(root.path(), "work", r#"{"title":"Work"}"#);
        fs::create_dir_all(root.path().join("config")).expect("config");
        fs::write(
            root.path().join("config/journal.json"),
            r#"{"identity":{"preferred":"Soleil","name":"Owner"}}"#,
        )
        .expect("config");
        attach(
            root.path(),
            "work",
            "journal_jer",
            "link_jer",
            r#"{"id":"jer","name":"Jer","type":"Person","is_principal":true}"#,
            r#"{"entity_id":"jer","description":"founder"}"#,
        );
        attach(
            root.path(),
            "work",
            "journal_ada",
            "link_ada",
            r#"{"id":"ada","name":"Ada","type":"Person"}"#,
            r#"{"entity_id":"ada","description":"colleague"}"#,
        );
        attach(
            root.path(),
            "work",
            "journal_moe",
            "link_moe",
            r#"{"id":"moe","name":"Moe","type":"Person"}"#,
            r#"{"entity_id":"moe","description":"partner"}"#,
        );
        let rendered = resolve_facets(root.path(), Some("work"), None).expect("focused");
        assert!(rendered.contains("Soleil"));
        assert!(rendered.contains("founder"));
        let list = focused_entity_lines(&rendered).join("\n");
        assert!(list.contains("Ada"), "{list}");
        assert!(list.contains("Moe"), "{list}");
        assert!(!list.contains("Jer"), "{list}");
        assert!(!list.contains("Soleil"), "{list}");
    }

    #[test]
    fn principal_without_role_line_stays_in_the_entity_list() {
        let root = tempfile::tempdir().expect("root");
        declaration(root.path(), "work", r#"{"title":"Work"}"#);
        attach(
            root.path(),
            "work",
            "journal_jer",
            "link_jer",
            r#"{"id":"jer","name":"Jer","type":"Person","is_principal":true}"#,
            r#"{"entity_id":"jer","description":"founder"}"#,
        );
        attach(
            root.path(),
            "work",
            "journal_ada",
            "link_ada",
            r#"{"id":"ada","name":"Ada","type":"Person"}"#,
            r#"{"entity_id":"ada","description":"colleague"}"#,
        );
        let rendered = resolve_facets(root.path(), Some("work"), None).expect("focused");
        let list = focused_entity_lines(&rendered).join("\n");
        assert!(
            !rendered.contains("'s Role"),
            "role line should not form without a usable preferred/name: {rendered}"
        );
        assert!(list.contains("Jer"), "principal vanished: {list}");
        assert!(list.contains("Ada"), "{list}");
    }

    // AC9 — all-facets path is names-only and uses the same cap.
    #[test]
    fn ac9_all_facets_is_names_only_and_caps() {
        let root = tempfile::tempdir().expect("root");
        declaration(root.path(), "work", r#"{"title":"Work"}"#);
        declaration(root.path(), "home", r#"{"title":"Home"}"#);
        write_ac2_entities(root.path(), "work");
        attach(
            root.path(),
            "home",
            "journal_kit",
            "link_kit",
            r#"{"id":"kit","name":"Kit","type":"Person"}"#,
            r#"{"entity_id":"kit","description":"should-not-appear"}"#,
        );
        let rendered = resolve_facets(root.path(), None, None).expect("all");
        let work_block = rendered
            .split_once("- **Work**")
            .map(|(_, rest)| rest)
            .expect("work block");
        let name_lines = work_block
            .lines()
            .filter(|line| {
                let trimmed = line.trim_start();
                trimmed.starts_with("- ") && !trimmed.starts_with("- **Work**")
            })
            .collect::<Vec<_>>();
        let names = name_lines
            .iter()
            .filter(|line| !line.contains("more entities_"))
            .collect::<Vec<_>>();
        assert!(
            names.len() <= MAX_ENTITIES_PER_FACET,
            "kept {} names: {names:?}",
            names.len()
        );
        assert!(
            work_block.contains(&format!(
                "_and {} more entities_",
                25 - MAX_ENTITIES_PER_FACET
            )),
            "{work_block}"
        );
        for name in ac2_low_count_names() {
            assert!(
                !work_block.contains(name),
                "low-count {name} in {work_block}"
            );
        }
        assert!(!work_block.contains("high-"));
        assert!(!work_block.contains("low-"));
        assert!(!work_block.contains("should-not-appear"));
        assert!(rendered.contains("Kit"));
    }

    #[test]
    fn all_facets_omits_principal_role_description() {
        let root = tempfile::tempdir().expect("root");
        declaration(root.path(), "work", r#"{"title":"Work"}"#);
        fs::create_dir_all(root.path().join("config")).expect("config");
        fs::write(
            root.path().join("config/journal.json"),
            r#"{"identity":{"preferred":"Soleil"}}"#,
        )
        .expect("config");
        attach(
            root.path(),
            "work",
            "journal_jer",
            "link_jer",
            r#"{"id":"jer","name":"Jer","type":"Person","is_principal":true}"#,
            r#"{"entity_id":"jer","description":"ROLE_DESC_MARKER"}"#,
        );
        attach(
            root.path(),
            "work",
            "journal_ada",
            "link_ada",
            r#"{"id":"ada","name":"Ada","type":"Person"}"#,
            r#"{"entity_id":"ada","description":"colleague"}"#,
        );
        let rendered = resolve_facets(root.path(), None, None).expect("all");
        assert!(
            !rendered.contains("ROLE_DESC_MARKER"),
            "principal role description leaked into names-only output: {rendered}"
        );
        assert!(rendered.contains("Ada"), "{rendered}");
        assert!(
            rendered.contains("Jer"),
            "principal should stay in the names-only roster when no role line is rendered: {rendered}"
        );
    }

    // AC10 — line ceiling keeps the token estimate under budget.
    #[test]
    fn ac10_line_ceiling_keeps_token_estimate_under_budget() {
        let root = tempfile::tempdir().expect("root");
        declaration(root.path(), "work", r#"{"title":"Work"}"#);
        let description = "D".repeat(2_000);
        let description_chars = description.chars().count();
        for index in 1..=21 {
            let name = format!("E{index:02}");
            let aka = format!("A{index:02}");
            let prefix = format!("- **Person**: {name} ({aka})");
            assert!(
                prefix.chars().count() < MAX_ENTITY_LINE_CHARS,
                "prefix {} was not under the line ceiling",
                prefix.chars().count()
            );
            assert!(
                prefix.chars().count() + " - ".chars().count() + description_chars >= 2_000,
                "uncapped line was shorter than 2000 chars"
            );
            attach(
                root.path(),
                "work",
                &format!("journal_{name}"),
                &format!("link_{name}"),
                &format!(r#"{{"id":"{name}","name":"{name}","type":"Person","aka":["{aka}"]}}"#),
                &format!(r#"{{"entity_id":"{name}","description":"{description}"}}"#),
            );
        }
        let production = resolve_facets(root.path(), Some("work"), None).expect("focused");
        let suffix = "x".repeat(3_396);
        let production_chars = production.chars().count() + suffix.chars().count();
        assert!(
            production_chars.div_ceil(3) < 12_032,
            "capped estimate {} was not under budget",
            production_chars.div_ceil(3)
        );
        let lines = focused_entity_lines(&production);
        assert!(
            !lines.is_empty(),
            "expected entity lines, got declaration-only render: {production}"
        );
        for line in &lines {
            assert!(
                line.chars().count() <= MAX_ENTITY_LINE_CHARS,
                "line exceeded {} chars: {}",
                MAX_ENTITY_LINE_CHARS,
                line.chars().count()
            );
        }
        let uncapped = production
            .lines()
            .map(|line| {
                if line.trim_start().starts_with("- **") && line.contains("**:") {
                    match line.split_once(" - ") {
                        Some((prefix, _desc)) => format!("{prefix} - {description}"),
                        None => format!("{line} - {description}"),
                    }
                } else {
                    line.to_owned()
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        let uncapped_chars = uncapped.chars().count() + suffix.chars().count();
        assert!(
            uncapped_chars.div_ceil(3) >= 12_032,
            "uncapped estimate {} should exceed budget; production had {} entity lines",
            uncapped_chars.div_ceil(3),
            lines.len()
        );
    }
}
