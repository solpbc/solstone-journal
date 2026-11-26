// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Header-only email models, MIME part tree, and owner choice types.
//!
//! A header-only scan records identity, reply headers, addresses, dates,
//! and the MIME part tree — never message bodies or attachment bytes.

use serde::{Deserialize, Serialize};

/// One RFC 5322 address header entry.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Addr {
    pub name: Option<String>,
    pub address: Option<String>,
}

impl Addr {
    #[must_use]
    pub fn lower(&self) -> Option<String> {
        self.address
            .as_ref()
            .map(|a| a.trim().to_ascii_lowercase())
            .filter(|a| !a.is_empty())
    }

    #[must_use]
    pub fn label(&self) -> String {
        self.name
            .as_deref()
            .map(str::trim)
            .filter(|n| !n.is_empty())
            .map(str::to_owned)
            .or_else(|| {
                self.address
                    .as_deref()
                    .map(str::trim)
                    .filter(|a| !a.is_empty())
                    .map(str::to_owned)
            })
            .unwrap_or_else(|| "Unknown".to_owned())
    }
}

/// One node of the MIME tree, numbered the IMAP way (`1`, `1.2`, ...).
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Part {
    pub path: Vec<u32>,
    /// `type/subtype`, lowercased.
    pub mime: String,
    pub charset: Option<String>,
    pub format: Option<String>,
    pub delsp: Option<String>,
    pub method: Option<String>,
    pub start: Option<String>,
    pub content_id: Option<String>,
    pub encoding: Option<String>,
    pub disposition: Option<String>,
    pub filename: Option<String>,
    /// Encoded octets, as the server reports them.
    pub size: u64,
    pub children: Vec<Part>,
}

impl Part {
    #[must_use]
    pub fn is_multipart(&self) -> bool {
        self.mime.starts_with("multipart/")
    }

    #[must_use]
    pub fn is_attachment(&self) -> bool {
        self.filename.is_some()
            || self
                .disposition
                .as_deref()
                .is_some_and(|d| d.eq_ignore_ascii_case("attachment"))
    }

    pub fn walk<'a>(&'a self, out: &mut Vec<&'a Part>) {
        out.push(self);
        for child in &self.children {
            child.walk(out);
        }
    }
}

/// Where a message sits in the scanned mailbox.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Location {
    pub mailbox: String,
    pub uid_validity: u32,
    pub uid: u32,
}

/// A named attachment or inline image (never fetched or stored in phase 1).
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct NamedAttachment {
    pub name: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub inline: bool,
}

/// Header-only representation of one scanned message.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Header {
    /// Stable identity within the account (`gm:<X-GM-MSGID>` on Gmail,
    /// otherwise `<mailbox>-<uidvalidity>-<uid>`).
    pub key: String,
    pub location: Location,
    pub gm_msgid: Option<String>,
    pub gm_thrid: Option<String>,
    pub labels: Vec<String>,
    pub flags: Vec<String>,
    pub internal_date_ms: Option<i64>,
    pub size: u64,
    pub message_id: Option<String>,
    pub in_reply_to: Vec<String>,
    pub references: Vec<String>,
    pub from: Vec<Addr>,
    pub to: Vec<Addr>,
    pub cc: Vec<Addr>,
    pub date_ms: Option<i64>,
    pub subject: Option<String>,
    pub auto_submitted: Option<String>,
    pub list_id: Option<String>,
    pub tree: Part,
}

impl Header {
    #[must_use]
    pub fn is_sent(&self) -> bool {
        self.labels.iter().any(|l| l.eq_ignore_ascii_case("\\Sent"))
    }

    #[must_use]
    pub fn is_draft(&self) -> bool {
        self.labels
            .iter()
            .any(|l| l.eq_ignore_ascii_case("\\Draft"))
            || self.flags.iter().any(|f| f.eq_ignore_ascii_case("\\Draft"))
    }

    #[must_use]
    pub fn when_ms(&self) -> Option<i64> {
        self.date_ms.or(self.internal_date_ms)
    }

    #[must_use]
    pub fn calendar_method(&self) -> Option<String> {
        let mut all = Vec::new();
        self.tree.walk(&mut all);
        all.iter()
            .find(|p| p.mime == "text/calendar")
            .map(|p| p.method.clone().unwrap_or_else(|| "unknown".into()))
    }

    #[must_use]
    pub fn is_report(&self) -> bool {
        self.tree.mime == "multipart/report"
    }

    /// Whether the message carries a forwarded `message/rfc822` MIME part.
    #[must_use]
    pub fn has_forwarded_rfc822(&self) -> bool {
        let mut all = Vec::new();
        self.tree.walk(&mut all);
        all.iter().any(|p| p.mime == "message/rfc822")
    }

    /// Non-inline file attachment names.
    #[must_use]
    pub fn attachment_names(&self) -> Vec<String> {
        let mut all = Vec::new();
        self.tree.walk(&mut all);
        all.iter()
            .filter(|p| !p.is_multipart() && p.is_attachment())
            .filter(|p| {
                !p.disposition
                    .as_deref()
                    .is_some_and(|d| d.eq_ignore_ascii_case("inline"))
            })
            .map(|p| p.filename.clone().unwrap_or_else(|| p.mime.clone()))
            .collect()
    }

    /// Non-text parts shown inline (embedded images, etc.). Named, never read.
    #[must_use]
    pub fn embedded_names(&self) -> Vec<String> {
        let mut all = Vec::new();
        self.tree.walk(&mut all);
        all.iter()
            .filter(|p| !p.is_multipart() && !p.mime.starts_with("text/"))
            .filter(|p| {
                p.disposition
                    .as_deref()
                    .is_some_and(|d| d.eq_ignore_ascii_case("inline"))
                    || (p.content_id.is_some() && !p.is_attachment())
            })
            .map(|p| p.filename.clone().unwrap_or_else(|| p.mime.clone()))
            .collect()
    }

