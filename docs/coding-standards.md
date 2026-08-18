# Coding Standards

## Language & Tools

- **Rust** in `core/`. `cargo fmt` / `make check-rust-fmt`. Clippy is part of
  `make ci`.
- **JavaScript** in convey-shell assets. `//` comments.

## Naming

Rust: modules, functions, and variables `snake_case`; types `PascalCase`;
constants `SCREAMING_SNAKE_CASE`. JavaScript follows the file it lives in.

## File Headers

Every `.rs` file starts with:

```
// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
```

JavaScript uses `//` for the same two lines.

## Development Principles

- **DRY, KISS, YAGNI.** Prefer the simple path.
- **Single responsibility.** One crate or function owns one write.
- **Self-contained.** No backwards-compatibility shims, fallback aliases, or
  deprecated parameter handling. Update every caller. Journal format changes
  get a `journal maintenance` migration, not a compatibility layer.
- **Trust journal resolution.** Never set `SOLSTONE_JOURNAL` from application
  code, agent prompts, subprocess environments, or service files. See
  [environment.md](environment.md).
- **Small focused commits.** Run git from the repo root, not `git -C`.

## Dependencies

Workspace crates inherit from `core/Cargo.toml`. Do not add a Python
dependency or a `make install` step. `make install` is retired.

## Layer Hygiene

The L1–L9 invariants (read/write separation, domain write ownership, naming
contracts, CLI verb polarity) live in `AGENTS.md` §7. Read that table's
`core/crates/` rows. Its Python module names are gone.
