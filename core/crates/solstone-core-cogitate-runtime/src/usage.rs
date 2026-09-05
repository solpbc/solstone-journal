// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::{Value, json};

/// Provider-neutral token accounting, named to match Python's usage snapshot.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cached_tokens: u64,
    pub cache_creation_tokens: u64,
    pub reasoning_tokens: u64,
    pub requests: u64,
}

impl Usage {
    /// Return the stable terminal-wire representation of this run's usage.
    pub fn to_wire_value(&self) -> Value {
        json!({
            "input_tokens": self.input_tokens,
            "output_tokens": self.output_tokens,
            "cached_tokens": self.cached_tokens,
            "cache_creation_tokens": self.cache_creation_tokens,
            "reasoning_tokens": self.reasoning_tokens,
            "requests": self.requests,
        })
    }

    pub fn total_tokens(&self) -> u64 {
        self.input_tokens.saturating_add(self.output_tokens)
    }

    /// Normalize one already-parsed generate-wire conversation turn.
    pub fn from_turn(value: &Value) -> Self {
        Self {
            input_tokens: number(value, "input_tokens"),
            output_tokens: number(value, "output_tokens"),
            cached_tokens: first_number(
                value,
                &["cached_tokens", "cached_input_tokens", "cache_read_tokens"],
            ),
            cache_creation_tokens: first_number(
                value,
                &[
                    "cache_creation_tokens",
                    "cache_creation_input_tokens",
                    "cache_write_tokens",
                ],
            ),
            reasoning_tokens: number(value, "reasoning_tokens"),
            requests: 1,
        }
    }

    pub fn add_assign(&mut self, turn: &Self) {
        self.input_tokens = self.input_tokens.saturating_add(turn.input_tokens);
        self.output_tokens = self.output_tokens.saturating_add(turn.output_tokens);
        self.cached_tokens = self.cached_tokens.saturating_add(turn.cached_tokens);
        self.cache_creation_tokens = self
            .cache_creation_tokens
            .saturating_add(turn.cache_creation_tokens);
        self.reasoning_tokens = self.reasoning_tokens.saturating_add(turn.reasoning_tokens);
        self.requests = self.requests.saturating_add(turn.requests);
    }
}

fn number(value: &Value, key: &str) -> u64 {
    value.get(key).and_then(Value::as_u64).unwrap_or(0)
}

fn first_number(value: &Value, keys: &[&str]) -> u64 {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(Value::as_u64))
        .unwrap_or(0)
}
