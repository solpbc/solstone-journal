// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Physical body segments on a day's civil grid, including facet siblings.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};
use solstone_core_format::content::body_records::{
    BodyRow, RowKind, copy_rank, grid_length, parse_source_time, same_grid,
};
use solstone_core_segment::owner_deleted;

use crate::apple_health::{
    DayRows, StoredRowsError, facet_file_text, same_row, stored_rows, stream_dir,
};
use crate::body_stream::BodySource;

#[cfg(all(test, feature = "full-tests"))]
#[path = "body_placement_tests.rs"]
mod tests;

struct Stored {
    rows: Vec<BodyRow>,
    filing: Option<String>,
}

#[derive(Clone)]
struct ParentFiling {
    filing: Option<String>,
    prior: bool,
}

/// A misplaced unfiled replica: retire it only after its replacement is live.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct TileRepair {
    pub source_key: String,
    pub destination_key: String,
    pub filing: String,
    pub mark: BodyRow,
}

/// A day's rows at their physical destinations, with any assignments to write.
#[derive(Default)]
pub struct FiledDay {
    pub windows: DayRows,
    pub filings: BTreeMap<String, String>,
    pub split_rows: BTreeMap<String, BTreeSet<String>>,
    /// Changed existing identities kept under their earlier assignment.
    pub revisions: BTreeMap<String, BTreeMap<String, BTreeSet<String>>>,
    pub reserved_unfiled: BTreeSet<String>,
    pub tile_repairs: Vec<TileRepair>,
    pub kept_unfiled: BTreeSet<String>,
}

/// One read of each live window on a day, under the source's import lock.
pub struct DayPlacement<'a> {
    journal: &'a Path,
    source: &'a BodySource,
    day: String,
    held: BTreeMap<String, Stored>,
    ids: BTreeMap<String, Vec<(String, usize)>>,
}

