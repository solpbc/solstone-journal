// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Source-path classification for connection-scoped index reads.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::ErrorKind;
use std::path::Path;

use serde_json::Value;
use solstone_core_format::content::{AdmittedCategory, IndexDisposition, ScopeBasis, resolve_spec};
use solstone_core_format::paths::resolve_journal_path;
use solstone_core_format::segment::{segment_key, segment_parse};

use crate::StoreError;
use crate::db::ChunkClassification;

#[derive(Clone, Debug)]
enum FacetResolution {
    Id(String),
    Missing,
    Unreadable,
    Duplicate,
}

/// Exact directory-name to facet-id view used by the indexer.
///
/// A facet rename does not rewrite per-segment `talents/facets.json`: material
/// under the facet directory follows its stable ID, while old segment names
/// stop resolving until the segment is re-sensed.
#[derive(Clone, Debug, Default)]
pub struct FacetDeclarationSet {
    names: BTreeMap<String, FacetResolution>,
}

impl FacetDeclarationSet {
    pub fn from_journal(journal: &Path) -> Result<Self, StoreError> {
        let facets = journal.join("facets");
        let entries = match fs::read_dir(facets) {
            Ok(entries) => entries,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Self::default()),
            Err(error) => return Err(StoreError::Io(error)),
        };
        let mut names = BTreeMap::new();
        for entry in entries {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            let declaration = entry.path().join("facet.json");
            let resolution = match fs::read_to_string(&declaration) {
                Ok(text) => serde_json::from_str::<Value>(&text)
                    .ok()
                    .and_then(|value| value.as_object().cloned())
                    .and_then(|object| object.get("id").and_then(Value::as_str).map(str::to_owned))
                    .filter(|id| is_well_formed_facet_id(id))
                    .map(FacetResolution::Id)
                    .unwrap_or(FacetResolution::Missing),
                Err(error) if error.kind() == ErrorKind::NotFound => FacetResolution::Missing,
                Err(_) => FacetResolution::Unreadable,
            };
            names.insert(name, resolution);
        }

        let mut id_names = BTreeMap::<String, Vec<String>>::new();
        for (name, resolution) in &names {
            if let FacetResolution::Id(id) = resolution {
                id_names.entry(id.clone()).or_default().push(name.clone());
            }
        }
        for duplicate_names in id_names.values().filter(|names| names.len() > 1) {
            for name in duplicate_names {
                names.insert(name.clone(), FacetResolution::Duplicate);
            }
        }
        Ok(Self { names })
    }

    #[cfg(test)]
    pub fn from_names(ids: BTreeMap<String, Option<String>>) -> Self {
        let names = ids
            .into_iter()
            .map(|(name, id)| {
                let resolution = id
                    .filter(|value| is_well_formed_facet_id(value))
                    .map(FacetResolution::Id)
                    .unwrap_or(FacetResolution::Missing);
                (name, resolution)
            })
            .collect();
        Self { names }
    }

    fn lookup(&self, name: &str) -> FacetResolution {
        self.names
            .get(name)
            .cloned()
            .unwrap_or(FacetResolution::Missing)
    }
}

/// Read named facet assignments written by think's segment writer.
/// Missing and malformed files mean no assignments.
pub fn read_segment_facet_assignments(journal: &Path, rel: &str) -> Vec<String> {
    let normalized = rel.replace('\\', "/");
    let parts = normalized.split('/').collect::<Vec<_>>();
    if parts.len() < 4 || segment_key(parts[2]).is_none() {
        return Vec::new();
    }
    let segment = parts[..3].join("/");
    let Ok(path) = resolve_journal_path(journal, &segment) else {
        return Vec::new();
    };
    let Ok(text) = fs::read_to_string(path.join("talents/facets.json")) else {
        return Vec::new();
    };
    let Ok(values) = serde_json::from_str::<Vec<Value>>(&text) else {
        return Vec::new();
    };
    values
        .iter()
        .filter_map(|value| value.get("facet").and_then(Value::as_str))
        .map(str::to_owned)
        .collect()
}

