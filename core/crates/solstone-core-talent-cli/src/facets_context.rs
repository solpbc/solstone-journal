// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Declaration-level `$facets` context only. Entity attachments, observations,
//! and activity lines are deliberately omitted from this port; this is the
//! documented scoped-down fidelity gap from Python's `facet_summary()` and
//! `facet_summaries()`.

use std::path::Path;

use solstone_core_facets::{list_facet_directories, read_facet_declaration};

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
    }
    Ok(output.trim().to_owned())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::symlink;

    use super::*;

    fn declaration(root: &Path, facet: &str, content: &str) {
        let directory = root.join("facets").join(facet);
        fs::create_dir_all(&directory).expect("facet directory");
        fs::write(directory.join("facet.json"), content).expect("declaration");
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

    #[test]
    fn all_summaries_skips_bad_declarations_without_hiding_good_facets() {
        let root = tempfile::tempdir().expect("root");
        declaration(root.path(), "bad", "{");
        declaration(root.path(), "work", r#"{"title":"Work"}"#);
        let all = resolve_facets(root.path(), None, None).expect("all");
        assert!(all.contains("**Work** (`work`)"));
        assert!(!all.contains("bad"));
    }
}
