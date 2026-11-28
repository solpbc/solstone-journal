// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Regenerable SQLite header scan cache under `cache/email/headers.sqlite`.
//!
//! Deleting this file loses no owner choices, skip decisions, or landed
//! exchanges — the next scan rebuilds it cleanly from the mailbox.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use rusqlite::{Connection, OptionalExtension, params};

use crate::error::{EmailError, Result};
use crate::imap::{ImapSource, MailboxInfo};
use crate::model::Header;
use crate::select::Decision;

/// Persisted scan cursor for one IMAP mailbox.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScanCursor {
    pub mailbox: String,
    pub uid_validity: u32,
    pub last_uid: u32,
    pub exists_count: u32,
    pub complete: bool,
}

pub struct HeaderCache {
    conn: Connection,
}

impl HeaderCache {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| EmailError::Storage(e.to_string()))?;
        }
        let conn = Connection::open(path)?;
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=NORMAL;
             CREATE TABLE IF NOT EXISTS messages (
                 key TEXT PRIMARY KEY,
                 message_id TEXT,
                 uid INTEGER NOT NULL DEFAULT 0,
                 json TEXT NOT NULL
             );
             CREATE INDEX IF NOT EXISTS messages_by_id ON messages(message_id);
             CREATE TABLE IF NOT EXISTS scan_state (
                 mailbox TEXT PRIMARY KEY,
                 uid_validity INTEGER NOT NULL,
                 last_uid INTEGER NOT NULL,
                 exists_count INTEGER NOT NULL DEFAULT 0,
                 complete INTEGER NOT NULL DEFAULT 0
             );",
        )?;
        Ok(Self { conn })
    }

    pub fn cursor(&self, mailbox: &str) -> Result<Option<ScanCursor>> {
        let row = self
            .conn
            .query_row(
                "SELECT mailbox, uid_validity, last_uid, exists_count, complete FROM scan_state WHERE mailbox = ?1",
                params![mailbox],
                |r| {
                    Ok(ScanCursor {
                        mailbox: r.get(0)?,
                        uid_validity: r.get(1)?,
                        last_uid: r.get(2)?,
                        exists_count: r.get(3)?,
                        complete: r.get::<_, i64>(4)? != 0,
                    })
                },
            )
            .optional()?;
        Ok(row)
    }

    /// Reset cached rows when a mailbox's `UIDVALIDITY` changes.
    pub fn clear_all(&mut self) -> Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute("DELETE FROM messages", [])?;
        tx.execute("DELETE FROM scan_state", [])?;
        tx.commit()?;
        Ok(())
    }

    /// Store one scanned UID page and advance the scan cursor atomically.
    pub fn put_page(
        &mut self,
        mailbox: &str,
        uid_validity: u32,
        last_uid: u32,
        exists_count: u32,
        complete: bool,
        rows: &[Header],
    ) -> Result<()> {
        let tx = self.conn.transaction()?;
        for h in rows {
            let existing: Option<String> = tx
                .query_row(
                    "SELECT json FROM messages WHERE key = ?1",
                    params![h.key],
                    |r| r.get(0),
                )
                .optional()?;
            let merged = match existing {
                Some(raw) => {
                    let mut first: Header = serde_json::from_str(&raw)
                        .map_err(|e| EmailError::Storage(e.to_string()))?;
                    for label in &h.labels {
                        if !first.labels.contains(label) {
                            first.labels.push(label.clone());
                        }
                    }
                    if h.attachment_names_checked && !first.attachment_names_checked {
                        first.tree = h.tree.clone();
                        first.attachment_names_checked = true;
                    }
                    first.location = h.location.clone();
                    first
                }
                None => h.clone(),
            };
            let encoded =
                serde_json::to_string(&merged).map_err(|e| EmailError::Storage(e.to_string()))?;
            tx.execute(
                "INSERT INTO messages(key, message_id, uid, json) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(key) DO UPDATE SET
                    message_id = excluded.message_id,
                    uid = excluded.uid,
                    json = excluded.json",
                params![merged.key, merged.message_id, merged.location.uid, encoded],
            )?;
        }
        tx.execute(
            "INSERT INTO scan_state(mailbox, uid_validity, last_uid, exists_count, complete)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(mailbox) DO UPDATE SET
                uid_validity = excluded.uid_validity,
                last_uid = excluded.last_uid,
                exists_count = excluded.exists_count,
                complete = excluded.complete",
            params![
                mailbox,
                uid_validity,
                last_uid,
                exists_count,
                i64::from(complete),
            ],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Upgrade the projections that could have lost an extended filename.
    /// Preserve the scan cursor, decisions and all non-MIME cached fields.
    pub(crate) fn refresh_attachment_names(
        &mut self,
        source: &mut dyn ImapSource,
        mailbox: &MailboxInfo,
        headers: &mut [Header],
        decisions: &[Decision],
        cancelled: impl Fn() -> bool,
    ) -> Result<()> {
        let selected: HashSet<&str> = decisions
            .iter()
            .filter(|d| d.outcome == "selected")
            .flat_map(|d| std::iter::once(d.key.as_str()).chain(d.parent.parent_key.as_deref()))
            .collect();
        let pending: Vec<usize> = headers
            .iter()
            .enumerate()
            .filter_map(|(i, h)| {
                if h.attachment_names_checked
                    || !selected.contains(h.key.as_str())
                    || h.gm_msgid.is_none()
                    || h.location.mailbox != mailbox.name
                    || h.location.uid_validity != mailbox.uid_validity
                {
                    return None;
                }
                let mut parts = Vec::new();
                h.tree.walk(&mut parts);
                parts
                    .iter()
                    .any(|p| {
                        p.filename.is_none()
                            && !p.is_multipart()
                            && (p.is_attachment()
                                || !matches!(p.mime.as_str(), "text/html" | "text/plain")
                                || (p.path.len() > 1
                                    && p.content_id.is_none()
                                    && p.disposition.is_some()))
                    })
                    .then_some(i)
            })
            .collect();
        for batch in pending.chunks(50) {
            if cancelled() {
                return Err(EmailError::Cancelled);
            }
            let uids: Vec<_> = batch.iter().map(|i| headers[*i].location.uid).collect();
            let mut reads: HashMap<_, _> = source
                .scan_uids(mailbox, &uids)?
                .into_iter()
                .map(|h| (h.location.uid, h))
                .collect();
            let tx = self.conn.transaction()?;
            for &i in batch {
                let h = &mut headers[i];
                if let Some(row) = reads.remove(&h.location.uid) {
                    // An anomalous response can never change the remembered message.
                    if row.gm_msgid != h.gm_msgid
                        || row.location != h.location
                        || !row.attachment_names_checked
                    {
                        continue;
                    }
                    h.tree = row.tree;
                }
                // A nonexistent UID in this UIDVALIDITY cannot reappear. Keep
                // its existing display names for already-landed conversations.
                h.attachment_names_checked = true;
                let json =
                    serde_json::to_string(h).map_err(|e| EmailError::Storage(e.to_string()))?;
                tx.execute(
                    "UPDATE messages SET json=?1 WHERE key=?2",
                    params![json, h.key],
                )?;
            }
            tx.commit()?;
        }
        Ok(())
    }

    pub fn all(&self) -> Result<Vec<Header>> {
        let mut stmt = self
            .conn
            .prepare("SELECT json FROM messages ORDER BY uid ASC, key ASC")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for row in rows {
            let raw = row?;
            let header: Header =
                serde_json::from_str(&raw).map_err(|e| EmailError::Storage(e.to_string()))?;
            out.push(header);
        }
        Ok(out)
    }

    pub fn get(&self, key: &str) -> Result<Option<Header>> {
        let raw: Option<String> = self
            .conn
            .query_row(
                "SELECT json FROM messages WHERE key = ?1",
                params![key],
                |r| r.get(0),
            )
            .optional()?;
        raw.map(|s| serde_json::from_str(&s).map_err(|e| EmailError::Storage(e.to_string())))
            .transpose()
    }

    pub fn count(&self) -> Result<u64> {
        let n: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM messages", [], |r| r.get(0))?;
        Ok(u64::try_from(n).unwrap_or(0))
    }
}
