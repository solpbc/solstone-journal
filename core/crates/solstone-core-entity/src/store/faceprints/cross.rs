// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Journal-wide face rules. Thresholds were calibrated on an owner-consented
//! evaluation of real meeting recordings. No embeddings enter the saved result.
use super::{Face, Result, Source, panel};
use panel::CrossEntity;
use serde::{Deserialize, Serialize};
use solstone_core_journal_io::{JournalRoot, contained_path};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

pub const FILE: &str = "faces_derived.json";
pub const MERGE_BASIS: &str = "meeting-face";
pub const MERGE_EVIDENCE: &str = "meeting photos suggest these may be the same person.";

#[derive(Clone, Debug)]
pub struct Summary {
    pub id: String,
    pub names: Vec<String>,
    pub centroid: Vec<f32>,
    pub anchored: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rules {
    pub entities: BTreeMap<String, CrossEntity>,
    pub contested_pairs: BTreeSet<[String; 2]>,
    pub merge_pairs: BTreeSet<[String; 2]>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Backfill {
    pub cursor: Option<String>,
    /// Recording-relative path -> raw metadata and screen-body binding. A changed
    /// input is tried again; unchanged unsupported codecs are not tried nightly.
    pub unsupported: BTreeMap<String, String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Derived {
    pub schema_version: u32,
    pub rules: Rules,
    pub backfill: Backfill,
}
impl Default for Derived {
    fn default() -> Self {
        Self {
            schema_version: 1,
            rules: Rules::default(),
            backfill: Backfill::default(),
        }
    }
}

fn normalize(text: &str) -> String {
    let mut depth = 0;
    let mut clean = String::new();
    for c in text.split('|').next().unwrap_or("").to_lowercase().chars() {
        match c {
            '(' => depth += 1,
            ')' => depth = (depth - 1).max(0),
            _ if depth == 0 && (c.is_ascii_alphanumeric() || c.is_whitespace()) => clean.push(c),
            _ => {}
        }
    }
    clean.split_whitespace().collect::<Vec<_>>().join(" ")
}
// SequenceMatcher's longest matching blocks, including its first-match tie rule.
fn matching(a: &[u8], b: &[u8]) -> usize {
    let (mut ai, mut bi, mut best) = (0, 0, 0);
    let mut prev = vec![0; b.len() + 1];
    for (i, x) in a.iter().enumerate() {
        let mut next = vec![0; b.len() + 1];
        for (j, y) in b.iter().enumerate() {
            if x == y {
                next[j + 1] = prev[j] + 1;
                if next[j + 1] > best {
                    best = next[j + 1];
                    ai = i + 1 - best;
                    bi = j + 1 - best;
                }
            }
        }
        prev = next;
    }
    if best == 0 {
        0
    } else {
        best + matching(&a[..ai], &b[..bi]) + matching(&a[ai + best..], &b[bi + best..])
    }
}

pub fn compatible(a: &str, b: &str) -> bool {
    let (a, b) = (normalize(a), normalize(b));
    if a.is_empty() || b.is_empty() {
        return false;
    }
    if a == b || a.starts_with(&(b.clone() + " ")) || b.starts_with(&(a.clone() + " ")) {
        return true;
    }
    if 2. * matching(a.as_bytes(), b.as_bytes()) as f64 / (a.len() + b.len()) as f64 >= 0.85 {
        return true;
    }
    let (ta, tb) = (
        a.split_whitespace().collect::<Vec<_>>(),
        b.split_whitespace().collect::<Vec<_>>(),
    );
    ta.len() > 1
        && tb.len() > 1
        && ta.last() == tb.last()
        && (ta[0].starts_with(tb[0]) || tb[0].starts_with(ta[0]))
}
pub fn names_compatible(a: &[String], b: &[String]) -> bool {
    a.iter().any(|a| b.iter().any(|b| compatible(a, b)))
}
pub fn exclusivity_drop(a: &Summary, others: &[Summary]) -> bool {
    others.iter().any(|b| {
        a.id != b.id
            && b.anchored >= 3 * a.anchored.max(1)
            && panel::cosine(&a.centroid, &b.centroid) >= 0.50
            && !names_compatible(&a.names, &b.names)
    })
}

/// The first pool supplies consensus centroids; the second supplies post-drop
/// look centroids. Keeping the stages separate avoids a hidden entity hiding a
/// third entity through a contested pair.
pub fn rules(consensus: &[Summary], looks: &[Summary]) -> Rules {
    let mut result = Rules::default();
    for a in consensus {
        result
            .entities
            .entry(a.id.clone())
            .or_default()
            .exclusivity_drop = exclusivity_drop(a, consensus);
    }
    for (i, a) in looks.iter().enumerate() {
        if result
            .entities
            .get(&a.id)
            .is_some_and(|r| r.exclusivity_drop)
        {
            continue;
        }
        for b in &looks[i + 1..] {
            if result
                .entities
                .get(&b.id)
                .is_some_and(|r| r.exclusivity_drop)
            {
                continue;
            }
            let s = panel::cosine(&a.centroid, &b.centroid);
            let mut pair = [a.id.clone(), b.id.clone()];
            pair.sort();
            if names_compatible(&a.names, &b.names) {
                if s >= 0.55 {
                    result.merge_pairs.insert(pair);
                }
            } else if s >= 0.80 {
                result.entities.entry(a.id.clone()).or_default().contested = true;
                result.entities.entry(b.id.clone()).or_default().contested = true;
                result.contested_pairs.insert(pair);
            }
        }
    }
    result
}

pub fn path(journal: &Path) -> Result<PathBuf> {
    contained_path(journal, &format!("entities/{FILE}")).map_err(|e| e.to_string())
}
/// No creation on a read, including when the feature has never been enabled.
pub fn load(journal: &Path) -> Result<Derived> {
    let text =
        solstone_core_journal_io::read_optional_text(path(journal)?).map_err(|e| e.to_string())?;
    let value: Derived = text
        .map(|s| serde_json::from_str(&s).map_err(|e| e.to_string()))
        .transpose()?
        .unwrap_or_default();
    if value.schema_version != 1 {
        return Err("unsupported face-derived version".into());
    }
    Ok(value)
}
pub fn for_entity(journal: &Path, id: &str) -> Result<CrossEntity> {
    Ok(load(journal)?
        .rules
        .entities
        .get(id)
        .copied()
        .unwrap_or_default())
}
fn publish(journal: &Path, value: &Derived) -> Result<()> {
    let p = path(journal)?;
    let dir = p.parent().expect("entities file");
    solstone_core_journal_io::create_directory_with_mode(dir, 0o700).map_err(|e| e.to_string())?;
    let binding = JournalRoot::open(dir).map_err(|e| e.to_string())?;
    let bytes = serde_json::to_vec(value).map_err(|e| e.to_string())?;
    if solstone_core_journal_io::read_optional_text(&p)
        .map_err(|e| e.to_string())?
        .as_deref()
        == std::str::from_utf8(&bytes).ok()
    {
        return Ok(());
    }
    super::storage::publish(dir, &binding, FILE, &bytes)
}
/// Preserve rules written by a concurrent maintenance pass and never repopulate
/// a file after the owner turned recognition off and cleared it.
pub fn update_backfill(journal: &Path, backfill: Backfill) -> Result<()> {
    let _trust = crate::hold_entity_trust_lock(journal).map_err(|e| e.to_string())?;
    if !super::enabled(journal)? {
        return Ok(());
    }
    let mut value = load(journal)?;
    value.backfill = backfill;
    publish(journal, &value)
}

fn names(entity: &crate::JournalEntity) -> Vec<String> {
    let mut result: Vec<_> = entity
        .value
        .get("name")
        .and_then(serde_json::Value::as_str)
        .into_iter()
        .map(str::to_owned)
        .collect();
    result.extend(
        entity
            .value
            .get("aka")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(serde_json::Value::as_str)
            .map(str::to_owned),
    );
    result
}
fn summary(id: &str, names: &[String], faces: &[&Face]) -> Option<Summary> {
    Some(Summary {
        id: id.into(),
        names: names.to_vec(),
        centroid: panel::centroid(faces)?,
        anchored: faces
            .iter()
            .filter(|f| f.metadata.source == Source::Anchored)
            .count(),
    })
}

/// Use a facet containing evidence for both people, newest shared day first,
/// then facet id. A journal-wide similarity never invents a facet membership.
fn shared_facet(a: &[&Face], b: &[&Face]) -> Option<(String, String)> {
    fn facets(faces: &[&Face]) -> BTreeMap<String, String> {
        let mut out: BTreeMap<String, String> = BTreeMap::new();
        for face in faces {
            for coordinate in &face.metadata.activities {
                if let Ok([facet, day, _]) = serde_json::from_str::<[String; 3]>(coordinate) {
                    out.entry(facet)
                        .and_modify(|d| *d = d.clone().max(day.clone()))
                        .or_insert(day);
                }
            }
        }
        out
    }
    let (a, b) = (facets(a), facets(b));
    a.into_iter()
        .filter_map(|(facet, day)| b.get(&facet).map(|other| (facet, day.min(other.clone()))))
        .max_by(|(af, ad), (bf, bd)| ad.cmp(bd).then_with(|| bf.cmp(af)))
}

/// Called after participation recovery; one identity census and one trust hold.
pub fn reconcile(journal: &Path) -> Result<Rules> {
    if !super::enabled(journal)? {
        return Ok(Rules::default());
    }
    let _trust = crate::hold_entity_trust_lock(journal).map_err(|e| e.to_string())?;
    if !super::enabled(journal)? {
        return Ok(Rules::default());
    }
    let roster = crate::live_journal_entities(journal).map_err(|e| e.to_string())?;
    let mut pools = BTreeMap::new();
    let mut encoder = None;
    for (folder, entity) in &roster {
        if crate::is_admissible_person(entity) {
            let (archive, state) = super::load_in_dir(journal, folder)?;
            if let Some(archive) = archive {
                if encoder
                    .as_ref()
                    .is_some_and(|e| e != &archive.envelope.encoder)
                {
                    return Err("face encoder mismatch across people".into());
                }
                encoder = Some(archive.envelope.encoder);
                pools.insert(entity.id.clone(), (entity, archive.faces, state));
            }
        }
    }
    let owner = pools
        .values()
        .find(|(e, _, _)| e.is_principal())
        .and_then(|(_, f, _)| panel::owner_centroid(f));
    let mut kept = BTreeMap::new();
    let mut consensus = vec![];
    let mut looks = vec![];
    for (id, (entity, faces, state)) in &pools {
        let rows = panel::kept_evidence(faces, state, entity.is_principal(), owner.as_deref());
        let ns = names(entity);
        if let Some(s) = summary(id, &ns, &rows) {
            consensus.push(s);
        }
        let distinct = panel::looks(&rows);
        if distinct.len() >= panel::MIN_LOOKS
            && let Some(s) = summary(id, &ns, &distinct)
        {
            looks.push(s);
        }
        kept.insert(id.clone(), rows);
    }
    let result = rules(&consensus, &looks);
    // Replaying unchanged evidence must not rewrite review timestamps or undo a
    // dismissal. Only a genuinely new face proposal is recorded.
    let existing = crate::load_merge_candidates(journal, None, None).map_err(|e| e.to_string())?;
    let proposals: Vec<_> = result
        .merge_pairs
        .iter()
        .filter_map(|[a, b]| {
            shared_facet(&kept[a], &kept[b]).map(|(facet, day)| (a, b, facet, day))
        })
        .collect();
    let withdrawn = existing.iter().any(|r| is_stale_proposal(r, &proposals));
    if withdrawn {
        super::super::review_candidates::mutate_candidates(journal, |rows| {
            rows.retain(|r| !is_stale_proposal(r, &proposals));
            Ok(())
        })
        .map_err(|e| e.to_string())?;
    }
    for (a, b, facet, day) in proposals {
        if existing.iter().any(|r| {
            r["facet"] == facet
                && ((r["source_slug"] == *a && r["target_slug"] == *b)
                    || (r["source_slug"] == *b && r["target_slug"] == *a))
        }) {
            continue;
        }
        let name = |id: &str| {
            pools[id]
                .0
                .value
                .get("name")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
        };
        crate::record_merge_candidate(
            journal,
            &facet,
            &day,
            name(a),
            a,
            name(b),
            b,
            MERGE_EVIDENCE,
            Some(MERGE_BASIS),
            None,
            None,
        )
        .map_err(|e| e.to_string())?;
    }
    let mut derived = load(journal)?;
    derived.rules = result.clone();
    publish(journal, &derived)?;
    Ok(result)
}

fn is_stale_proposal(
    row: &serde_json::Value,
    proposals: &[(&String, &String, String, String)],
) -> bool {
    row.pointer("/evidence/basis")
        .and_then(serde_json::Value::as_str)
        == Some(MERGE_BASIS)
        && row["status"] == "open"
        && !proposals.iter().any(|(a, b, facet, _)| {
            row["facet"] == *facet
                && ((row["source_slug"] == **a && row["target_slug"] == **b)
                    || (row["source_slug"] == **b && row["target_slug"] == **a))
        })
}

/// Clear face-derived suggestions along with the cross flags and backfill ledger.
pub(super) fn clear_candidates(journal: &Path) -> Result<()> {
    super::super::review_candidates::mutate_candidates(journal, |rows| {
        rows.retain(|r| {
            r.pointer("/evidence/basis")
                .and_then(serde_json::Value::as_str)
                != Some(MERGE_BASIS)
        });
        Ok(())
    })
    .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests;
