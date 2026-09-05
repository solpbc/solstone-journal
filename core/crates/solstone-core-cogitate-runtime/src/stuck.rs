// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Stuck detection preserves the retired runtime's bounded-history rules.
//!
//! It examines the last 20 history entries, retaining only entries after the
//! last user-role entry when one occurs in that window. The four live patterns
//! are repeating action→observation (4), repeating action→error (3), agent
//! monologue (3), and alternating action/observation (6). The fifth
//! context-window-error loop was disabled in the retired runtime; it is not
//! ported beyond preserving that fact.
//!
//! Equality is defined over this crate's types, rather than SDK event classes:
//! an action has the same tool name and JSON arguments, and an observation has
//! the same tool name and output text. A monologue is three consecutive
//! assistant-message turns with no tool call or intervening user message,
//! regardless of message content. The `agent_stuck` deterministic-failure cap is 2 in
//! `solstone-core-cogitate`'s `DETERMINISTIC_FAILURE_CAPS` and is enforced by a
//! caller, not this crate. A genuine detector trip and either budget ladder's
//! stage-3 force-stop pause share that `agent_stuck` condition, but the outcome
//! tail checks context/turn exhaustion before stuck/paused. Therefore a
//! force-stopped run reports its own budget reason first and never also
//! double-reports as `agent_stuck`.

use std::collections::VecDeque;

use serde_json::Value;

const WINDOW: usize = 20;

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum HistoryEntry {
    User,
    AssistantText(String),
    Action {
        tool: String,
        arguments: Value,
    },
    Observation {
        tool: String,
        output: String,
        is_error: bool,
    },
}

#[derive(Default)]
pub(crate) struct StuckDetector {
    entries: VecDeque<HistoryEntry>,
}

impl StuckDetector {
    pub(crate) fn push(&mut self, entry: HistoryEntry) {
        self.entries.push_back(entry);
        if self.entries.len() > WINDOW {
            self.entries.pop_front();
        }
    }

    pub(crate) fn is_stuck(&self) -> bool {
        let entries = self.after_last_user();
        if entries.len() < 3 {
            return false;
        }
        let actions = entries
            .iter()
            .rev()
            .filter_map(|entry| match entry {
                HistoryEntry::Action { tool, arguments } => Some((tool, arguments)),
                _ => None,
            })
            .collect::<Vec<_>>();
        let observations = entries
            .iter()
            .rev()
            .filter_map(|entry| match entry {
                HistoryEntry::Observation {
                    tool,
                    output,
                    is_error,
                } => Some((tool, output, is_error)),
                _ => None,
            })
            .collect::<Vec<_>>();
        if actions.len() >= 4
            && observations.len() >= 4
            && actions[..4].iter().all(|item| *item == actions[0])
            && observations[..4]
                .iter()
                .all(|item| *item == observations[0])
        {
            return true;
        }
        if actions.len() >= 3
            && observations.len() >= 3
            && actions[..3].iter().all(|item| *item == actions[0])
            && observations[..3].iter().all(|(_, _, is_error)| **is_error)
        {
            return true;
        }
        if entries
            .iter()
            .rev()
            .take(3)
            .all(|entry| matches!(entry, HistoryEntry::AssistantText(_)))
        {
            return true;
        }
        if entries.len() >= 6
            && actions.len() >= 6
            && observations.len() >= 6
            && (0..4).all(|index| actions[index] == actions[index + 2])
            && (0..4).all(|index| observations[index] == observations[index + 2])
        {
            return true;
        }
        // The retired runtime's fifth context-window-error pattern was
        // disabled; preserve that behavior.
        false
    }

    fn after_last_user(&self) -> Vec<&HistoryEntry> {
        let entries = self.entries.iter().collect::<Vec<_>>();
        entries
            .iter()
            .rposition(|entry| matches!(entry, HistoryEntry::User))
            .map_or(entries.clone(), |index| entries[index + 1..].to_vec())
    }
}
