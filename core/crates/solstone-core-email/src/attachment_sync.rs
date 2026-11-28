// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Publish independently readable files inside their selected email exchange.
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use rusqlite::{Connection, params};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use solstone_core_format::content::segment_items::live_parent as live;
use solstone_core_journal_io::{AtomicWriteOptions, LockOptions, atomic_replace, hold_lock};

use crate::attachment_parts::{
    AUTO_ENCODED_LIMIT, AttachmentCandidate, CandidateDisposition, candidates,
};
use crate::attachment_prepare::{AttachmentPreparer, PreparedAttachment};
use crate::cache::HeaderCache;
use crate::choice::{AttachmentScope, Choice};
use crate::error::{EmailError, Result};
use crate::imap::ImapSource;
use crate::model::{Header, choose_body};
use crate::select::Decision;
use crate::store::{self, LandedRecord};
use crate::sync::{BatchPublishHook, CreatedEmailSegment};

mod choices;
mod publication;
pub use choices::{CandidateCounts, FilePreview, FileProgress, preview, queue_item};
pub(crate) use choices::{progress, record_estimate};

fn storage(e: impl std::fmt::Display) -> EmailError {
    EmailError::Storage(e.to_string())
}

fn open_db(root: &Path) -> Result<Connection> {
    let db = Connection::open(store::config_dir(root).join("attachment-state.sqlite"))?;
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS items (
        id TEXT PRIMARY KEY, day TEXT NOT NULL, segment TEXT NOT NULL, info TEXT NOT NULL, installing INTEGER NOT NULL);
        CREATE TABLE IF NOT EXISTS checked (
        account TEXT NOT NULL, key TEXT NOT NULL, policy TEXT NOT NULL,
        PRIMARY KEY(account,key));
        CREATE TABLE IF NOT EXISTS requests (id TEXT PRIMARY KEY, source_key TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS forecast_images (account TEXT NOT NULL, id TEXT NOT NULL, PRIMARY KEY(account,id));
        CREATE TABLE IF NOT EXISTS reports (
        account TEXT PRIMARY KEY, estimated INTEGER NOT NULL, actual INTEGER NOT NULL DEFAULT 0,
        done INTEGER NOT NULL DEFAULT 0, total INTEGER NOT NULL DEFAULT 0);",
    )?;
    Ok(db)
}

fn segment_dir(root: &Path, day: &str, segment: &str) -> Option<PathBuf> {
    if !solstone_core_format::segment::is_date_key(day)
        || solstone_core_format::segment::segment_key(segment).as_deref() != Some(segment)
    {
        return None;
    }
    solstone_core_journal_io::resolve_segment_exact(root, day, store::EMAIL_STREAM, segment)
        .ok()
        .flatten()
}

fn occurrence_id(account: &str, h: &Header, c: &AttachmentCandidate<'_>) -> String {
    let mut hash = Sha256::new();
    for input in [account, h.key.as_str()] {
        hash.update((input.len() as u64).to_be_bytes());
        hash.update(input.as_bytes());
    }
    for n in &c.part.path {
        hash.update(n.to_be_bytes());
    }
    format!("{:x}", hash.finalize())
}

/// Preserve just the local CID role while the selected body is in memory.
pub(crate) fn remember_inline(side: &mut Value, account: &str, h: &Header, html: Option<&str>) {
    let Some(html) = html else {
        return;
    };
    let Some(rows) = side.get_mut("attachments").and_then(Value::as_array_mut) else {
        return;
    };
    for (row, c) in rows.iter_mut().zip(candidates(h, Some(html))) {
        if c.inline {
            row["attachment_id"] = occurrence_id(account, h, &c).into();
            row["inline_context"] = context(c.disposition).into();
        }
    }
}

fn context(role: CandidateDisposition) -> &'static str {
    match role {
        CandidateDisposition::InlineDecorative => "decoration",
        CandidateDisposition::InlineUncertain => "uncertain",
        CandidateDisposition::Unsupported => "unsupported",
        CandidateDisposition::Eligible | CandidateDisposition::TooLarge => "authored",
    }
}

