# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Canonical cogitate runtime contract — the in-context preamble and the locked
access-tier / finalization vocabularies every cogitate talent is written against.

See docs/COGITATE.md for the full contract. COGITATE_RUNTIME_PREAMBLE is prepended
to every cogitate run's system prompt in solstone.think.providers.cli.assemble_prompt.
Downstream prompt/lint/inventory work references these constants by path rather than
re-typing the contract.
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import Any

COGITATE_JOURNAL_COMMANDS = ("identity", "health", "talent")


def cogitate_journal_command_list() -> str:
    """Return the approved direct journal command families for prompt copy."""
    return ", ".join(COGITATE_JOURNAL_COMMANDS)


COGITATE_RUNTIME_PREAMBLE = f"""\
You are a solstone cogitate talent running inside the live system. This runtime contract is authoritative; do not assume capabilities beyond it.

- Reach the journal through the `sol` command line: emit `sol` / `sol call ...` command lines, e.g. sol(command="sol call activities list"). The runtime runs each call as a single parsed command-line invocation, not an arbitrary shell. The `sol` CLI is the one authoritative path between you and the journal; never assume direct database, socket, or HTTP access.
- The approved host command families are {cogitate_journal_command_list()}; run them directly as `journal <family> ...` through the same tool, never prefixed with `sol` or `sol call`.
- Write journal state only through approved journal commands (`sol call ...` verbs for the data you own, plus approved direct host commands when a prompt names one). There is no general-purpose write tool; persistence that does not go through an approved journal command will not happen.
- Raw evidence reads use the provided read tools (`read_file`, `list_directory`, `glob`, `grep_search`) for bounded journal evidence: a denylist (`.git`, caches, credentials, virtualenvs, `node_modules`) and per-call / per-run caps apply. Recursive scans must not start at the journal root, `chronicle/`, or `facets/`: `glob` and directory `grep_search` must start below them, as must recursive `list_directory`. Prefer `sol call` reads; use raw reads only for evidence that has no `sol` command.
- Finalize as your run is configured: call `emit_final` when an `emit_final` tool is present; otherwise finish through the built-in finish tool; a side-effect-only talent that has already persisted its work finishes quietly with no output.
- Do not assume tools or context you were not given: no other bare `journal ...` family, no raw `cat` / `ls` / shell file reads, no shell composition (pipes, redirects, chaining, or command substitution; one command per call), no auto-loaded skills or AGENTS.md / CLAUDE.md, no browser or web access, no MCP tools, and no delegating to sub-agents. Any guidance file is a normal journal file with no special status; this contract is your source of truth.
"""

COGITATE_DIAGNOSTIC_PREAMBLE = """\
You are a bounded solstone diagnostic cogitate check. This runtime contract is authoritative; do not assume capabilities beyond it.

- The only available tool is `emit_final`. Call it exactly once with a concise, non-empty diagnostic result.
- No journal access is available: no `sol` command line, no read tools, no submit tools, no shell commands, no browser or web access, and no delegating to sub-agents.
- Do not claim that you read or changed journal state. This check returns its final content in memory only.
"""

# Locked cogitate access-tier vocabulary (the C1 contract). Downstream milestones
# key per-talent assignment, enforcement, redesign, and lint off these names.
COGITATE_ACCESS_TIERS = (
    "normal",
    "system-read",
    "outbound",
    "synthesis",
    "diagnostic",
)
TALENT_ACCESS_TIERS = tuple(
    tier for tier in COGITATE_ACCESS_TIERS if tier != "diagnostic"
)

# `code-agent` is a documented FUTURE tier — NOT part of the current cogitate
# runtime (it needs write access, broad tools, and a repo cwd, deliberately out of
# scope). There is intentionally NO `repair` tier in cogitate: health
# fact-gathering and repair are a deterministic supervisor/system workflow, not an
# LLM talent.
FUTURE_ACCESS_TIERS = ("code-agent",)

# The finalization mode a talent uses to signal completion.
TALENT_FINALIZATION_MODES = ("emit_final", "FinishTool", "quiet")

# Structured source for the read-tool names enumerated in the runtime preamble.
COGITATE_READ_TOOL_NAMES = ("read_file", "list_directory", "glob", "grep_search")


@dataclass(frozen=True)
class AccessCapabilities:
    sol: bool
    reads: bool
    submit: bool


_ACCESS_TIER_CAPABILITIES: dict[str, AccessCapabilities] = {
    "normal": AccessCapabilities(sol=True, reads=True, submit=False),
    "system-read": AccessCapabilities(sol=True, reads=True, submit=False),
    "outbound": AccessCapabilities(sol=True, reads=False, submit=True),
    # Pure command-surface synthesis: the journal is reached only through
    # approved journal commands, with no raw-filesystem read tier and no
    # outbound submit.
    # For synthesis talents (weekly_reflection, partner) whose source of record
    # is a documented command form, not the raw journal tree.
    "synthesis": AccessCapabilities(sol=True, reads=False, submit=False),
    "diagnostic": AccessCapabilities(sol=False, reads=False, submit=False),
}

_missing_access_tiers = set(COGITATE_ACCESS_TIERS) - set(_ACCESS_TIER_CAPABILITIES)
if _missing_access_tiers:
    missing = ", ".join(sorted(_missing_access_tiers))
    raise RuntimeError(f"missing access tier capability mapping for: {missing}")

_stray_access_tiers = set(_ACCESS_TIER_CAPABILITIES) - set(COGITATE_ACCESS_TIERS)
if _stray_access_tiers:
    stray = ", ".join(sorted(_stray_access_tiers))
    raise RuntimeError(f"stray access tier capability mapping for: {stray}")


def capabilities_for_access_tier(tier: str) -> AccessCapabilities:
    try:
        return _ACCESS_TIER_CAPABILITIES[tier]
    except KeyError as exc:
        raise ValueError(f"unknown access_tier: {tier}") from exc


def expects_emit_final(config: dict[str, Any]) -> bool:
    """Select emit_final vs the built-in finish tool for providers and inventory."""
    return (
        config.get("diagnostic") is True
        or bool(config.get("output_path"))
        or config.get("schedule") in {"daily", "weekly", "activity"}
    )


__all__ = [
    "AccessCapabilities",
    "COGITATE_DIAGNOSTIC_PREAMBLE",
    "COGITATE_JOURNAL_COMMANDS",
    "COGITATE_RUNTIME_PREAMBLE",
    "COGITATE_ACCESS_TIERS",
    "COGITATE_READ_TOOL_NAMES",
    "FUTURE_ACCESS_TIERS",
    "TALENT_ACCESS_TIERS",
    "TALENT_FINALIZATION_MODES",
    "capabilities_for_access_tier",
    "cogitate_journal_command_list",
    "expects_emit_final",
]
