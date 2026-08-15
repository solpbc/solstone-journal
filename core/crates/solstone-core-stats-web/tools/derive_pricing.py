# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc
"""Emit the complete bounded genai-prices 0.0.55 Rust snapshot."""

from genai_prices import Usage, calc_price

# The pinned snapshot lacks the two current default aliases; preserve Python's
# family-fallback price explicitly rather than relying on a Rust fallback.
MODELS = [
    ("gpt-5.2", "gpt-5.2"),
    ("gpt-5-mini", "gpt-5-mini"),
    ("gpt-5.4-mini", "gpt-5-mini"),
    ("gpt-5-nano", "gpt-5-nano"),
    ("gpt-5.2-pro", "gpt-5.2-pro"),
    ("claude-opus-4-6", "claude-opus-4-6"),
    ("claude-sonnet-4-6", "claude-sonnet-4-6"),
    ("claude-haiku-4-5", "claude-haiku-4-5"),
    ("gemini-3-pro-preview", "gemini-3-pro-preview"),
    ("gemini-3-flash-preview", "gemini-3-flash-preview"),
    ("gemini-3.5-flash", "gemini-3-flash-preview"),
    ("gemini-2.5-flash-lite", "gemini-2.5-flash-lite"),
]

HEADER = """// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

// Bounded `genai-prices` compatibility snapshot.
//
// Snapshot of genai-prices 0.0.55. Refreshed intentionally by re-running
// `.venv/bin/python tools/derive_pricing.py` from this crate; never executed
// at build or runtime. This bounded snapshot is deliberate: talent-cli
// logs.rs declines to ship genai-prices' generated ~494KB generic table.

pub(super) static RATES: &[(&str, Rate)] = &[
"""


def price(source_model, usage):
    value = calc_price(usage, source_model).total_price * 1_000_000
    text = format(value, "f").rstrip("0").rstrip(".") or "0"
    return f"{text}.0" if "." not in text else text


print(HEADER, end="")
for name, source in MODELS:
    uncached = price(source, Usage(input_tokens=1, output_tokens=0))
    cached = price(
        source,
        Usage(input_tokens=1, cache_read_tokens=1, output_tokens=0),
    )
    output = price(source, Usage(output_tokens=1))
    print(f'    ("{name}", Rate::new({uncached}, {cached}, {output})),')
print("];" )