fn occurrence_metadata(
    account: &str,
    header: &Header,
    candidate: &AttachmentCandidate<'_>,
    id: &str,
) -> Value {
    let part = candidate.part;
    let mut row = json!({
        "name":candidate.name, "attachment_id":id, "encoded_bytes":part.size,
        "sender":header.from.first().map(crate::model::Addr::label),
        "mime":part.mime, "part_path":part.path, "mailbox_reference":crate::model::Reference::of(account,header)
    });
    if candidate.inline {
        row["inline_context"] = context(candidate.disposition).into();
        row["inline"] = true.into();
    }
    row
}

struct Worker<'a> {
    root: &'a Path,
    account: &'a str,
    requested: HashSet<String>,
    failed_this_pass: HashMap<String, Value>,
    source: &'a mut dyn ImapSource,
    prepared: HashMap<[u8; 32], PreparedAttachment>,
    cached_bytes: usize,
    pdf_unavailable: bool,
    history: bool,
    started: Instant,
    gate_checked: Option<Instant>,
    yield_history: bool,
    choice: &'a Choice,
    db: Connection,
    headers: HeaderCache,
    prepare: &'a AttachmentPreparer<'a>,
}

impl Worker<'_> {
    fn history_waits(&mut self) -> bool {
        if !self.history {
            return false;
        }
        if self.started.elapsed() >= Duration::from_secs(60) {
            return true;
        }
        if self
            .gate_checked
            .is_none_or(|seen| seen.elapsed() >= Duration::from_secs(1))
        {
            self.gate_checked = Some(Instant::now());
            self.yield_history = solstone_core_system::work_policy::WorkPolicy::read(self.root)
                .map_or(true, |p| p.backfill_paused)
                || solstone_core_system::work_inventory::newer_work_pending(self.root)
                    .unwrap_or(true);
        }
        self.yield_history
    }

    fn side(
        &mut self,
        side: &mut Value,
        key: &str,
        allowed: bool,
        rec: &LandedRecord,
        added: &mut Vec<PathBuf>,
    ) -> Result<()> {
        let Some(h) = self.headers.get(key)? else {
            return Ok(());
        };
        let list = candidates(&h, None);
        if list.is_empty() {
            return Ok(());
        }
        let known: HashMap<_, _> = side
            .get("attachments")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|r| Some((r.get("attachment_id")?.as_str()?, r)))
            .collect();
        let auto_allowed = allowed && !self.history_waits();
        let needs_context = list.iter().any(|c| {
            let id = occurrence_id(self.account, &h, c);
            c.inline
                && (auto_allowed || self.requested.contains(&id))
                && known
                    .get(id.as_str())
                    .and_then(|v| v.get("inline_context"))
                    .is_none()
        });
        let html = if needs_context {
            choose_body(&h.tree).html.and_then(|part| {
                self.source
                    .fetch_texts(&[(&h, &part)])
                    .ok()?
                    .pop()
                    .flatten()
            })
        } else {
            None
        };
        let mut rows = Vec::new();
        for mut c in candidates(&h, html.as_deref()) {
            let id = occurrence_id(self.account, &h, &c);
            let previous = known.get(id.as_str()).copied();
            let prior = previous.and_then(|v| v["inline_context"].as_str());
            if c.inline && c.disposition == CandidateDisposition::InlineUncertain {
                c.disposition = match prior {
                    Some("decoration") => CandidateDisposition::InlineDecorative,
                    Some("authored") if c.part.size > AUTO_ENCODED_LIMIT as u64 => {
                        CandidateDisposition::TooLarge
                    }
                    Some("authored") => CandidateDisposition::Eligible,
                    _ => c.disposition,
                };
            }
            let mut row = match self.item(rec, &h, &c, allowed, previous, added) {
                Ok(row) => row,
                Err(EmailError::Storage(err)) => {
                    log::warn!("email attachment publication failed: {err}");
                    let mut row = occurrence_metadata(self.account, &h, &c, &id);
                    row["state"] = "failed".into();
                    row["attempted"] = true.into();
                    row
                }
                Err(err) => return Err(err),
            };
            if c.inline
                && html.is_none()
                && prior.is_none()
                && let Some(fields) = row.as_object_mut()
            {
                fields.remove("inline_context");
            }
            if row["attempted"] == true
                && matches!(
                    row["state"].as_str(),
                    Some("unavailable" | "failed" | "encrypted" | "over_limit" | "unsupported")
                )
            {
                self.failed_this_pass.insert(id, row.clone());
            }
            rows.push(row);
        }
        side["attachments"] = Value::Array(rows);
        Ok(())
    }
}

