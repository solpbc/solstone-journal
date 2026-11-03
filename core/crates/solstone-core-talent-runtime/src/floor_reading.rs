// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! A smaller text for a window too small for a segment source: it reaches the
//! model through `overflow_text`, so a larger window still reads the source
//! whole (docs/GENERATE.md § `overflow_text`).

use serde_json::{Map, Value, json};

/// Room for the request's message framing and tokenizer boundary effects
/// between separately counted parts.
const FRAMING_TOKENS: u32 = 256;

#[derive(Clone, Debug)]
pub(crate) struct FloorReading {
    pub(crate) text: String,
    pub(crate) pages_not_whole: usize,
    pub(crate) chars_left_out: usize,
}

impl FloorReading {
    /// What a result read from this text records as left out, if it left anything out.
    pub(crate) fn input_budget(&self) -> Option<Value> {
        (self.pages_not_whole > 0).then(|| {
            json!({
                "clipped": true,
                "used_overflow_text": true,
                "dropped_entries": self.pages_not_whole,
                "dropped_chars": self.chars_left_out,
            })
        })
    }
}

/// The first text part's room at the floor window. The Generate boundary takes
/// an overflow text only when it fits with the full reply, which can be larger
/// than the input budget's capped reservation.
pub(crate) fn floor_budget(config: &Map<String, Value>) -> Option<u32> {
    let max_output_tokens = config
        .get("max_output_tokens")
        .and_then(Value::as_u64)
        .and_then(|tokens| u32::try_from(tokens).ok())?;
    let preserved: u32 = ["user_instruction", "prompt"]
        .iter()
        .filter_map(|key| config.get(*key).and_then(Value::as_str))
        .map(solstone_core_tokens::count)
        .sum();
    Some(
        solstone_core_local::plan::LOCAL_MIN_CONTEXT_TOKENS
            .saturating_sub(max_output_tokens)
            .saturating_sub(preserved + 2 * FRAMING_TOKENS),
    )
}
