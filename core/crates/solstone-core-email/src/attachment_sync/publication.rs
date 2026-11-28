// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Prepare one occurrence and install its typed files before linking the exchange.
use super::{Worker, live, occurrence_id, occurrence_metadata, segment_dir, storage};
use crate::attachment_parts::{
    AUTO_ENCODED_LIMIT, AttachmentCandidate, CandidateDisposition, HARD_LIMIT,
};
use crate::attachment_prepare::{PrepareFailure, PreparedAttachment};
use crate::error::Result;
use crate::model::Header;
use crate::store::LandedRecord;
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use solstone_core_format::content::segment_items::published_item;
use solstone_core_journal_io::{AtomicWriteOptions, LockOptions, atomic_replace, hold_lock};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Deserialize, Serialize)]
struct Published {
    kind: String,
    partial: bool,
    thoughts: usize,
}

impl Worker<'_> {
    pub(super) fn item(
        &mut self,
        rec: &LandedRecord,
        header: &Header,
        candidate: &AttachmentCandidate<'_>,
        allowed: bool,
        previous: Option<&Value>,
        added: &mut Vec<PathBuf>,
    ) -> Result<Value> {
        let id = occurrence_id(self.account, header, candidate);
        let p = candidate.part;
        let mut row = occurrence_metadata(self.account, header, candidate, &id);
        let prior: Option<(String, String, String, bool)> = self
            .db
            .query_row(
                "SELECT day,segment,info,installing FROM items WHERE id=?1",
                [&id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()?;
        if let Some((day, seg, info, installing)) = prior {
            if installing
                && let Some(dir) = segment_dir(self.root, &day, &seg).filter(|dir| live(dir))
            {
                let _guard = hold_lock(&dir, LockOptions::default()).map_err(storage)?;
                let items = dir.join("items");
                let stage = items.join(format!(".{id}.tmp"));
                if live(&dir)
                    && fs::symlink_metadata(&items).is_ok_and(|m| m.is_dir())
                    && fs::symlink_metadata(&stage).is_ok_and(|m| m.is_dir())
                    && !items.join(&id).exists()
                {
                    fs::rename(stage, items.join(&id)).map_err(storage)?;
                }
                self.db
                    .execute("UPDATE items SET installing=0 WHERE id=?1", [&id])?;
            }
            if let Some(path) = segment_dir(self.root, &day, &seg)
                .filter(|dir| live(dir))
                .and_then(|dir| published_item(&dir, &id))
            {
                if day == rec.day && seg == rec.segment && !added.contains(&path) {
                    added.push(path);
                }
                let info = serde_json::from_str(&info).map_err(storage)?;
                ready(&mut row, &id, &day, &seg, &info);
            } else {
                row["state"] = "removed".into();
            }
            return Ok(row);
        }
        if let Some(previous) = self.failed_this_pass.get(&id) {
            return Ok(previous.clone());
        }
        // A journal restore can retain the turn but lack the latest bookkeeping.
        // Its published reference still identifies the occurrence and its deletion.
        if let Some(old) = previous
            && (old["state"] == "removed" || old["item_ref"] == id)
        {
            let day = old["item_day"].as_str().unwrap_or(&rec.day);
            let segment = old["item_segment"].as_str().unwrap_or(&rec.segment);
            if let Some(path) = segment_dir(self.root, day, segment)
                .filter(|dir| live(dir))
                .and_then(|dir| published_item(&dir, &id))
            {
                let info = Published {
                    kind: old["kind"].as_str().unwrap_or("").to_owned(),
                    partial: old["partial"].as_bool().unwrap_or(false),
                    thoughts: 0,
                };
                self.db.execute(
                    "INSERT OR IGNORE INTO items VALUES (?1,?2,?3,?4,0)",
                    params![
                        id,
                        day,
                        segment,
                        serde_json::to_string(&info).map_err(storage)?
                    ],
                )?;
                if day == rec.day && segment == rec.segment {
                    added.push(path);
                }
                ready(&mut row, &id, day, segment, &info);
            } else {
                row["state"] = "removed".into();
            }
            return Ok(row);
        }
        let manual = self.requested.contains(&id);
        if !manual
            && let Some(old) = previous
            && old["attempted"] == true
            && old["state"] != "ready"
        {
            return Ok(old.clone());
        }
        let auto = allowed && candidate.disposition == CandidateDisposition::Eligible;
        let blocked = match candidate.disposition {
            CandidateDisposition::Unsupported => Some("unsupported"),
            CandidateDisposition::InlineDecorative => Some("decorative"),
            _ if p.size > HARD_LIMIT as u64 => Some("over_limit"),
            CandidateDisposition::TooLarge if !manual => Some("too_large"),
            CandidateDisposition::InlineUncertain if !manual => Some("ambiguous"),
            _ if !auto && !manual => Some("names_only"),
            _ if self.pdf_unavailable
                && candidate.kind == Some(crate::attachment_parts::AttachmentKind::Pdf) =>
            {
                Some("reader_unavailable")
            }
            _ => None,
        };
        if let Some(state) = blocked {
            row["state"] = state.into();
            return Ok(row);
        }
        if !manual && self.history_waits() {
            row["state"] = "pending".into();
            return Ok(row);
        }
        let ceiling = if manual {
            HARD_LIMIT
        } else {
            AUTO_ENCODED_LIMIT
        };
        row["attempted"] = true.into();
        let bytes = match self.source.fetch_attachment(header, p, ceiling, ceiling) {
            Ok(Some(bytes)) => bytes,
            Ok(None) => {
                row["state"] = "unavailable".into();
                return Ok(row);
            }
            Err(_) => {
                row["state"] = "failed".into();
                return Ok(row);
            }
        };
        // Keep extraction memoization within this pass so its reader, model and settings match.
        // Only the decoded result is reused; occurrence, attribution and grants stay independent.
        let mut key = Sha256::new();
        for b in [
            candidate.name.as_bytes(),
            p.mime.as_bytes(),
            bytes.as_slice(),
        ] {
            key.update((b.len() as u64).to_be_bytes());
            key.update(b);
        }
        let key: [u8; 32] = key.finalize().into();
        let prepared = if let Some(item) = self.prepared.get(&key) {
            let mut copy = item.clone();
            copy.thoughts = 0;
            Ok(copy)
        } else {
            let result = (self.prepare)(&candidate.name, &p.mime, &bytes);
            if let Ok(item) = &result {
                let size = item.text.len() + item.preview_jpeg.as_ref().map_or(0, Vec::len);
                if self.cached_bytes + size <= 8 * 1024 * 1024
                    && !item.partial
                    && self.prepared.len() < 1024
                {
                    self.prepared.insert(key, item.clone());
                    self.cached_bytes += size;
                }
            }
            result
        };
        match prepared {
            Ok(item) => {
                let Some(dir) = segment_dir(self.root, &rec.day, &rec.segment) else {
                    return Err(storage("invalid email segment"));
                };
                let _lock = hold_lock(&dir, LockOptions::default()).map_err(storage)?;
                if !live(&dir) {
                    row["state"] = "removed".into();
                    return Ok(row);
                }
                let info = Published {
                    kind: item.kind.to_owned(),
                    partial: item.partial,
                    thoughts: item.thoughts,
                };
                let record = || -> Result<()> {
                    let tx = self.db.transaction()?;
                    tx.execute(
                        "INSERT INTO items VALUES (?1,?2,?3,?4,1)",
                        params![
                            id,
                            rec.day,
                            rec.segment,
                            serde_json::to_string(&info).map_err(storage)?
                        ],
                    )?;
                    tx.execute(
                        "UPDATE reports SET actual=actual+?1 WHERE account=?2 AND EXISTS (SELECT 1 FROM forecast_images WHERE account=?2 AND id=?3)",
                        params![item.thoughts as i64, self.account, id],
                    )?;
                    tx.commit()?;
                    Ok(())
                };
                let file = write_item(
                    &dir,
                    &id,
                    &candidate.name,
                    &item,
                    self.choice.keep_originals.then_some(bytes.as_slice()),
                    record,
                )?;
                self.db
                    .execute("UPDATE items SET installing=0 WHERE id=?1", [&id])?;
                added.push(file);
                ready(&mut row, &id, &rec.day, &rec.segment, &info);
            }
            Err(err) => {
                row["state"] = match err {
                    PrepareFailure::Encrypted => "encrypted",
                    PrepareFailure::Unsupported => "unsupported",
                    PrepareFailure::Unreadable => "failed",
                    PrepareFailure::OverBudget => "over_limit",
                    PrepareFailure::DescriptionUnavailable => {
                        self.db.execute(
                            "UPDATE reports SET actual=actual+1 WHERE account=?1 AND EXISTS (SELECT 1 FROM forecast_images WHERE account=?1 AND id=?2)",
                            params![self.account,id],
                        )?;
                        "failed"
                    }
                    PrepareFailure::Unavailable => "failed",
                }
                .into()
            }
        }
        Ok(row)
    }
}
fn ready(row: &mut Value, id: &str, day: &str, segment: &str, info: &Published) {
    row["state"] = "ready".into();
    row["item_ref"] = id.into();
    row["item_day"] = day.into();
    row["item_segment"] = segment.into();
    row["kind"] = info.kind.clone().into();
    row["partial"] = info.partial.into();
    row["thoughts"] = info.thoughts.into();
}

fn write_item(
    dir: &Path,
    id: &str,
    name: &str,
    item: &PreparedAttachment,
    original: Option<&[u8]>,
    record: impl FnOnce() -> Result<()>,
) -> Result<PathBuf> {
    let file = match item.kind {
        "documents" => "document_transcript.md",
        "notes" => "note_transcript.md",
        "photos" => "image_transcript.md",
        "screenshots" => "screenshot_transcript.md",
        _ => return Err(storage("unknown attachment kind")),
    };
    let items = dir.join("items");
    if !items.exists() {
        fs::create_dir(&items).map_err(storage)?;
    }
    if !fs::symlink_metadata(&items).map_err(storage)?.is_dir() {
        return Err(storage("invalid items directory"));
    }
    let target = items.join(id);
    if let Some(path) = published_item(dir, id) {
        record()?;
        return Ok(path);
    }
    let stage = items.join(format!(".{id}.tmp"));
    if stage.exists() {
        fs::remove_dir_all(&stage).map_err(storage)?;
    }
    fs::create_dir(&stage).map_err(storage)?;
    let result = (|| {
        let title: String = name
            .chars()
            .map(|c| if c.is_control() { ' ' } else { c })
            .collect();
        atomic_replace(
            stage.join(file),
            format!("# {title}\n\n{}\n", item.text).as_bytes(),
            AtomicWriteOptions::default(),
        )
        .map_err(storage)?;
        if let Some(jpeg) = &item.preview_jpeg {
            atomic_replace(
                stage.join("preview.jpg"),
                jpeg,
                AtomicWriteOptions::default(),
            )
            .map_err(storage)?;
        }
        if let Some(bytes) = original {
            atomic_replace(
                stage.join("original.bin"),
                bytes,
                AtomicWriteOptions::default(),
            )
            .map_err(storage)?;
        }
        record()?;
        Ok(())
    })();
    if let Err(err) = result {
        let _ = fs::remove_dir_all(stage);
        return Err(err);
    }
    fs::rename(&stage, &target).map_err(storage)?;
    Ok(target.join(file))
}
