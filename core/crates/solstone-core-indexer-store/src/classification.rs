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

#[derive(Clone, Debug, PartialEq, Eq)]
enum FacetResolution {
    Id(String),
    Missing,
    Unreadable,
    Duplicate,
}

/// Exact directory-name to facet-id view used by the indexer.
///
/// Material under a live `facets/<name>/` directory follows that directory's
/// stable id. Stored references to a facet by name (segment assignments and
/// per-segment `talents/<name>/` output) also consult `facets/retired.json`:
/// a merged-away name resolves to the facet it was merged into, and a deleted
/// or unknown name resolves to no facet.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FacetDeclarationSet {
    names: BTreeMap<String, FacetResolution>,
    live_ids: BTreeMap<String, String>,
    retired: BTreeMap<String, RetiredName>,
}

/// One `facets/retired.json` entry, reduced to what resolution needs.
///
/// Keep this reader behaviorally identical to `solstone_core_facets`'
/// `read_retired_facets`; both are checked against one shared fixture corpus.
#[derive(Clone, Debug, PartialEq, Eq)]
struct RetiredName {
    id: Option<String>,
    successor: Option<String>,
}

const MAX_RETIRED_HOPS: usize = 16;

/// Parse `facets/retired.json`. A missing, unreadable or malformed file yields
/// no entries: every name that is not live then resolves to no facet, which
/// fails closed for chosen-facet access without hiding anything from
/// whole-journal access.
fn read_retired_names(journal: &Path) -> BTreeMap<String, RetiredName> {
    let Ok(text) = fs::read_to_string(journal.join("facets").join("retired.json")) else {
        return BTreeMap::new();
    };
    let Ok(Value::Object(root)) = serde_json::from_str::<Value>(&text) else {
        return BTreeMap::new();
    };
    let Some(Value::Object(names)) = root.get("names") else {
        return BTreeMap::new();
    };
    let well_formed = |value: Option<&Value>| {
        value
            .and_then(Value::as_str)
            .filter(|id| is_well_formed_facet_id(id))
            .map(str::to_owned)
    };
    names
        .iter()
        .filter_map(|(name, entry)| {
            let entry = entry.as_object()?;
            let state = entry.get("state").and_then(Value::as_str);
            // An unknown state reserves the name like a deletion: no successor.
            let successor = match state {
                Some("merged" | "renamed") => well_formed(entry.get("successor")),
                _ => None,
            };
            Some((
                name.clone(),
                RetiredName {
                    id: well_formed(entry.get("id")),
                    successor,
                },
            ))
        })
        .collect()
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
            // Facet names start with a letter; dot-directories are merge scratch
            // space and backups, never facets.
            if name.starts_with('.') {
                continue;
            }
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
        let live_ids = id_names
            .into_iter()
            .filter(|(_, names)| names.len() == 1)
            .map(|(id, mut names)| (id, names.remove(0)))
            .collect();
        Ok(Self {
            names,
            live_ids,
            retired: read_retired_names(journal),
        })
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
            .collect::<BTreeMap<String, FacetResolution>>();
        let mut id_names = BTreeMap::<String, Vec<String>>::new();
        for (name, resolution) in &names {
            if let FacetResolution::Id(id) = resolution {
                id_names.entry(id.clone()).or_default().push(name.clone());
            }
        }
        let live_ids = id_names
            .into_iter()
            .filter(|(_, names)| names.len() == 1)
            .map(|(id, mut names)| (id, names.remove(0)))
            .collect();
        Self {
            names,
            live_ids,
            retired: BTreeMap::new(),
        }
    }

    /// Whether `id` belongs to exactly one live facet directory.
    pub fn is_live_id(&self, id: &str) -> bool {
        self.live_ids.contains_key(id)
    }

    /// Whether two views resolve every name identically.
    pub fn same_as(&self, other: &Self) -> bool {
        self == other
    }

    /// Resolve material that lives under a `facets/<name>/` directory. Only a
    /// live directory owns it; a retired entry never redirects it.
    fn lookup(&self, name: &str) -> FacetResolution {
        self.names
            .get(name)
            .cloned()
            .unwrap_or(FacetResolution::Missing)
    }

    /// Resolve a stored reference to a facet by name.
    fn lookup_reference(&self, name: &str) -> FacetResolution {
        let live = self.names.get(name).cloned();
        let Some(retired) = self.retired.get(name) else {
            return live.unwrap_or(FacetResolution::Missing);
        };
        match live {
            // The same facet: a leftover of an operation that did not complete,
            // including one written before the facet was given an id.
            Some(FacetResolution::Id(id))
                if retired.id.is_none() || retired.id.as_deref() == Some(id.as_str()) =>
            {
                FacetResolution::Id(id)
            }
            // A live directory whose declaration cannot be read stays visibly
            // unclassified; a duplicate id stays excluded.
            Some(resolution @ (FacetResolution::Unreadable | FacetResolution::Duplicate)) => {
                resolution
            }
            // The name is live with a different identity and also retired: the
            // references are ambiguous, so they belong to no facet.
            Some(_) => FacetResolution::Missing,
            None => self.follow_successor(retired),
        }
    }

    fn follow_successor(&self, retired: &RetiredName) -> FacetResolution {
        let mut successor = retired.successor.clone();
        for _ in 0..MAX_RETIRED_HOPS {
            let Some(id) = successor else {
                return FacetResolution::Missing;
            };
            if let Some(name) = self.live_ids.get(&id) {
                // A live facet that is itself retired under its own name keeps
                // its identity only when the entry is its own leftover.
                return match self.retired.get(name) {
                    Some(entry)
                        if entry.id.is_some() && entry.id.as_deref() != Some(id.as_str()) =>
                    {
                        FacetResolution::Missing
                    }
                    _ => FacetResolution::Id(id),
                };
            }
            successor = self
                .retired
                .values()
                .find(|entry| entry.id.as_deref() == Some(id.as_str()))
                .and_then(|entry| entry.successor.clone());
        }
        FacetResolution::Missing
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
            let resolution = if path.starts_with("facets/") {
                declarations.lookup(name)
            } else {
                declarations.lookup_reference(name)
            };
            match resolution {
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
            let mut unresolved = false;
            for name in read_segment_facet_assignments(journal, path) {
                match declarations.lookup_reference(&name) {
                    FacetResolution::Id(id) => {
                        ids.insert(id);
                    }
                    FacetResolution::Unreadable => return unclassified(path),
                    FacetResolution::Duplicate => return excluded(path),
                    FacetResolution::Missing => unresolved = true,
                }
            }
            // Chosen-facet access requires every assigned facet. A name that no
            // longer resolves (renamed, deleted, merged away or id-less) cannot be
            // granted, so dropping it would shrink the requirement and widen reach;
            // the material instead belongs to no chosen facet.
            if unresolved {
                ids.clear();
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
    fn segment_assignments_with_unresolved_names_belong_to_no_chosen_facet() {
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
        // An assigned name that does not resolve to a stable id empties the set, so
        // a grant on `Work` alone never reaches material also assigned elsewhere.
        let classified = classify_source(&root, path, None, &declarations);
        assert!(classified.eligible && !classified.unclassified);
        assert!(classified.facet_ids.is_empty());
        std::fs::write(segment.join("facets.json"), r#"[{"facet":"Work"}]"#)
            .expect("resolvable assignment");
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

    const S: &str = "22222222-2222-4222-8222-222222222222";
    const P: &str = "44444444-4444-4444-8444-444444444444";
    const W: &str = "55555555-5555-4555-8555-555555555555";
    const Z: &str = "66666666-6666-4666-8666-666666666666";
    const X: &str = "77777777-7777-4777-8777-777777777777";

    fn declare(root: &std::path::Path, dir: &str, id: &str) {
        std::fs::create_dir_all(root.join("facets").join(dir)).expect("facet dir");
        std::fs::write(
            root.join("facets").join(dir).join("facet.json"),
            format!(r#"{{"id":"{id}"}}"#),
        )
        .expect("declaration");
    }

    fn assign(root: &std::path::Path, names: &[&str]) -> &'static str {
        let talents = root.join("chronicle/20260107/default/123456_300/talents");
        std::fs::create_dir_all(&talents).expect("segment");
        let rows = names
            .iter()
            .map(|name| format!(r#"{{"facet":"{name}"}}"#))
            .collect::<Vec<_>>()
            .join(",");
        std::fs::write(talents.join("facets.json"), format!("[{rows}]")).expect("assignment");
        "20260107/default/123456_300/talents/brief.md"
    }

    fn ids(root: &std::path::Path, path: &str) -> Vec<String> {
        let declarations = FacetDeclarationSet::from_journal(root).expect("declarations");
        let classified = classify_source(root, path, None, &declarations);
        assert!(
            classified.eligible && !classified.unclassified,
            "{path} stays visible"
        );
        classified.facet_ids
    }

    #[test]
    fn retired_facet_names_resolve_references_to_their_successor_or_to_no_facet() {
        let root = reserve_temp_path("classification-retired");
        declare(&root, "solstone", S);
        declare(&root, "personal", P);
        std::fs::write(
            root.join("facets/retired.json"),
            format!(
                r#"{{"names":{{
                    "sunstone":{{"state":"merged","id":null,"successor":"{S}"}},
                    "gone":{{"state":"deleted","id":null}},
                    "a":{{"state":"merged","id":null,"successor":"{X}"}},
                    "b":{{"state":"merged","id":"{X}","successor":"{S}"}}
                }}}}"#
            ),
        )
        .expect("retired");

        let path = assign(&root, &["sunstone"]);
        assert_eq!(ids(&root, path), vec![S]);
        let path = assign(&root, &["personal", "sunstone"]);
        assert_eq!(
            ids(&root, path),
            vec![S.to_owned(), P.to_owned()]
                .into_iter()
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect::<Vec<_>>()
        );
        let path = assign(&root, &["gone"]);
        assert!(ids(&root, path).is_empty());
        // A deleted facet among several denies the whole segment to chosen facets.
        let path = assign(&root, &["personal", "gone"]);
        assert!(ids(&root, path).is_empty());
        // Chains follow retired ids to a live facet.
        let path = assign(&root, &["a"]);
        assert_eq!(ids(&root, path), vec![S]);
        // Per-segment output filed under a merged name is a reference too.
        assert_eq!(
            ids(
                &root,
                "20260107/default/123456_300/talents/sunstone/brief.md"
            ),
            vec![S]
        );
        // Material in a live folder keeps its own facet.
        assert_eq!(ids(&root, "facets/solstone/news/20260107.md"), vec![S]);
        // Merge scratch copies never make a live id a duplicate.
        declare(&root, ".facet-merge-1.dest", S);
        let path = assign(&root, &["sunstone"]);
        assert_eq!(ids(&root, path), vec![S]);
        std::fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn a_live_name_that_is_also_retired_keeps_its_folder_but_not_its_references() {
        let root = reserve_temp_path("classification-ambiguous");
        declare(&root, "work", W);
        std::fs::write(
            root.join("facets/retired.json"),
            format!(r#"{{"names":{{"work":{{"state":"renamed","id":"{Z}","successor":"{W}"}}}}}}"#),
        )
        .expect("retired");
        let path = assign(&root, &["work"]);
        assert!(ids(&root, path).is_empty());
        assert!(ids(&root, "20260107/default/123456_300/talents/work/brief.md").is_empty());
        assert_eq!(ids(&root, "facets/work/news/20260107.md"), vec![W]);

        // The facet's own leftover entry is the same facet, with or without
        // the id it has since been given.
        for leftover in [format!(r#""id":"{W}""#), r#""id":null"#.to_owned()] {
            std::fs::write(
                root.join("facets/retired.json"),
                format!(r#"{{"names":{{"work":{{"state":"deleted",{leftover}}}}}}}"#),
            )
            .expect("leftover");
            let path = assign(&root, &["work"]);
            assert_eq!(ids(&root, path), vec![W]);
        }
        std::fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn a_damaged_retired_record_fails_closed_only_for_names_that_are_not_live() {
        let root = reserve_temp_path("classification-retired-damaged");
        declare(&root, "personal", P);
        std::fs::write(root.join("facets/retired.json"), "{not json").expect("damaged");
        let path = assign(&root, &["sunstone"]);
        assert!(ids(&root, path).is_empty());
        let path = assign(&root, &["personal"]);
        assert_eq!(ids(&root, path), vec![P]);
        std::fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn retired_record_parse_matches_the_shared_corpus() {
        let corpus: Value = serde_json::from_str(
            &std::fs::read_to_string(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../solstone-core-facets/tests/fixtures/retired-facets-corpus.json"),
            )
            .expect("corpus"),
        )
        .expect("corpus json");
        for case in corpus["cases"].as_array().expect("cases") {
            let root = reserve_temp_path("classification-retired-corpus");
            std::fs::create_dir_all(root.join("facets")).expect("facets");
            std::fs::write(
                root.join("facets/retired.json"),
                case["input"].as_str().expect("input"),
            )
            .expect("input");
            let parsed = read_retired_names(&root);
            let expected = match case["expected"].as_object() {
                None => BTreeMap::new(),
                Some(entries) => entries
                    .iter()
                    .map(|(name, entry)| {
                        (
                            name.clone(),
                            RetiredName {
                                id: entry["id"].as_str().map(str::to_owned),
                                successor: entry["successor"].as_str().map(str::to_owned),
                            },
                        )
                    })
                    .collect(),
            };
            assert_eq!(parsed, expected, "{}", case["input"]);
            std::fs::remove_dir_all(root).expect("cleanup");
        }
    }
}