/// Classify one stored source path. An unclassifiable source is ineligible even
/// under whole-journal scope; it is never promoted to journal-wide access.
pub fn classify_source(
    journal: &Path,
    path: &str,
    stream: Option<&str>,
    declarations: &FacetDeclarationSet,
) -> ChunkClassification {
    if path.starts_with("entity_search:")
        || stream == Some("mcp.agent")
        || path.split('/').any(|component| component == "mcp.agent")
        || is_authored_chat_path(path)
    {
        return excluded(path);
    }
    let Some(spec) = resolve_spec(path) else {
        return excluded(path);
    };
    let IndexDisposition::Admitted { category, basis } = spec.disposition else {
        return excluded(path);
    };
    if basis == ScopeBasis::SegmentAssigned
        && path.split('/').any(|component| component == "talents")
        && !has_valid_segment_talent_shape(path)
    {
        return excluded(path);
    }
    match basis {
        ScopeBasis::JournalWide => admitted(path, category, basis, Vec::new()),
        ScopeBasis::FacetOwned => {
            let Some(name) = facet_owned_name(path) else {
                return excluded(path);
            };
            match declarations.lookup(name) {
                FacetResolution::Id(id) => admitted(path, category, basis, vec![id]),
                FacetResolution::Unreadable => unclassified(path),
                FacetResolution::Duplicate => excluded(path),
                FacetResolution::Missing => admitted(path, category, basis, Vec::new()),
            }
        }
        ScopeBasis::SegmentAssigned => {
            // Per-segment derived prose follows its capture, so it is
            // Transcripts even when the assignment list is empty.
            let mut ids = BTreeSet::new();
            for name in read_segment_facet_assignments(journal, path) {
                match declarations.lookup(&name) {
                    FacetResolution::Id(id) => {
                        ids.insert(id);
                    }
                    FacetResolution::Unreadable => return unclassified(path),
                    FacetResolution::Duplicate => return excluded(path),
                    FacetResolution::Missing => {}
                }
            }
            admitted(path, category, basis, ids.into_iter().collect())
        }
    }
}

fn admitted(
    path: &str,
    category: AdmittedCategory,
    basis: ScopeBasis,
    facet_ids: Vec<String>,
) -> ChunkClassification {
    ChunkClassification {
        path: path.to_owned(),
        category: Some(category_name(category)),
        basis: Some(basis_name(basis)),
        eligible: true,
        unclassified: false,
        facet_ids,
    }
}

fn excluded(path: &str) -> ChunkClassification {
    ChunkClassification {
        path: path.to_owned(),
        category: None,
        basis: None,
        eligible: false,
        unclassified: false,
        facet_ids: Vec::new(),
    }
}

fn unclassified(path: &str) -> ChunkClassification {
    ChunkClassification {
        path: path.to_owned(),
        category: None,
        basis: None,
        eligible: false,
        unclassified: true,
        facet_ids: Vec::new(),
    }
}

fn category_name(category: AdmittedCategory) -> &'static str {
    match category {
        AdmittedCategory::Transcripts => "transcripts",
        AdmittedCategory::Entities => "entities",
        AdmittedCategory::Facets => "facets",
    }
}

fn basis_name(basis: ScopeBasis) -> &'static str {
    match basis {
        ScopeBasis::FacetOwned => "facet_owned",
        ScopeBasis::SegmentAssigned => "segment_assigned",
        ScopeBasis::JournalWide => "journal_wide",
    }
}

fn facet_owned_name(path: &str) -> Option<&str> {
    let parts = path.split('/').collect::<Vec<_>>();
    if parts.first() == Some(&"facets") {
        return parts.get(1).copied();
    }
    parts
        .iter()
        .position(|part| *part == "talents")
        .and_then(|index| parts.get(index + 1).copied())
}

fn is_authored_chat_path(path: &str) -> bool {
    let parts = path.split('/').collect::<Vec<_>>();
    let parts = if parts.first() == Some(&"chronicle") {
        &parts[1..]
    } else {
        &parts[..]
    };
    parts.len() == 4
        && parts[0].len() == 8
        && parts[0].bytes().all(|byte| byte.is_ascii_digit())
        && parts[1] == "chat"
        && parts[3] == "chat.jsonl"
}

fn has_valid_segment_talent_shape(path: &str) -> bool {
    let parts = path.split('/').collect::<Vec<_>>();
    let Some(talents) = parts.iter().position(|component| *component == "talents") else {
        return true;
    };
    talents
        .checked_sub(1)
        .and_then(|segment| parts.get(segment))
        .and_then(|segment| segment_parse(segment))
        .is_some()
}