fn parse_rows(source: &str) -> Result<Vec<Value>> {
    source
        .lines()
        .map(|l| serde_json::from_str(l).map_err(storage))
        .collect()
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn enrich(
    root: &Path,
    source: &mut dyn ImapSource,
    account: &str,
    choice: &Choice,
    decisions: &[Decision],
    landed: &[LandedRecord],
    prepare: &AttachmentPreparer<'_>,
    pdf_unavailable: bool,
    on_batch: Option<BatchPublishHook<'_>>,
) -> Result<()> {
    if landed.is_empty() {
        return Ok(());
    }
    let db = open_db(root)?;
    let policy = format!(
        "v2-{:?}-{}-{pdf_unavailable}",
        choice.attachments, choice.keep_originals
    );
    let checked: HashSet<String> = db
        .prepare("SELECT key FROM checked WHERE account=?1 AND policy=?2")?
        .query_map(params![account, policy], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    let selected: HashMap<_, _> = decisions
        .iter()
        .filter(|d| {
            choice.includes_decision(d) && d.reference.account.eq_ignore_ascii_case(account)
        })
        .map(|d| (d.key.as_str(), d))
        .collect();
    let today = jiff::Timestamp::now()
        .to_zoned_datetime(store::journal_timezone(root))
        .strftime("%Y%m%d")
        .to_string();
    let mut groups = BTreeMap::new();
    let mut eligible = 0;
    let mut done = 0;
    let mut history_total = 0;
    let mut history_done = 0;
    for rec in landed {
        if let Some(&d) = selected.get(rec.key.as_str()) {
            eligible += 1;
            let historical = rec.day < today;
            history_total += usize::from(historical);
            if checked.contains(&rec.key) {
                done += 1;
                history_done += usize::from(historical);
                continue;
            }
            groups
                .entry((&rec.day, &rec.segment))
                .or_insert_with(HashMap::new)
                .insert(rec.key.as_str(), (rec, d));
        }
    }
    db.execute(
        "UPDATE reports SET done=?1,total=?2 WHERE account=?3",
        params![done, eligible, account],
    )?;
    let save_progress = |history_done: usize| -> Result<()> {
        let mut state = store::read_state(root)?;
        if (state.file_history_done, state.file_history_total) != (history_done, history_total) {
            state.file_history_done = history_done;
            state.file_history_total = history_total;
            store::write_state(root, &state)?;
        }
        Ok(())
    };
    save_progress(history_done)?;
    if groups.is_empty() {
        return Ok(());
    }
    let picks = db
        .prepare("SELECT id,source_key FROM requests")?
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let (requested, requested_sources): (HashSet<_>, HashSet<_>) = picks.into_iter().unzip();
    let mut worker = Worker {
        requested,
        failed_this_pass: HashMap::new(),
        root,
        source,
        account,
        choice,
        db,
        prepared: HashMap::new(),
        cached_bytes: 0,
        pdf_unavailable,
        history: false,
        started: Instant::now(),
        gate_checked: None,
        yield_history: false,
        headers: HeaderCache::open(&store::header_cache_path(root))?,
        prepare,
    };
    for ((day, segment), group) in groups.into_iter().rev() {
        if store::is_cancel_requested(root) {
            return Err(EmailError::Cancelled);
        }
        worker.history = day < &today;
        if worker.history_waits() {
            if worker.requested.is_empty() {
                break;
            }
            if !group.values().any(|(_, d)| {
                requested_sources.contains(&d.key)
                    || d.parent
                        .parent_key
                        .as_ref()
                        .is_some_and(|key| requested_sources.contains(key))
            }) {
                continue;
            }
        }
        let Some(dir) = segment_dir(root, day, segment).filter(|dir| live(dir)) else {
            // A segment can come back through the owner's Restore action.
            // Leave the check open without bringing any content back ourselves.
            worker.db.execute(
                "UPDATE reports SET done=done+?1 WHERE account=?2",
                params![group.len() as i64, account],
            )?;
            if worker.history {
                history_done += group.len();
                save_progress(history_done)?;
            }
            continue;
        };
        let file = dir.join("conversation_transcript.jsonl");
        let before = fs::read_to_string(&file).map_err(storage)?;
        let rows = parse_rows(&before)?;
        if rows
            .first()
            .and_then(|r| r.pointer("/email/account"))
            .and_then(Value::as_str)
            != Some(account)
        {
            continue;
        }
        let mut output = String::new();
        let mut changed = false;
        let mut keys = Vec::new();
        let mut added = Vec::new();
        let mut requests = Vec::new();
        let mut run_id = "";
        for mut row in rows {
            if let Some((rec, d)) = row
                .pointer("/email/key")
                .and_then(Value::as_str)
                .and_then(|key| group.get(key))
            {
                let original = row.clone();
                worker.side(
                    &mut row,
                    &d.key,
                    choice.attachments != AttachmentScope::Names,
                    rec,
                    &mut added,
                )?;
                if let (Some(parent), Some(reply)) = (&d.parent.parent_key, row.get_mut("reply_to"))
                {
                    worker.side(
                        reply,
                        parent,
                        choice.attachments == AttachmentScope::Both,
                        rec,
                        &mut added,
                    )?;
                }
                let mut pending = false;
                for arr in [row.get("attachments"), row.pointer("/reply_to/attachments")]
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_array)
                {
                    pending |= arr.iter().any(|a| a["state"] == "pending");
                    requests.extend(
                        arr.iter()
                            .filter(|a| {
                                a["state"] != "pending" && a["state"] != "reader_unavailable"
                            })
                            .filter_map(|a| a["attachment_id"].as_str().map(str::to_owned)),
                    );
                }
                changed |= original != row;
                if !pending {
                    keys.push(&d.key);
                }
                run_id = &rec.run_id;
            }
            output.push_str(&row.to_string());
            output.push('\n');
        }
        if changed || !added.is_empty() {
            {
                let _lock = hold_lock(&dir, LockOptions::default()).map_err(storage)?;
                if !live(&dir) || fs::read_to_string(&file).map_err(storage)? != before {
                    continue;
                }
                atomic_replace(&file, output.as_bytes(), AtomicWriteOptions::default())
                    .map_err(storage)?;
            }
            if let Some(publish) = on_batch {
                publish(
                    run_id,
                    &[CreatedEmailSegment {
                        day: day.clone(),
                        segment: segment.clone(),
                        file,
                        attachment_files: added,
                    }],
                );
            }
        }
        let tx = worker.db.transaction()?;
        tx.execute(
            "UPDATE reports SET done=done+?1 WHERE account=?2",
            params![keys.len() as i64, account],
        )?;
        if worker.history {
            history_done += keys.len();
            save_progress(history_done)?;
        }
        for key in keys {
            tx.execute(
                "INSERT OR REPLACE INTO checked VALUES (?1,?2,?3)",
                params![account, key, policy],
            )?;
        }
        for id in requests {
            worker.requested.remove(&id);
            tx.execute("DELETE FROM requests WHERE id=?1", [id])?;
        }
        tx.commit()?;
    }
    Ok(())
}
