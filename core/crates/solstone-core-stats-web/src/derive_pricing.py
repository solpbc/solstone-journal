# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc
"""One-off generator for pricing_data.rs; run with genai-prices 0.0.55."""

from genai_prices import Usage, calc_price

MODELS = [
    "gpt-5.2", "gpt-5-mini", "gpt-5-nano", "gpt-5.2-pro",
    "claude-opus-4-6", "claude-sonnet-4-6", "claude-haiku-4-5",
    "gemini-3-pro-preview", "gemini-3-flash-preview", "gemini-2.5-flash-lite",
]

for model in MODELS:
    uncached = calc_price(Usage(input_tokens=1_000_000, output_tokens=0), model).total_price
    cached = calc_price(Usage(input_tokens=1_000_000, cache_read_tokens=1_000_000, output_tokens=0), model).total_price
    output = calc_price(Usage(output_tokens=1_000_000), model).total_price
    print(f'    ("{model}", Rate::new({uncached}, {cached}, {output})),')
