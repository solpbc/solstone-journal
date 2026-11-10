// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Physical body segments on a day's civil grid, including facet siblings.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use solstone_core_format::content::body_records::{
    BodyRow, RowKind, copy_rank, grid_length, same_grid,
};
use solstone_core_segment::owner_deleted;

use crate::apple_health::{
    DayRows, StoredRowsError, facet_file_text, same_row, stored_rows, stream_dir,
};
use crate::body_stream::BodySource;

struct Stored {
    rows: Vec<BodyRow>,
    filing: Option<String>,
}

/// A day's rows at their physical destinations, with any assignments to write.
#[derive(Default)]
pub struct FiledDay {
    pub windows: DayRows,
    pub filings: BTreeMap<String, String>,
    pub split_rows: BTreeMap<String, u64>,
    /// Changed existing identities kept under their earlier assignment.
    pub revisions: BTreeMap<String, BTreeMap<String, u64>>,
}

/// One read of each live window on a day, under the source's import lock.
pub struct DayPlacement<'a> {
    journal: &'a Path,
    source: &'a BodySource,
    day: String,
    held: BTreeMap<String, Stored>,
    ids: BTreeMap<String, Vec<String>>,
}

impl<'a> DayPlacement<'a> {
    pub fn load(journal: &'a Path, source: &'a BodySource, day: &str) -> Result<Self, String> {
        let mut held = BTreeMap::new();
        let mut ids: BTreeMap<String, Vec<String>> = BTreeMap::new();
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
                Ok(None) | Err(StoredRowsError::NotRows) => continue,
                Err(StoredRowsError::Read(error)) => return Err(error),
            };
            let file = entry.path().join("talents/facets.json");
            let filing = match fs::read_to_string(&file) {
                Ok(text) => Some(text),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                Err(error) => return Err(format!("{}: {error}", file.display())),
            };
            for row in rows.iter().filter(|row| row.kind != RowKind::Moved) {
                ids.entry(row.id.clone()).or_default().push(key.clone());
            }
            held.insert(key, Stored { rows, filing });
        }
        for keys in ids.values_mut() {
            keys.sort_by(|left, right| (copy_rank(left), left).cmp(&(copy_rank(right), right)));
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
        let key = keys
            .iter()
            .find(|key| same_grid(grid, key))
            .or_else(|| keys.first())?;
        let held = &self.held[key];
        let row = held
            .rows
            .iter()
            .find(|row| row.id == id && row.kind != RowKind::Moved)?;
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
        let journal = self.journal;
        let source = self.source;
        let mut result = FiledDay::default();
        let mut placed: BTreeMap<String, BTreeMap<String, BodyRow>> = BTreeMap::new();
        let chosen = facet.map(facet_file_text);
        for ((day, key), rows) in windows {
            debug_assert_eq!(day, self.day);
            let original = stream_dir(journal, source, &day).join(&key);
            // Preserve the original placement's existing refusals. A deleted
            // sibling is never inferred to have held a particular incoming id.
            let blocked = owner_deleted(&original).map_err(|error| error.to_string())?
                || matches!(
                    stored_rows(journal, source, &(day, key.clone())),
                    Err(StoredRowsError::NotRows)
                );
            for (id, mut row) in rows {
                if row.kind == RowKind::Moved || blocked {
                    let target = placed.entry(key.clone()).or_default();
                    if row.kind != RowKind::Moved || !target.contains_key(&id) {
                        target.insert(id, row);
                    }
                    continue;
                }
                let existing = self.existing(&key, &id);
                let same_place = existing.filter(|(old, _, _)| same_grid(&key, old));
                let filing = if facet.is_none() {
                    None
                } else if !may_refile {
                    existing
                        .and_then(|(_, held, _)| held.filing.as_deref())
                        .or(chosen.as_deref())
                } else {
                    chosen.as_deref()
                };
                let destination = if let Some((old, _, _)) = same_place {
                    old.to_owned()
                } else if facet.is_none() || (filing.is_none() && existing.is_none()) {
                    key.clone()
                } else {
                    self.destination(&key, filing, may_refile, &placed, &result.filings)?
                };
                if let Some((_, held, old)) = existing {
                    if facet.is_some() {
                        row.run.clone_from(&old.run);
                    }
                    if !may_refile
                        && held.filing.is_some()
                        && chosen.is_some()
                        && held.filing.as_deref() != chosen.as_deref()
                        && !same_row(&row, old)
                    {
                        *result
                            .revisions
                            .entry(destination.clone())
                            .or_default()
                            .entry(held.filing.clone().unwrap_or_default())
                            .or_default() += 1;
                    }
                } else if facet.is_some() && destination != key {
                    *result.split_rows.entry(destination.clone()).or_default() += 1;
                }
                let assign = may_refile
                    || existing.is_none()
                    || same_place.is_none()
                    || existing.is_some_and(|(_, held, _)| held.filing.is_none());
                if assign && let Some(filing) = filing {
                    result
                        .filings
                        .insert(destination.clone(), filing.to_owned());
                }
                // A live row wins over the mark staged at its former physical
                // key when only its facet sibling, rather than its time, differs.
                placed.entry(destination).or_default().insert(id, row);
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
        may_refile: bool,
        placed: &BTreeMap<String, BTreeMap<String, BodyRow>>,
        filings: &BTreeMap<String, String>,
    ) -> Result<String, String> {
        let journal = self.journal;
        let source = self.source;
        // A run authorized to move may reuse the original segment as before.
        let original_filing = filings.get(original).map(String::as_str).or_else(|| {
            self.held
                .get(original)
                .and_then(|held| held.filing.as_deref())
        });
        if may_refile
            || (original_filing.is_none()
                && !solstone_core_segment::holds_foreign_content(
                    &stream_dir(journal, source, &self.day).join(original),
                ))
        {
            return Ok(original.to_owned());
        }
        let matching = self
            .held
            .iter()
            .find(|(key, held)| {
                same_grid(key, original)
                    && held.filing.as_deref() == filing
                    && !solstone_core_segment::holds_foreign_content(
                        &stream_dir(journal, source, &self.day).join(key),
                    )
            })
            .map(|(key, _)| key.clone())
            .or_else(|| {
                filings
                    .iter()
                    .find(|(key, value)| same_grid(key, original) && Some(value.as_str()) == filing)
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
