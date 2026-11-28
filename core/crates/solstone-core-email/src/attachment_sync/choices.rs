// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Owner choice, metadata-only estimates and explicit per-file requests.
use super::{live, open_db, parse_rows, segment_dir, storage};
use crate::attachment_parts::{
    AUTO_ENCODED_LIMIT, AttachmentKind, CandidateDisposition, HARD_LIMIT, candidates,
};
use crate::cache::HeaderCache;
use crate::choice::{AttachmentScope, Choice};
use crate::error::{EmailError, Result};
use crate::select::Decision;
use crate::store;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use solstone_core_format::content::segment_items::valid_id;
use std::collections::HashSet;
use std::fs;
use std::path::Path;

#[derive(Default, Serialize)]
pub struct CandidateCounts {
    pub files: usize,
    pub encoded_bytes: u64,
    pub unsupported: usize,
    pub too_large: usize,
    pub uncertain: usize,
    pub images: usize,
    pub pdfs: usize,
    pub already_synced: usize,
    pub remaining_files: usize,
    pub remaining_images: usize,
}

#[derive(Default, Serialize)]
pub struct FilePreview {
    pub sent: CandidateCounts,
    pub answered: CandidateCounts,
    pub automatic_limit: usize,
    pub hard_limit: usize,
    pub pdf_reader_available: bool,
}

/// A repeated reference to the same answered message counts once on its side.
pub fn preview(
    root: &Path,
    choice: &Choice,
    decisions: &[Decision],
    pdf_reader_available: bool,
) -> Result<FilePreview> {
    preview_counts(root, choice, decisions, pdf_reader_available, |_, _| {})
}

fn preview_counts(
    root: &Path,
    choice: &Choice,
    decisions: &[Decision],
    pdf_reader_available: bool,
    mut remaining_image: impl FnMut(bool, String),
) -> Result<FilePreview> {
    let mut out = FilePreview {
        pdf_reader_available,
        automatic_limit: AUTO_ENCODED_LIMIT,
        hard_limit: HARD_LIMIT,
        ..Default::default()
    };
    let prior: HashSet<_> = store::read_landed(root)?
        .into_iter()
        .map(|r| r.key)
        .collect();
    let mut old = HashSet::new();
    let mut sent = HashSet::new();
    let mut answered = HashSet::new();
    for d in decisions.iter().filter(|d| choice.includes_decision(d)) {
        if prior.contains(&d.key) {
            old.insert(d.key.as_str());
            if let Some(k) = d.parent.parent_key.as_deref() {
                old.insert(k);
            }
        }
        sent.insert(d.key.as_str());
        if let Some(k) = d.parent.parent_key.as_deref() {
            answered.insert(k);
        }
    }
    if sent.is_empty() {
        return Ok(out);
    }
    let cache = HeaderCache::open(&store::header_cache_path(root))?;
    let account = store::read_account(root)?
        .map(|a| a.account)
        .unwrap_or_default();
    let db = read_db(root)?;
    let mut exists = db
        .as_ref()
        .map(|db| db.prepare("SELECT EXISTS(SELECT 1 FROM items WHERE id=?1)"))
        .transpose()?;
    for (keys, counts, sent_by_owner) in [
        (sent, &mut out.sent, true),
        (answered, &mut out.answered, false),
    ] {
        for key in keys {
            let Some(h) = cache.get(key)? else {
                continue;
            };
            for c in candidates(&h, None) {
                counts.files += 1;
                counts.already_synced += usize::from(old.contains(key));
                counts.encoded_bytes = counts.encoded_bytes.saturating_add(c.part.size);
                match c.disposition {
                    CandidateDisposition::Unsupported | CandidateDisposition::InlineDecorative => {
                        counts.unsupported += 1
                    }
                    CandidateDisposition::TooLarge => counts.too_large += 1,
                    CandidateDisposition::InlineUncertain => counts.uncertain += 1,
                    CandidateDisposition::Eligible => {}
                }
                if matches!(
                    c.disposition,
                    CandidateDisposition::Eligible | CandidateDisposition::InlineUncertain
                ) {
                    let image = c.kind == Some(AttachmentKind::Image);
                    let pdf = c.kind == Some(AttachmentKind::Pdf);
                    counts.images += usize::from(image);
                    counts.pdfs += usize::from(pdf);
                    let id = super::occurrence_id(&account, &h, &c);
                    let processed: bool = match &mut exists {
                        Some(statement) => statement.query_row([&id], |r| r.get(0))?,
                        None => false,
                    };
                    if !processed && c.part.size <= AUTO_ENCODED_LIMIT as u64 {
                        if image {
                            counts.remaining_images += 1;
                            remaining_image(sent_by_owner, id);
                        }
                        counts.remaining_files += usize::from(
                            c.disposition == CandidateDisposition::Eligible
                                && (!pdf || pdf_reader_available),
                        );
                    }
                }
            }
        }
    }
    Ok(out)
}

