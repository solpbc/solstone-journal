// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Deterministic selection of owner-sent messages, parent resolution (two tiers),
//! left-out categorization, and versioned re-reach for skipped mail (AC 6).

use std::collections::{BTreeMap, BTreeSet, HashMap};

use serde::{Deserialize, Serialize};

use crate::model::{Header, MessageType, Reference};
use crate::text::is_forward_subject;

/// Current selection rules version stamped onto every persisted decision.
pub const RULES_VERSION: u32 = 1;

/// Releases can bump this threshold to trigger re-evaluation of previously
/// skipped decisions with `rules_version < STALE_SKIP_BEFORE_VERSION`.
/// Re-reach only ever adds newly qualifying mail and never modifies or
/// removes already-landed exchanges.
pub const STALE_SKIP_BEFORE_VERSION: u32 = 1;

/// Left-out reason buckets shown on `/app/sources/` and `/app/sources/email`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LeftOutBucket {
    CalendarResponses,
    AutomaticReplies,
    Drafts,
    DeliveryReports,
}

impl LeftOutBucket {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::CalendarResponses => "calendar_responses",
            Self::AutomaticReplies => "automatic_replies",
            Self::Drafts => "drafts",
            Self::DeliveryReports => "delivery_reports",
        }
    }
}

/// Parent match strength stored alongside every resolved reply.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParentMatchRule {
    InReplyTo,
    ReferencesOnly,
}

