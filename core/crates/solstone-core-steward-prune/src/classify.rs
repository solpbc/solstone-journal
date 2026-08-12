// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::MAX_ROW_CONTENT_BYTES;
use crate::coerce::{Coercion, coerce};
use crate::rows::{Row, RowSplitter};
use crate::syntax::{SyntaxClass, recognize};
use crate::unicode::is_python_whitespace;

/// The observable action a filesystem adapter would take.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Disposition {
    Rewrite { dropped: u64 },
    NoChange,
    WholeNoop(WholeNoopReason),
}

/// Closed causes for the Python fail-safe whole-file no-op path.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WholeNoopReason {
    NumericOverflow,
    IntegerDigitLimit,
    InvalidUtf8,
    RecursionLimit,
    UnknownParserDifference,
}

impl WholeNoopReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NumericOverflow => "numeric-overflow",
            Self::IntegerDigitLimit => "integer-digit-limit",
            Self::InvalidUtf8 => "invalid-utf8",
            Self::RecursionLimit => "recursion-limit",
            Self::UnknownParserDifference => "unknown-parser-difference",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PruneClassification {
    pub output: Vec<u8>,
    pub aged: u64,
    pub malformed: u64,
    pub compatibility_kept: u64,
    pub disposition: Disposition,
}

#[derive(Clone, Copy)]
struct Decision<'a> {
    row: Row<'a>,
    keep: bool,
}

/// Classifies `steward.log` bytes without reading or writing the filesystem.
pub fn classify_prune(input: &[u8], now_ms: i64) -> PruneClassification {
    let cutoff = now_ms.saturating_sub(30 * 86_400_000);
    let mut decisions = Vec::new();
    let mut aged = 0_u64;
    let mut malformed = 0_u64;
    let mut compatibility_kept = 0_u64;

    for row in RowSplitter::new(input) {
        // The precedence is per physical row; raw splitting precedes UTF-8.
        if row.content.len() > MAX_ROW_CONTENT_BYTES {
            return whole_noop(input, WholeNoopReason::UnknownParserDifference);
        }
        let text = match core::str::from_utf8(row.content) {
            Ok(text) => text,
            Err(_) => return whole_noop(input, WholeNoopReason::InvalidUtf8),
        };
        if text
            .chars()
            .all(|character| is_python_whitespace(character as u32))
        {
            decisions.push(Decision { row, keep: true });
            continue;
        }
        match recognize(row.content) {
            SyntaxClass::IntegerDigitLimit => {
                return whole_noop(input, WholeNoopReason::IntegerDigitLimit);
            }
            SyntaxClass::RecursionLimit => {
                return whole_noop(input, WholeNoopReason::RecursionLimit);
            }
            SyntaxClass::Malformed => {
                malformed += 1;
                decisions.push(Decision { row, keep: false });
            }
            SyntaxClass::Unknown => {
                return whole_noop(input, WholeNoopReason::UnknownParserDifference);
            }
            SyntaxClass::Valid(facts) => {
                // json.loads converts all bare integer literals before it reaches
                // row.get("ts"), so this has higher precedence than coercion.
                if facts.has_integer_digit_limit {
                    return whole_noop(input, WholeNoopReason::IntegerDigitLimit);
                }
                let coercion = if facts.top_is_object {
                    facts.ts.map(|value| coerce(value, cutoff))
                } else {
                    None
                };
                if matches!(coercion, Some(Coercion::NumericOverflow)) {
                    return whole_noop(input, WholeNoopReason::NumericOverflow);
                }
                if facts.max_depth >= 10_001 {
                    return whole_noop(input, WholeNoopReason::RecursionLimit);
                }
                if facts.has_extended_outside_last_ts {
                    return whole_noop(input, WholeNoopReason::UnknownParserDifference);
                }
                let drop = matches!(coercion, Some(Coercion::Aged));
                if drop {
                    aged += 1;
                }
                let compatibility = facts.has_lone_surrogate
                    || (128..=10_000).contains(&facts.max_depth)
                    || matches!(
                        coercion,
                        Some(Coercion::Kept {
                            compatibility: true
                        })
                    );
                if !drop && compatibility {
                    compatibility_kept += 1;
                }
                decisions.push(Decision { row, keep: !drop });
            }
        }
    }
    let dropped = aged + malformed;
    if dropped == 0 {
        return PruneClassification {
            output: input.to_vec(),
            aged,
            malformed,
            compatibility_kept,
            disposition: Disposition::NoChange,
        };
    }
    let retained = decisions
        .iter()
        .filter(|decision| decision.keep)
        .map(|decision| decision.row.content.len() + decision.row.terminator.bytes().len())
        .sum();
    let mut output = Vec::with_capacity(retained);
    for decision in decisions.into_iter().filter(|decision| decision.keep) {
        output.extend_from_slice(decision.row.content);
        output.extend_from_slice(decision.row.terminator.bytes());
    }
    PruneClassification {
        output,
        aged,
        malformed,
        compatibility_kept,
        disposition: Disposition::Rewrite { dropped },
    }
}

fn whole_noop(input: &[u8], reason: WholeNoopReason) -> PruneClassification {
    PruneClassification {
        output: input.to_vec(),
        aged: 0,
        malformed: 0,
        compatibility_kept: 0,
        disposition: Disposition::WholeNoop(reason),
    }
}
