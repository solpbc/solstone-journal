// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Provider-specific completion-token request budgets.

pub fn generate_token_budget(
    provider: &str,
    max_output_tokens: u64,
    thinking_budget: Option<u64>,
) -> u64 {
    match provider {
        "google" => max_output_tokens
            .saturating_add(thinking_budget.unwrap_or(0))
            .min(65_535),
        "anthropic" if thinking_budget.is_some_and(|budget| budget > 0) => max_output_tokens.max(
            thinking_budget
                .expect("positive thinking budget was checked")
                .saturating_add(1_001),
        ),
        _ => max_output_tokens,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anthropic_budget_expands_for_thinking_and_passthrough_without_it() {
        assert_eq!(
            generate_token_budget("anthropic", 4_000, Some(5_000)),
            6_001
        );
        assert_eq!(generate_token_budget("anthropic", 4_000, None), 4_000);
        assert_eq!(generate_token_budget("anthropic", 4_000, Some(0)), 4_000);
    }

    #[test]
    fn google_budget_sums_thinking_and_clamps_to_limit() {
        assert_eq!(generate_token_budget("google", 4_000, Some(500)), 4_500);
        assert_eq!(generate_token_budget("google", 65_000, Some(1_000)), 65_535);
    }
}
