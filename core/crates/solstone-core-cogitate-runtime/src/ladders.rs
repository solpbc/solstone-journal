// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeSet;

use crate::events::{BudgetLadder, BudgetStage};

const CONTEXT_WARN_FRAC: f64 = 0.70;
const CONTEXT_FINAL_FRAC: f64 = 0.78;
const TURN_WARN_FRACS: [(u8, u8); 3] = [(50, 50), (75, 75), (90, 90)];

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct LadderEvent {
    pub ladder: BudgetLadder,
    pub stage: BudgetStage,
    pub message: Option<String>,
}

#[derive(Default)]
pub(crate) struct ResourceLadder {
    pub(crate) wrapup_nudged: bool,
    pub(crate) final_turn_armed: bool,
    pub(crate) force_stopped: bool,
}

impl ResourceLadder {
    /// This intentionally has no response-id argument or dedupe. Resource
    /// limits apply to every nonterminal action, unlike the turn ladder's
    /// response-id dedupe.
    pub(crate) fn check(
        &mut self,
        context_fraction: Option<f64>,
        finish_tool: &str,
    ) -> Option<LadderEvent> {
        if self.force_stopped {
            return None;
        }
        if self.final_turn_armed {
            self.force_stopped = true;
            return Some(LadderEvent {
                ladder: BudgetLadder::Resource,
                stage: BudgetStage::ForceStopped,
                message: None,
            });
        }
        if context_fraction.is_some_and(|fraction| fraction >= CONTEXT_FINAL_FRAC) {
            self.final_turn_armed = true;
            self.wrapup_nudged = true;
            return Some(LadderEvent {
                ladder: BudgetLadder::Resource,
                stage: BudgetStage::FinalTurn,
                message: Some(format!(
                    "Resource budget reached: this is the final turn. Stop gathering more context or using tools, and call {finish_tool} now with the best result available."
                )),
            });
        }
        if !self.wrapup_nudged
            && context_fraction.is_some_and(|fraction| fraction >= CONTEXT_WARN_FRAC)
        {
            self.wrapup_nudged = true;
            return Some(LadderEvent {
                ladder: BudgetLadder::Resource,
                stage: BudgetStage::Warning,
                message: Some(format!(
                    "Resource budget warning: this run is approaching its per-run resource budget. Finish useful work now and call {finish_tool} with the best complete result you can produce."
                )),
            });
        }
        None
    }
}

#[derive(Default)]
pub(crate) struct TurnLadder {
    pub(crate) observed_turns: usize,
    pub(crate) seen_response_ids: BTreeSet<String>,
    pub(crate) warnings_fired: BTreeSet<u8>,
    pub(crate) final_turn_armed: bool,
    pub(crate) force_stopped: bool,
}

impl TurnLadder {
    pub(crate) fn check(
        &mut self,
        response_id: &str,
        limit: usize,
        finish_tool: &str,
    ) -> Option<LadderEvent> {
        if self.force_stopped {
            return None;
        }
        if !response_id.is_empty() && self.seen_response_ids.contains(response_id) {
            return None;
        }
        if self.final_turn_armed {
            self.force_stopped = true;
            return Some(LadderEvent {
                ladder: BudgetLadder::Turn,
                stage: BudgetStage::ForceStopped,
                message: None,
            });
        }
        if !response_id.is_empty() {
            self.seen_response_ids.insert(response_id.to_owned());
        }
        self.observed_turns = self.observed_turns.saturating_add(1);
        let used = self.observed_turns;
        let remaining = limit.saturating_sub(used);
        if used >= limit.saturating_sub(1) {
            self.final_turn_armed = true;
            return Some(LadderEvent {
                ladder: BudgetLadder::Turn,
                stage: BudgetStage::FinalTurn,
                message: Some(format!(
                    "Turn budget reached: this is your last turn. Stop gathering more context or using tools, and call {finish_tool} now with the best result available."
                )),
            });
        }
        for (percent, numerator) in TURN_WARN_FRACS {
            let threshold = (usize::from(numerator) * limit).div_ceil(100);
            if !self.warnings_fired.contains(&percent) && used >= threshold {
                self.warnings_fired.insert(percent);
                let instruction = match percent {
                    50 => format!(
                        "Start converging on the final result and call {finish_tool} as soon as useful work is complete."
                    ),
                    75 => format!(
                        "Stop broad gathering; use the remaining turns only for synthesis and final checks, then call {finish_tool}."
                    ),
                    90 => format!(
                        "Finish now unless one more tool call is essential; call {finish_tool} with the best complete result available."
                    ),
                    _ => unreachable!("fixed warning table"),
                };
                return Some(LadderEvent {
                    ladder: BudgetLadder::Turn,
                    stage: BudgetStage::Warning,
                    message: Some(format!(
                        "Turn budget warning: you've used {percent}% of your turn budget so far: {used} of {limit} turns, {remaining} turns left. {instruction}"
                    )),
                });
            }
        }
        None
    }
}