impl<'a> DayPlacement<'a> {
    pub fn load(journal: &'a Path, source: &'a BodySource, day: &str) -> Result<Self, String> {
        let mut held = BTreeMap::new();
        let mut ids: BTreeMap<String, Vec<(String, usize)>> = BTreeMap::new();
        let dir = stream_dir(journal, source, day);
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => Some(entries),
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
                ) =>
            {
                None
            }
            Err(error) => return Err(format!("{}: {error}", dir.display())),
        };
        for entry in entries.into_iter().flatten() {
            let entry = entry.map_err(|error| error.to_string())?;
            let key = entry.file_name().to_string_lossy().into_owned();
            if grid_length(&key).is_none()
                || owner_deleted(&entry.path()).map_err(|error| error.to_string())?
            {
                continue;
            }
            let rows = match stored_rows(journal, source, &(day.to_owned(), key.clone())) {
                Ok(Some((_, rows))) => rows,
                Ok(None) => Vec::new(),
                Err(StoredRowsError::NotRows) => continue,
                Err(StoredRowsError::Read(error)) => return Err(error),
            };
            let file = entry.path().join("talents/facets.json");
            let filing = match fs::read_to_string(&file) {
                Ok(text) => Some(text),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                Err(error) => return Err(format!("{}: {error}", file.display())),
            };
            for (index, row) in rows
                .iter()
                .enumerate()
                .filter(|(_, row)| row.kind != RowKind::Moved)
            {
                ids.entry(row.id.clone())
                    .or_default()
                    .push((key.clone(), index));
            }
            held.insert(key, Stored { rows, filing });
        }
        for keys in ids.values_mut() {
            keys.sort_by(|left, right| {
                (copy_rank(&left.0), &left.0).cmp(&(copy_rank(&right.0), &right.0))
            });
        }
        Ok(Self {
            journal,
            source,
            day: day.to_owned(),
            held,
            ids,
        })
    }

    fn existing(&self, grid: &str, id: &str) -> Option<(&str, &Stored, &BodyRow)> {
        let keys = self.ids.get(id)?;
        let (key, index) = keys
            .iter()
            .find(|(key, _)| same_grid(grid, key))
            .or_else(|| keys.first())?;
        let held = &self.held[key];
        let row = held.rows.get(*index)?;
        Some((key, held, row))
    }

    /// File new identities separately when the original segment is filed
    /// elsewhere. Existing identities keep their filing without a move answer.
    pub fn file(
        &self,
        windows: DayRows,
        facet: Option<&str>,
        may_refile: bool,
    ) -> Result<FiledDay, String> {
        self.file_for_import(windows, facet, may_refile, None)
    }

    pub fn file_for_import(
        &self,
        windows: DayRows,
        facet: Option<&str>,
        may_refile: bool,
        import_id: Option<&str>,
    ) -> Result<FiledDay, String> {
        let journal = self.journal;
        let source = self.source;
        let mut result = FiledDay::default();
        let mut placed: BTreeMap<String, BTreeMap<String, BodyRow>> = BTreeMap::new();
        let chosen = facet.map(facet_file_text);
        let prior = |row: &BodyRow| import_id.is_none() || row.run.as_deref() != import_id;
        let mut parents = BTreeMap::new();
        for id in self.ids.keys() {
            if let Some((_, held, row)) = self.existing("", id)
                && row.kind == RowKind::Experience
            {
                parents.insert(
                    id.clone(),
                    ParentFiling {
                        filing: held.filing.clone(),
                        prior: prior(row),
                    },
                );
            }
        }
        let incoming_parents: BTreeSet<String> = windows
            .iter()
            .flat_map(|(_, rows)| rows.values())
            .filter(|row| row.kind == RowKind::Experience)
            .map(|row| row.id.clone())
            .collect();
        let mut parent_days = BTreeMap::new();
        for row in windows
            .iter()
            .flat_map(|(_, rows)| rows.values())
            .chain(self.held.values().flat_map(|held| held.rows.iter()))
            .filter(|row| row.kind == RowKind::ExperienceTile)
        {
            let Some(parent) = &row.of else {
                continue;
            };
            if parents.contains_key(parent) || incoming_parents.contains(parent) {
                continue;
            }
            let zone = solstone_core_journal_config::placed_zone(journal, source.source)
                .ok_or(source.zone_unrecognised)?;
            let Some(start) = row.start.as_deref().and_then(parse_source_time) else {
                continue;
            };
            let day = start.with_timezone(&zone).format("%Y%m%d").to_string();
            if day == self.day {
                continue;
            }
            if !parent_days.contains_key(&day) {
                parent_days.insert(day.clone(), Self::load(journal, source, &day)?);
            }
            if let Some((_, held, first)) = parent_days[&day].existing("", parent)
                && first.kind == RowKind::Experience
            {
                parents.insert(
                    parent.clone(),
                    ParentFiling {
                        filing: held.filing.clone(),
                        prior: prior(first),
                    },
                );
            }
        }
        let mut orphaned_tiles = BTreeSet::new();
        if !may_refile {
            for (key, held) in &self.held {
                if held.filing.is_none() {
                    orphaned_tiles.extend(
                        held.rows
                            .iter()
                            .filter(|row| {
                                row.kind == RowKind::ExperienceTile
                                    && row.of.as_ref().and_then(|id| parents.get(id)).is_none()
                            })
                            .map(|row| row.id.clone()),
                    );
                }
                if held.filing.is_none()
                    && held.rows.iter().any(|row| {
                        row.kind == RowKind::ExperienceTile
                            && row
                                .of
                                .as_ref()
                                .and_then(|id| parents.get(id))
                                .is_none_or(|parent| parent.filing.is_some())
                    })
                {
                    result.reserved_unfiled.insert(key.clone());
                }
            }
        }
        // All parent choices precede synthetic tile choices, even in a repeated
        // civil hour. Both passes only plan; the caller writes physical files.
        let mut firsts = Vec::new();
        let mut rest = Vec::new();
        for (window, rows) in windows {
            let (first, other): (BTreeMap<_, _>, BTreeMap<_, _>) = rows
                .into_iter()
                .partition(|(_, row)| row.kind == RowKind::Experience);
            if !first.is_empty() {
                firsts.push((window.clone(), first));
            }
            if !other.is_empty() {
                rest.push((window, other));
            }
        }
        for ((day, key), rows) in firsts.into_iter().chain(rest) {
            debug_assert_eq!(day, self.day);
            let original = stream_dir(journal, source, &day).join(&key);
            let blocked = owner_deleted(&original).map_err(|error| error.to_string())?
                || matches!(
                    stored_rows(journal, source, &(day, key.clone())),
                    Err(StoredRowsError::NotRows)
                );
            let has_earlier_foreign_rows = !may_refile
                && chosen.is_some()
                && (self.held.iter().any(|(held_key, held)| {
                    same_grid(&key, held_key)
                        && held.filing.is_some()
                        && held.filing != chosen
                        && held.rows.iter().any(|row| row.kind != RowKind::Moved)
                }) || result.filings.iter().any(|(held_key, filing)| {
                    same_grid(&key, held_key) && Some(filing) != chosen.as_ref()
                }) || rows.values().any(|row| {
                    row.kind != RowKind::Moved
                        && (self.existing(&key, &row.id).is_some_and(|(_, held, _)| {
                            held.filing.is_some() && held.filing != chosen
                        }) || (row.kind == RowKind::ExperienceTile
                            && row.of.as_ref().and_then(|id| parents.get(id)).is_some_and(
                                |parent| parent.filing.is_some() && parent.filing != chosen,
                            )))
                }));
            for (id, mut row) in rows {
                let existing = self.existing(&key, &id);
                let same_place = existing.filter(|(old, _, _)| same_grid(&key, old));
                let needs_parent = row.kind == RowKind::ExperienceTile
                    && existing.is_none_or(|(_, held, _)| held.filing.is_none());
                let parent = needs_parent
                    .then(|| row.of.as_ref().and_then(|id| parents.get(id)))
                    .flatten()
                    .filter(|_| !orphaned_tiles.contains(&id))
                    .cloned();
                if row.kind == RowKind::ExperienceTile && existing.is_none() && parent.is_none() {
                    // A refused/unreadable parent cannot become readable through
                    // a new synthetic tile under this run's different facet.
                    continue;
                }
                if row.kind == RowKind::Moved || (blocked && same_place.is_none()) {
                    let target = placed.entry(key.clone()).or_default();
                    if row.kind != RowKind::Moved || !target.contains_key(&id) {
                        target.insert(id, row);
                    }
                    continue;
                }
                let inherited = parent
                    .as_ref()
                    .and_then(|parent| parent.filing.as_deref())
                    .filter(|_| !may_refile);
                let repair = inherited.is_some()
                    && existing.is_some_and(|(old, held, _)| {
                        held.filing.is_none() && result.reserved_unfiled.contains(old)
                    });
                let same_place = same_place.filter(|_| !repair);
                let filing = if inherited.is_some() {
                    inherited
                } else if (needs_parent && parent.is_none())
                    || existing.is_some_and(|(old, held, _)| {
                        held.filing.is_none() && result.reserved_unfiled.contains(old)
                    })
                    || facet.is_none()
                {
                    None
                } else if !may_refile {
                    existing
                        .and_then(|(_, held, _)| held.filing.as_deref())
                        .or(chosen.as_deref())
                } else {
                    chosen.as_deref()
                };
                let destination =
                    if let Some((old, _, _)) = existing.filter(|_| orphaned_tiles.contains(&id)) {
                        old.to_owned()
                    } else if let Some((old, _, _)) = same_place {
                        old.to_owned()
                    } else if (facet.is_none() && inherited.is_none())
                        || (filing.is_none() && existing.is_none())
                    {
                        key.clone()
                    } else {
                        self.destination(
                            &key,
                            filing,
                            if facet.is_none() && parent.is_none() {
                                filing
                            } else {
                                chosen.as_deref()
                            },
                            may_refile,
                            &placed,
                            &result,
                        )?
                    };
                if repair && let Some((old_key, _, old)) = existing {
                    let mut mark = BodyRow::new(
                        RowKind::Moved,
                        id.clone(),
                        &old.source,
                        &old.record_type,
                        old.label.clone(),
                    );
                    mark.of.clone_from(&old.of);
                    mark.run.clone_from(&old.run);
                    placed
                        .entry(old_key.to_owned())
                        .or_default()
                        .insert(id.clone(), mark.clone());
                    result.tile_repairs.push(TileRepair {
                        source_key: old_key.to_owned(),
                        destination_key: destination.clone(),
                        filing: filing.expect("a repair inherits a filing").to_owned(),
                        mark,
                    });
                }
                if let Some((_, held, old)) = existing {
                    if facet.is_some() || repair {
                        row.run.clone_from(&old.run);
                    }
                    if row.kind != RowKind::ExperienceTile
                        && !may_refile
                        && held.filing.is_some()
                        && chosen.is_some()
                        && held.filing.as_deref() != chosen.as_deref()
                        && !same_row(&row, old)
                    {
                        result
                            .revisions
                            .entry(destination.clone())
                            .or_default()
                            .entry(held.filing.clone().unwrap_or_default())
                            .or_default()
                            .insert(id.clone());
                    }
                } else if has_earlier_foreign_rows
                    && !parent.as_ref().is_some_and(|parent| parent.prior)
                {
                    result
                        .split_rows
                        .entry(destination.clone())
                        .or_default()
                        .insert(if row.kind == RowKind::ExperienceTile {
                            row.of.clone().unwrap_or_else(|| id.clone())
                        } else {
                            id.clone()
                        });
                }
                let assign = may_refile
                    || existing.is_none()
                    || same_place.is_none()
                    || existing.is_some_and(|(_, held, _)| held.filing.is_none());
                if assign && let Some(filing) = filing {
                    if result
                        .filings
                        .get(&destination)
                        .is_some_and(|planned| planned != filing)
                    {
                        return Err("a part of your journal is now filed elsewhere. the entries waiting to be saved were left unwritten.".to_owned());
                    }
                    result
                        .filings
                        .insert(destination.clone(), filing.to_owned());
                }
                if row.kind == RowKind::Experience {
                    let effective = result
                        .filings
                        .get(&destination)
                        .cloned()
                        .or_else(|| {
                            self.held
                                .get(&destination)
                                .and_then(|held| held.filing.clone())
                        })
                        .or_else(|| existing.and_then(|(_, held, _)| held.filing.clone()));
                    parents.insert(
                        id.clone(),
                        ParentFiling {
                            filing: effective,
                            prior: existing.is_some_and(|(_, _, old)| prior(old)),
                        },
                    );
                }
                placed.entry(destination).or_default().insert(id, row);
            }
        }
        for key in &result.reserved_unfiled {
            if self.held[key].rows.iter().any(|row| {
                row.kind != RowKind::Moved
                    && !result
                        .tile_repairs
                        .iter()
                        .any(|repair| repair.source_key == *key && repair.mark.id == row.id)
            }) {
                result.kept_unfiled.insert(key.clone());
            }
        }
        result.windows = placed
            .into_iter()
            .map(|(key, rows)| ((self.day.clone(), key), rows))
            .collect();
        Ok(result)
    }

    fn destination(
        &self,
        original: &str,
        filing: Option<&str>,
        chosen: Option<&str>,
        may_refile: bool,
        placed: &BTreeMap<String, BTreeMap<String, BodyRow>>,
        plan: &FiledDay,
    ) -> Result<String, String> {
        let filings = &plan.filings;
        let reserved_unfiled = &plan.reserved_unfiled;
        let sibling_only = chosen.is_none() && filing.is_some();
        let journal = self.journal;
        let source = self.source;
        // A run authorized to move may reuse the original segment as before.
        let original_filing = filings.get(original).map(String::as_str).or_else(|| {
            self.held
                .get(original)
                .and_then(|held| held.filing.as_deref())
        });
        if !reserved_unfiled.contains(original)
            && filings
                .get(original)
                .is_none_or(|planned| Some(planned.as_str()) == filing)
            && (may_refile
                || (original_filing.is_none()
                    && filing == chosen
                    && !solstone_core_segment::holds_foreign_content(
                        &stream_dir(journal, source, &self.day).join(original),
                    )))
        {
            return Ok(original.to_owned());
        }
        let matching = self
            .held
            .iter()
            .find(|(key, held)| {
                same_grid(key, original)
                    && !(sibling_only && key.as_str() == original)
                    && !reserved_unfiled.contains(*key)
                    && filings
                        .get(*key)
                        .map(String::as_str)
                        .or(held.filing.as_deref())
                        == filing
                    && !solstone_core_segment::holds_foreign_content(
                        &stream_dir(journal, source, &self.day).join(key),
                    )
            })
            .map(|(key, _)| key.clone())
            .or_else(|| {
                filings
                    .iter()
                    .find(|(key, value)| {
                        same_grid(key, original)
                            && !(sibling_only && key.as_str() == original)
                            && Some(value.as_str()) == filing
                    })
                    .map(|(key, _)| key.clone())
            })
            .or_else(|| {
                filing
                    .is_none()
                    .then(|| {
                        placed
                            .keys()
                            .find(|key| {
                                same_grid(key, original)
                                    && !filings.contains_key(*key)
                                    && self.held.get(*key).is_none_or(|held| held.filing.is_none())
                            })
                            .cloned()
                    })
                    .flatten()
            });
        if let Some(key) = matching {
            return Ok(key);
        }
        let (clock, _) = original
            .split_once('_')
            .ok_or("a body window has no grid")?;
        let base = grid_length(original).ok_or("a body window has no grid")?;
        let end = match base {
            300 => 3_600,
            3_600 => 86_400,
            _ => u64::from(u32::MAX),
        };
        let stream = stream_dir(journal, source, &self.day);
        for length in (base + 2)..end {
            let key = format!("{clock}_{length}");
            if placed.contains_key(&key) || filings.contains_key(&key) {
                continue;
            }
            match stream.join(&key).symlink_metadata() {
                Ok(_) => continue,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(key),
                Err(error) => return Err(format!("{}: {error}", stream.join(&key).display())),
            }
        }
        Err("no free body segment name remains for this time".to_owned())
    }
}