#[derive(Default, Debug, Clone, Serialize, Deserialize)]
pub struct FileProgress {
    pub estimated_thoughts: i64,
    pub actual_thoughts: i64,
    pub exchanges_done: i64,
    pub exchanges_total: i64,
}

pub(crate) fn record_estimate(
    root: &Path,
    account: &str,
    choice: &Choice,
    decisions: &[Decision],
) -> Result<()> {
    let mut forecast = HashSet::new();
    preview_counts(root, choice, decisions, true, |sent, id| {
        if choice.attachments == AttachmentScope::Both
            || (sent && choice.attachments == AttachmentScope::Sent)
        {
            forecast.insert(id);
        }
    })?;
    let mut db = open_db(root)?;
    let tx = db.transaction()?;
    tx.execute("DELETE FROM forecast_images WHERE account=?1", [account])?;
    tx.execute(
        "INSERT OR REPLACE INTO reports(account,estimated) VALUES (?1,?2)",
        params![account, forecast.len() as i64],
    )?;
    for id in forecast {
        tx.execute(
            "INSERT INTO forecast_images VALUES (?1,?2)",
            params![account, id],
        )?;
    }
    tx.commit()?;
    Ok(())
}

fn read_db(root: &Path) -> Result<Option<Connection>> {
    let path = store::config_dir(root).join("attachment-state.sqlite");
    if !path.is_file() {
        return Ok(None);
    }
    Ok(Some(Connection::open_with_flags(
        path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )?))
}

pub(crate) fn progress(root: &Path, account: &str) -> Result<Option<FileProgress>> {
    let Some(db) = read_db(root)? else {
        return Ok(None);
    };
    Ok(db
        .query_row(
            "SELECT estimated,actual,done,total FROM reports WHERE account=?1",
            [account],
            |r| {
                Ok(FileProgress {
                    estimated_thoughts: r.get(0)?,
                    actual_thoughts: r.get(1)?,
                    exchanges_done: r.get(2)?,
                    exchanges_total: r.get(3)?,
                })
            },
        )
        .optional()?)
}

pub fn queue_item(root: &Path, day: &str, seg: &str, id: &str) -> Result<()> {
    if !valid_id(id) {
        return Err(EmailError::InvalidInput("invalid file".into()));
    }
    let _guard = store::hold_sync_lock(root)?;
    let account = store::read_credentials(root)?
        .ok_or(EmailError::NotConnected)?
        .account;
    let choice = store::read_choice(root)?
        .ok_or_else(|| EmailError::InvalidInput("choose email first".into()))?;
    let dir = segment_dir(root, day, seg)
        .filter(|dir| live(dir))
        .ok_or_else(|| EmailError::InvalidInput("exchange unavailable".into()))?;
    let rows = parse_rows(
        &fs::read_to_string(dir.join("conversation_transcript.jsonl")).map_err(storage)?,
    )?;
    if rows
        .first()
        .and_then(|r| r.pointer("/email/account"))
        .and_then(Value::as_str)
        != Some(&account)
    {
        return Err(EmailError::NotConnected);
    }
    let (owner_key, answered) = rows
        .iter()
        .skip(1)
        .find_map(|row| {
            let key = row.pointer("/email/key")?.as_str()?;
            for (is_parent, attachments) in [
                (false, row.get("attachments")),
                (true, row.pointer("/reply_to/attachments")),
            ] {
                if attachments.and_then(Value::as_array).is_some_and(|files| {
                    files.iter().any(|a| {
                        a["attachment_id"] == id
                            && matches!(
                                a["state"].as_str(),
                                Some(
                                    "ambiguous"
                                        | "names_only"
                                        | "too_large"
                                        | "failed"
                                        | "unavailable"
                                )
                            )
                    })
                }) {
                    return Some((key, is_parent));
                }
            }
            None
        })
        .ok_or_else(|| EmailError::InvalidInput("file cannot be selected".into()))?;
    let decisions = store::read_decisions(root)?;
    let selected = decisions
        .iter()
        .find(|d| {
            d.key == owner_key
                && d.reference.account.eq_ignore_ascii_case(&account)
                && choice.includes_decision(d)
        })
        .ok_or_else(|| {
            EmailError::InvalidInput("exchange is outside the current email choice".into())
        })?;
    let source_key = if answered {
        selected
            .parent
            .parent_key
            .as_deref()
            .ok_or_else(|| EmailError::InvalidInput("answered message is unavailable".into()))?
    } else {
        owner_key
    };
    let mut db = open_db(root)?;
    let tx = db.transaction()?;
    tx.execute(
        "INSERT OR REPLACE INTO requests VALUES (?1,?2)",
        params![id, source_key],
    )?;
    for d in decisions
        .iter()
        .filter(|d| d.key == source_key || d.parent.parent_key.as_deref() == Some(source_key))
    {
        tx.execute(
            "DELETE FROM checked WHERE account=?1 AND key=?2",
            params![account, d.key],
        )?;
    }
    tx.commit()?;
    Ok(())
}
