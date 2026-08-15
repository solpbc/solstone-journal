// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

// Bounded `genai-prices` compatibility snapshot.
//
// Snapshot of genai-prices 0.0.55. Refreshed intentionally by re-running
// `python3 derive_pricing.py` beside this file; never executed at build or
// runtime. This bounded snapshot is deliberate: talent-cli logs.rs declines
// to ship genai-prices' generated ~494KB generic table.

pub(super) static RATES: &[(&str, Rate)] = &[
    ("gpt-5.2", Rate::new(1.75, 0.175, 14.0)),
    ("gpt-5-mini", Rate::new(0.25, 0.025, 2.0)),
    ("gpt-5-nano", Rate::new(0.05, 0.005, 0.4)),
    ("gpt-5.2-pro", Rate::new(21.0, 0.0, 168.0)),
    ("claude-opus-4-6", Rate::new(5.0, 0.5, 25.0)),
    ("claude-sonnet-4-6", Rate::new(3.0, 0.3, 15.0)),
    ("claude-haiku-4-5", Rate::new(1.0, 0.1, 5.0)),
    ("gemini-3-pro-preview", Rate::new(2.0, 0.2, 12.0)),
    ("gemini-3-flash-preview", Rate::new(0.5, 0.05, 3.0)),
    ("gemini-2.5-flash-lite", Rate::new(0.1, 0.01, 0.4)),
];