// Keep this check behaviorally identical to
// `solstone_core_journal_io::is_uuid_v4`. `indexer-store` is the live index
// writer and a direct dependency would change the Windows source-attestation
// lock binding. Facets uses the journal-io helper through its lower dependency.
fn is_well_formed_facet_id(id: &str) -> bool {
    if id.len() != 36 {
        return false;
    }
    let bytes = id.as_bytes();
    if bytes[8] != b'-' || bytes[13] != b'-' || bytes[18] != b'-' || bytes[23] != b'-' {
        return false;
    }
    if bytes[14] != b'4' || !matches!(bytes[19], b'8' | b'9' | b'a' | b'b') {
        return false;
    }
    bytes.iter().enumerate().all(|(index, byte)| {
        matches!(index, 8 | 13 | 18 | 23)
            || (byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::reserve_temp_path;

    const ID: &str = "a1b2c3d4-e5f6-4a7b-8c9d-0e1f2a3b4c5d";

    #[test]
    fn disposition_uses_the_pattern_and_exact_facet_name() {
        let declarations = FacetDeclarationSet::from_names(BTreeMap::from([(
            "Work".to_string(),
            Some(ID.to_string()),
        )]));
        let root = reserve_temp_path("classification-pattern");
        let facet = classify_source(&root, "facets/Work/news/20260107.md", None, &declarations);
        assert_eq!(facet.category, Some("facets"));
        assert_eq!(facet.basis, Some("facet_owned"));
        assert_eq!(facet.facet_ids, vec![ID]);

        let journal_wide = classify_source(&root, "20260107/talents/day.md", None, &declarations);
        assert_eq!(journal_wide.basis, Some("journal_wide"));

        let mcp = classify_source(
            &root,
            "20260107/mcp.agent/123456_300/talents/brief.md",
            None,
            &declarations,
        );
        assert!(!mcp.eligible);
    }

    #[test]
    fn exclusions_and_malformed_segment_shapes_are_ineligible() {
        let root = reserve_temp_path("classification-exclusions");
        let declarations = FacetDeclarationSet::default();
        // AC4: a broad pattern match is insufficient without the segment shape.
        assert!(
            !classify_source(
                &root,
                "20260107/default/not-a-segment/talents/brief.md",
                None,
                &declarations,
            )
            .eligible
        );
        assert!(!classify_source(&root, "unrecognized/path", None, &declarations).eligible);

        // AC5: every named excluded family remains unreachable even with a
        // whole-journal all-category query (classification is ineligible).
        for path in [
            "config/actions/20260107.jsonl",
            "facets/work/logs/20260107.jsonl",
            "apps/work/talents/brief.md",
            "20260107/default/123456_300/talents/sense.json",
            "20260107/default/123456_300/talents/documents.json",
            "20260107/default/123456_300/talents/screen.json",
            "20260107/talents/day.jsonl",
            "20260107/talents/morning_briefing.json",
            "20260107/default/123456_300/browser_tab.jsonl",
            "20260107/import.chatgpt/imported.jsonl",
            "20260107/import.chatgpt/thread/conversation_transcript.jsonl",
            "20260107/import.claude/thread/imported_audio.jsonl",
            "entity_search:alice",
        ] {
            assert!(
                !classify_source(&root, path, None, &declarations).eligible,
                "{path}"
            );
        }
        // AC6: mcp.agent is excluded by path component without stream.json.
        assert!(
            !classify_source(
                &root,
                "20260107/default/123456_300/mcp.agent/talents/brief.md",
                None,
                &declarations,
            )
            .eligible
        );
    }

    #[test]
    fn declarations_assign_stable_ids_without_casefolding_or_mute_filtering() {
        const OLD: &str = "a1b2c3d4-e5f6-4a7b-8c9d-0e1f2a3b4c5d";
        const NEW: &str = "b1b2c3d4-e5f6-4a7b-8c9d-0e1f2a3b4c5d";
        let root = reserve_temp_path("classification-declarations");
        std::fs::create_dir_all(root.join("facets/Work")).expect("work facet");
        std::fs::write(
            root.join("facets/Work/facet.json"),
            format!(r#"{{"id":"{OLD}","muted":true}}"#),
        )
        .expect("write work declaration");

        let before = FacetDeclarationSet::from_journal(&root).expect("declarations");
        let material = classify_source(&root, "facets/Work/news/20260107.md", None, &before);
        // AC14 / AC18: mute is irrelevant and exact-case directory naming is used.
        assert_eq!(material.facet_ids, vec![OLD]);
        assert!(
            classify_source(&root, "facets/work/news/20260107.md", None, &before)
                .facet_ids
                .is_empty()
        );

        // AC12: moving a declaration preserves its stable id for facet-owned
        // material without touching an already-recorded classification.
        std::fs::rename(root.join("facets/Work"), root.join("facets/Renamed"))
            .expect("rename facet directory");
        let after_rename = FacetDeclarationSet::from_journal(&root).expect("renamed declarations");
        assert_eq!(
            classify_source(
                &root,
                "facets/Renamed/news/20260107.md",
                None,
                &after_rename
            )
            .facet_ids,
            vec![OLD]
        );
        assert_eq!(material.facet_ids, vec![OLD]);

        // AC13: deleting and recreating a name gives it a different id; old
        // recorded material therefore cannot be reached by the new id.
        std::fs::create_dir_all(root.join("facets/Work")).expect("recreate work facet");
        std::fs::write(
            root.join("facets/Work/facet.json"),
            format!(r#"{{"id":"{NEW}"}}"#),
        )
        .expect("write recreated declaration");
        let recreated = FacetDeclarationSet::from_journal(&root).expect("recreated declarations");
        assert_eq!(
            classify_source(&root, "facets/Work/news/20260107.md", None, &recreated).facet_ids,
            vec![NEW]
        );
        assert_ne!(material.facet_ids, vec![NEW]);
        std::fs::remove_dir_all(root).expect("cleanup declarations");
    }

    #[test]
    fn segment_assignments_drop_missing_names_and_record_unreadable_declarations() {
        const ID: &str = "a1b2c3d4-e5f6-4a7b-8c9d-0e1f2a3b4c5d";
        let root = reserve_temp_path("classification-assignments");
        let segment = root.join("chronicle/20260107/default/123456_300/talents");
        std::fs::create_dir_all(&segment).expect("segment");
        let path = "20260107/default/123456_300/talents/brief.md";
        let empty_declarations = FacetDeclarationSet::default();
        // AC16: missing and malformed assignment files are empty assignment sets.
        assert!(
            classify_source(&root, path, None, &empty_declarations)
                .facet_ids
                .is_empty()
        );
        std::fs::write(segment.join("facets.json"), "{}").expect("malformed assignment");
        assert!(
            classify_source(&root, path, None, &empty_declarations)
                .facet_ids
                .is_empty()
        );

        std::fs::create_dir_all(root.join("facets/Work")).expect("work facet");
        std::fs::write(
            root.join("facets/Work/facet.json"),
            format!(r#"{{"id":"{ID}"}}"#),
        )
        .expect("work declaration");
        std::fs::create_dir_all(root.join("facets/NoId")).expect("no id facet");
        std::fs::write(root.join("facets/NoId/facet.json"), "{}").expect("no id declaration");
        std::fs::write(
            segment.join("facets.json"),
            r#"[{"facet":"Work"},{"facet":"Missing"},{"facet":"NoId"}]"#,
        )
        .expect("assignment names");
        let declarations = FacetDeclarationSet::from_journal(&root).expect("declarations");
        assert_eq!(
            classify_source(&root, path, None, &declarations).facet_ids,
            vec![ID]
        );

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            std::fs::create_dir_all(root.join("facets/Blocked")).expect("blocked facet");
            let blocked = root.join("facets/Blocked/facet.json");
            std::fs::write(&blocked, format!(r#"{{"id":"{ID}"}}"#)).expect("blocked declaration");
            std::fs::set_permissions(&blocked, std::fs::Permissions::from_mode(0o000))
                .expect("make declaration unreadable");
            std::fs::write(segment.join("facets.json"), r#"[{"facet":"Blocked"}]"#)
                .expect("blocked assignment");
            let declarations =
                FacetDeclarationSet::from_journal(&root).expect("blocked declarations");
            let classified = classify_source(&root, path, None, &declarations);
            // AC17: unreadable named declarations stay visibly unclassified.
            assert!(classified.unclassified);
            std::fs::set_permissions(&blocked, std::fs::Permissions::from_mode(0o644))
                .expect("restore declaration permissions");
        }
        std::fs::remove_dir_all(root).expect("cleanup assignments");
    }
}