impl ParentMatchRule {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::InReplyTo => "in_reply_to",
            Self::ReferencesOnly => "references_only",
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ParentOutcome {
    /// `none` (not a reply), `certain`, or `unresolved`.
    pub outcome: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_match_rule: Option<ParentMatchRule>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_message_id: Option<String>,
}

/// One durable selection decision for an owner-side message.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Decision {
    pub key: String,
    pub reference: Reference,
    /// `selected` or `left_out`.
    pub outcome: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_type: Option<MessageType>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub left_out_bucket: Option<LeftOutBucket>,
    pub parent: ParentOutcome,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sent_ms: Option<i64>,
    pub rules_version: u32,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct LeftOutCounts {
    pub total: usize,
    pub calendar_responses: usize,
    pub automatic_replies: usize,
    pub drafts: usize,
    pub delivery_reports: usize,
}

impl LeftOutCounts {
    pub fn record(&mut self, bucket: LeftOutBucket) {
        self.total += 1;
        match bucket {
            LeftOutBucket::CalendarResponses => self.calendar_responses += 1,
            LeftOutBucket::AutomaticReplies => self.automatic_replies += 1,
            LeftOutBucket::Drafts => self.drafts += 1,
            LeftOutBucket::DeliveryReports => self.delivery_reports += 1,
        }
    }
}

/// Learn owner email aliases from the account address plus `From:` addresses
/// observed on `\Sent` messages (excluding obvious automated daemon addresses).
#[must_use]
pub fn learn_owner_aliases(account: &str, all: &[Header]) -> Vec<String> {
    let mut aliases = BTreeSet::new();
    let acct = account.trim().to_ascii_lowercase();
    if !acct.is_empty() {
        aliases.insert(acct);
    }
    let mut sent_counts: BTreeMap<String, usize> = BTreeMap::new();
    for h in all {
        if !h.is_sent() || h.is_draft() {
            continue;
        }
        if let Some(addr) = h.from.first().and_then(crate::model::Addr::lower)
            && !is_daemon_address(&addr)
        {
            *sent_counts.entry(addr).or_default() += 1;
        }
    }
    for (addr, count) in sent_counts {
        if count >= 1 {
            aliases.insert(addr);
        }
    }
    aliases.into_iter().collect()
}

fn is_daemon_address(addr: &str) -> bool {
    let local = addr.split('@').next().unwrap_or(addr);
    matches!(
        local,
        "mailer-daemon" | "postmaster" | "noreply" | "no-reply" | "donotreply" | "do-not-reply"
    )
}

#[must_use]
pub fn is_owner(h: &Header, aliases: &[String]) -> bool {
    h.from
        .first()
        .and_then(crate::model::Addr::lower)
        .is_some_and(|a| aliases.iter().any(|x| x.eq_ignore_ascii_case(&a)))
}

/// Classify an eligible sent message into one of the three phase-1 message types
/// using headers + MIME structure alone (zero bodies read).
#[must_use]
pub fn classify_message_type(h: &Header) -> MessageType {
    if is_forward_subject(h.subject.as_deref()) || h.has_forwarded_rfc822() {
        MessageType::Forwards
    } else if !h.in_reply_to.is_empty() || !h.references.is_empty() {
        MessageType::Replies
    } else {
        MessageType::NewMessages
    }
}

/// Resolve the single message a reply answered using the two-tier rule:
/// 1. `In-Reply-To` (single non-self Message-ID uniquely matching one header,
///    not conflicting with `References`'s tail) -> `certain`, `in_reply_to`.
/// 2. `References` tail when `In-Reply-To` is absent (uniquely matching one
///    non-self header) -> `certain`, `references_only`.
#[must_use]
pub fn resolve_parent(h: &Header, by_id: &HashMap<&str, Vec<&Header>>) -> ParentOutcome {
    let unresolved = |reason: &str, target: Option<&String>| ParentOutcome {
        outcome: "unresolved".into(),
        reason: Some(reason.into()),
        parent_key: None,
        parent_match_rule: None,
        target_message_id: target.cloned(),
    };
    match h.in_reply_to.as_slice() {
        [] if h.references.is_empty() => ParentOutcome {
            outcome: "none".into(),
            ..ParentOutcome::default()
        },
        [] => {
            let Some(id) = h.references.last() else {
                return ParentOutcome {
                    outcome: "none".into(),
                    ..ParentOutcome::default()
                };
            };
            if h.message_id.as_deref() == Some(id.as_str()) {
                return unresolved("self_reference", Some(id));
            }
            let targets = by_id.get(id.as_str()).map_or(&[][..], Vec::as_slice);
            match targets {
                [] => unresolved("missing_target", Some(id)),
                [one] => ParentOutcome {
                    outcome: "certain".into(),
                    reason: None,
                    parent_key: Some(one.key.clone()),
                    parent_match_rule: Some(ParentMatchRule::ReferencesOnly),
                    target_message_id: Some(id.clone()),
                },
                _ => unresolved("duplicate_target", Some(id)),
            }
        }
        [_, _, ..] => unresolved("multiple_ids", h.in_reply_to.first()),
        [id] => {
            if h.message_id.as_deref() == Some(id.as_str()) {
                return unresolved("self_reference", Some(id));
            }
            let targets = by_id.get(id.as_str()).map_or(&[][..], Vec::as_slice);
            match targets {
                [] => unresolved("missing_target", Some(id)),
                [one] => {
                    if h.references.last().is_some_and(|last| last != id) {
                        unresolved("conflict", Some(id))
                    } else {
                        ParentOutcome {
                            outcome: "certain".into(),
                            reason: None,
                            parent_key: Some(one.key.clone()),
                            parent_match_rule: Some(ParentMatchRule::InReplyTo),
                            target_message_id: Some(id.clone()),
                        }
                    }
                }
                _ => unresolved("duplicate_target", Some(id)),
            }
        }
    }
}

/// Evaluate all scanned headers into durable [`Decision`] records, applying
/// versioned re-reach rules cleanly:
/// - Already-landed keys are never downgraded or modified (AC 6).
/// - Existing `left_out` decisions with `rules_version < stale_before_version`
///   are re-evaluated under `rules_version`, potentially qualifying for sync.
/// - Existing decisions at or above `stale_before_version` are refreshed when
///   unchanged or newly discovered.
#[must_use]
pub fn evaluate_decisions(
    account: &str,
    owner_aliases: &[String],
    all: &[Header],
    existing: &HashMap<String, Decision>,
    landed_keys: &BTreeSet<String>,
    rules_version: u32,
    stale_before_version: u32,
) -> Vec<Decision> {
    let mut by_id: HashMap<&str, Vec<&Header>> = HashMap::new();
    for h in all {
        if let Some(id) = h.message_id.as_deref() {
            by_id.entry(id).or_default().push(h);
        }
    }

    let mut out = Vec::new();
    let mut seen_keys = BTreeSet::new();

    for h in all {
        // Already-landed exchanges are immutable under selection upgrades (AC 6).
        if landed_keys.contains(&h.key)
            && let Some(prev) = existing.get(&h.key)
        {
            seen_keys.insert(h.key.clone());
            out.push(prev.clone());
            continue;
        }

        // Preserve non-stale prior skip decisions without re-judging them;
        // re-evaluate any skipped decision stamped below `stale_before_version`.
        if let Some(prev) = existing.get(&h.key)
            && prev.outcome == "left_out"
            && prev.rules_version >= stale_before_version
        {
            seen_keys.insert(h.key.clone());
            out.push(prev.clone());
            continue;
        }

        let owner = is_owner(h, owner_aliases);
        let sent = h.is_sent();
        if !sent && !h.is_draft() {
            continue;
        }
        // Only owner-authored drafts count in owner-side draft totals.
        if h.is_draft() && !sent && !owner && !h.from.is_empty() {
            continue;
        }

        let (reason, bucket) = if h.is_draft() {
            (Some("draft"), Some(LeftOutBucket::Drafts))
        } else if h.calendar_method().is_some() {
            (
                Some("calendar_part"),
                Some(LeftOutBucket::CalendarResponses),
            )
        } else if h.auto_submitted.as_deref().is_some_and(|v| v != "no") {
            (
                Some("auto_submitted"),
                Some(LeftOutBucket::AutomaticReplies),
            )
        } else if h.is_report() {
            (
                Some("delivery_report"),
                Some(LeftOutBucket::DeliveryReports),
            )
        } else if !owner {
            // In Sent but from an automated/non-owner daemon address.
            (
                Some("auto_submitted"),
                Some(LeftOutBucket::AutomaticReplies),
            )
        } else {
            (None, None)
        };

        let (outcome, message_type, parent) = if reason.is_some() {
            ("left_out".to_owned(), None, ParentOutcome::default())
        } else {
            let msg_type = classify_message_type(h);
            let parent = resolve_parent(h, &by_id);
            ("selected".to_owned(), Some(msg_type), parent)
        };

        seen_keys.insert(h.key.clone());
        out.push(Decision {
            key: h.key.clone(),
            reference: Reference::of(account, h),
            outcome,
            message_type,
            reason: reason.map(str::to_owned),
            left_out_bucket: bucket,
            parent,
            sent_ms: h.when_ms(),
            rules_version,
        });
    }

    // Preserve any landed decision whose message was later deleted from the mailbox (AC 18).
    for (key, prev) in existing {
        if landed_keys.contains(key) && !seen_keys.contains(key) {
            out.push(prev.clone());
        }
    }

    out.sort_by(|a, b| a.key.cmp(&b.key));
    out
}