    /// Combined named attachments + inline images in deterministic order.
    #[must_use]
    pub fn named_attachments(&self) -> Vec<NamedAttachment> {
        let mut out = Vec::new();
        for name in self.attachment_names() {
            out.push(NamedAttachment {
                name,
                inline: false,
            });
        }
        for name in self.embedded_names() {
            out.push(NamedAttachment { name, inline: true });
        }
        out
    }
}

/// The text leaves phase 1 reads: at most one native plain and one HTML leaf.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct BodyChoice {
    pub plain: Option<Part>,
    pub html: Option<Part>,
    pub skipped_attachments: usize,
}

/// Choose the body leaves from the MIME tree without descending into
/// attachments or nested `message/rfc822` parts.
#[must_use]
pub fn choose_body(tree: &Part) -> BodyChoice {
    let mut choice = BodyChoice::default();
    choose(tree, &mut choice);
    let mut all = Vec::new();
    tree.walk(&mut all);
    choice.skipped_attachments = all
        .iter()
        .filter(|p| !p.is_multipart() && (p.is_attachment() || p.mime == "message/rfc822"))
        .count();
    choice
}

fn choose(part: &Part, out: &mut BodyChoice) {
    if part.is_attachment() || part.mime == "message/rfc822" {
        return;
    }
    match part.mime.as_str() {
        "text/plain" => {
            if out.plain.is_none() {
                out.plain = Some(leaf(part));
            }
        }
        "text/html" => {
            if out.html.is_none() {
                out.html = Some(leaf(part));
            }
        }
        "multipart/alternative" => {
            // The last representation of each kind is the richest the client sent.
            let mut plain = None;
            let mut html = None;
            for child in &part.children {
                let mut sub = BodyChoice::default();
                choose(child, &mut sub);
                if sub.plain.is_some() {
                    plain = sub.plain;
                }
                if sub.html.is_some() {
                    html = sub.html;
                }
            }
            if out.plain.is_none() {
                out.plain = plain;
            }
            if out.html.is_none() {
                out.html = html;
            }
        }
        "multipart/related" => {
            let root = part
                .start
                .as_deref()
                .map(|s| s.trim_matches(['<', '>']))
                .and_then(|start| {
                    part.children.iter().find(|c| {
                        c.content_id
                            .as_deref()
                            .map(|id| id.trim_matches(['<', '>']))
                            == Some(start)
                    })
                })
                .or_else(|| part.children.first());
            if let Some(root) = root {
                choose(root, out);
            }
        }
        m if m.starts_with("multipart/") => {
            for child in &part.children {
                if child.is_attachment() || child.mime == "message/rfc822" {
                    continue;
                }
                choose(child, out);
                if out.plain.is_some() || out.html.is_some() {
                    break;
                }
            }
        }
        _ => {}
    }
}

fn leaf(part: &Part) -> Part {
    Part {
        children: Vec::new(),
        ..part.clone()
    }
}

/// Content-free reference back to the message in the mailbox.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Reference {
    pub account: String,
    pub message_id: Option<String>,
    pub gm_msgid: Option<String>,
    pub gm_thrid: Option<String>,
    pub mailbox: String,
    pub uid_validity: u32,
    pub uid: u32,
}

impl Reference {
    #[must_use]
    pub fn of(account: &str, h: &Header) -> Self {
        Self {
            account: account.to_owned(),
            message_id: h.message_id.clone(),
            gm_msgid: h.gm_msgid.clone(),
            gm_thrid: h.gm_thrid.clone(),
            mailbox: h.location.mailbox.clone(),
            uid_validity: h.location.uid_validity,
            uid: h.location.uid,
        }
    }
}

/// The three phase-1 message types the owner can tick or untick.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageType {
    Replies,
    NewMessages,
    Forwards,
}

impl MessageType {
    pub const ALL: [Self; 3] = [Self::Replies, Self::NewMessages, Self::Forwards];

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Replies => "replies",
            Self::NewMessages => "new_messages",
            Self::Forwards => "forwards",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim() {
            "replies" => Some(Self::Replies),
            "new_messages" => Some(Self::NewMessages),
            "forwards" => Some(Self::Forwards),
            _ => None,
        }
    }
}

/// Canonical range presets shared across selective-sync sources.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RangePreset {
    #[serde(rename = "30d")]
    Last30d,
    #[serde(rename = "6m")]
    Last6m,
    #[serde(rename = "1y")]
    Last1y,
    #[serde(rename = "all")]
    All,
    #[serde(rename = "none")]
    None,
}

impl RangePreset {
    pub const ALL_PRESETS: [Self; 5] = [
        Self::Last30d,
        Self::Last6m,
        Self::Last1y,
        Self::All,
        Self::None,
    ];

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Last30d => "30d",
            Self::Last6m => "6m",
            Self::Last1y => "1y",
            Self::All => "all",
            Self::None => "none",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim() {
            "30d" => Some(Self::Last30d),
            "6m" => Some(Self::Last6m),
            "1y" => Some(Self::Last1y),
            "all" => Some(Self::All),
            "none" => Some(Self::None),
            _ => None,
        }
    }

    /// Rank for widening comparisons (`none` < `30d` < `6m` < `1y` < `all`).
    #[must_use]
    pub fn rank(self) -> u8 {
        match self {
            Self::None => 0,
            Self::Last30d => 1,
            Self::Last6m => 2,
            Self::Last1y => 3,
            Self::All => 4,
        }
    }
}
