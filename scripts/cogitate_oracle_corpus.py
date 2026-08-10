#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Freeze the deterministic half of the cogitate runtime contract by EXECUTING
the Python reference over a corpus.

The Rust rebuild of ``cogitate`` needs to agree with the reference on decisions
no unit test states in one place: which command lines the policy gate admits,
what it says when it refuses, which paths the raw-read tier will resolve, and
what it returns when it will not.  Those decisions live in code, not in prose,
and the reference tree stops being runnable once Python is cut.

So this script does not *describe* the reference — it *runs* it, once per case,
and records what came back.  Every vector carries a ``citation`` naming the
reference ``file:line`` that determines its verdict.

    python3 scripts/cogitate_oracle_corpus.py --out core/fixtures/cogitate_oracle.json

CLOCK: this generator can only produce output while the Python reference is
still importable.  The fixture it writes is a **frozen record**, not a
regenerable artifact, and its header says so.  Tests read the fixture; they
never execute Python.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import shutil
import stat
import subprocess
import sys
import tempfile
from dataclasses import asdict, is_dataclass
from pathlib import Path
from typing import Any

REPO_ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(REPO_ROOT))

from solstone.think import cogitate_read_tools as crt  # noqa: E402
from solstone.think.cogitate_contract import (  # noqa: E402
    COGITATE_ACCESS_TIERS,
    COGITATE_DIAGNOSTIC_PREAMBLE,
    COGITATE_JOURNAL_COMMANDS,
    COGITATE_READ_TOOL_NAMES,
    COGITATE_RUNTIME_PREAMBLE,
    FUTURE_ACCESS_TIERS,
    TALENT_ACCESS_TIERS,
    TALENT_FINALIZATION_MODES,
    capabilities_for_access_tier,
    expects_emit_final,
)
from solstone.think.cogitate_policy import (  # noqa: E402
    DETERMINISTIC_FAILURE_CAPS,
    DETERMINISTIC_FAILURE_REASON_CODES,
    CogitatePolicy,
    failure_capped,
    resolve_read_scope,
)

SDK_INJECTED_ACTION_PROPERTIES = frozenset({"kind"})
DEFAULT_READ_CALL_BUDGET_VALUE = 200

FIXTURE_NAME = "solstone-cogitate-oracle"
FIXTURE_VERSION = 6

# Spelled out so the corpus below never depends on how a shell or an editor
# treats a backslash. Several vectors exist specifically to pin backslash
# handling, so an escaping accident in this file would be invisible.
BS = chr(92)

# ---------------------------------------------------------------------------
# A. the command policy gate
# ---------------------------------------------------------------------------

# (case_id, command, access_tier, outbound_approval, citation, why)
POLICY_CASES: list[tuple[str, str, str, str | None, str, str]] = [
    # --- the happy paths
    (
        "sol_bare",
        "sol",
        "normal",
        None,
        "cogitate_policy.py:233-241",
        "a bare `sol` is admitted",
    ),
    (
        "sol_call",
        "sol call activities list",
        "normal",
        None,
        "cogitate_policy.py:233-241",
        "the documented `sol call` form",
    ),
    (
        "sol_call_quoted_arg",
        "sol call entities search --name 'Ada Lovelace'",
        "normal",
        None,
        "cogitate_policy.py:212",
        "shlex splits quoted data, quotes are not operators",
    ),
    (
        "sol_call_double_quoted",
        'sol call entities search --name "Ada Lovelace"',
        "normal",
        None,
        "cogitate_policy.py:150-152",
        "double quotes are data",
    ),
    # --- approved direct host families (the closed set)
    (
        "journal_identity",
        "journal identity partner",
        "normal",
        None,
        "cogitate_policy.py:236-240",
        "approved host family",
    ),
    (
        "journal_health",
        "journal health",
        "normal",
        None,
        "cogitate_policy.py:236-240",
        "approved host family",
    ),
    (
        "journal_talent",
        "journal talent inventory",
        "normal",
        None,
        "cogitate_policy.py:236-240",
        "approved host family",
    ),
    (
        "journal_bare",
        "journal",
        "normal",
        None,
        "cogitate_policy.py:236-240",
        "bare `journal` has no family -> restricted",
    ),
    (
        "journal_unapproved_family",
        "journal doctor",
        "normal",
        None,
        "cogitate_policy.py:233-241",
        "an unapproved family is restricted, not repaired",
    ),
    # --- the two repair denials (they carry a *suggested* command, so the text is contract)
    (
        "hybrid_identity",
        "sol call journal identity partner",
        "normal",
        None,
        "cogitate_policy.py:219-224",
        "hybrid form is refused WITH a repair",
    ),
    (
        "hybrid_health",
        "sol call journal health --json",
        "normal",
        None,
        "cogitate_policy.py:219-224",
        "hybrid form, flags survive into the repair",
    ),
    (
        "hybrid_talent",
        "sol call journal talent show read",
        "normal",
        None,
        "cogitate_policy.py:219-224",
        "hybrid form",
    ),
    (
        "hybrid_not_a_family",
        "sol call journal search foo",
        "normal",
        None,
        "cogitate_policy.py:219-224",
        "`search` is NOT in COGITATE_JOURNAL_COMMANDS -> falls through to allowed",
    ),
    (
        "bare_journal_search",
        "journal search sunlight",
        "normal",
        None,
        "cogitate_policy.py:226-231",
        "bare `journal search` -> repair to `sol call journal search`",
    ),
    (
        "bare_journal_facet",
        "journal facet list",
        "normal",
        None,
        "cogitate_policy.py:226-231",
        "bare `journal facet` -> repair",
    ),
    (
        "bare_journal_facet_quoting",
        "journal facet set 'my facet'",
        "normal",
        None,
        "cogitate_policy.py:167-170",
        "shlex.join re-quotes the repair suggestion",
    ),
    # --- restricted commands
    (
        "restricted_cat",
        "cat /etc/passwd",
        "normal",
        None,
        "cogitate_policy.py:233-241",
        "not sol, not journal",
    ),
    (
        "restricted_ls",
        "ls",
        "normal",
        None,
        "cogitate_policy.py:233-241",
        "not sol, not journal",
    ),
    (
        "restricted_python",
        "python -c 'print(1)'",
        "normal",
        None,
        "cogitate_policy.py:233-241",
        "not sol, not journal",
    ),
    (
        "restricted_solstone_core",
        "solstone-core generate",
        "normal",
        None,
        "cogitate_policy.py:233-241",
        "the native binary is NOT reachable from a talent",
    ),
    # --- shell composition (each operator, and each quoting posture)
    (
        "shell_pipe",
        "sol call activities list | head",
        "normal",
        None,
        "cogitate_policy.py:129-158",
        "pipe outside quotes",
    ),
    (
        "shell_semicolon",
        "sol call activities list; sol call entities list",
        "normal",
        None,
        "cogitate_policy.py:129-158",
        "chaining",
    ),
    (
        "shell_and",
        "sol a && sol b",
        "normal",
        None,
        "cogitate_policy.py:129-158",
        "chaining",
    ),
    (
        "shell_redirect_out",
        "sol call activities list > /tmp/x",
        "normal",
        None,
        "cogitate_policy.py:129-158",
        "redirect",
    ),
    (
        "shell_redirect_in",
        "sol call activities list < /tmp/x",
        "normal",
        None,
        "cogitate_policy.py:129-158",
        "redirect",
    ),
    (
        "shell_subshell",
        "(sol call activities list)",
        "normal",
        None,
        "cogitate_policy.py:129-158",
        "subshell",
    ),
    (
        "shell_dollar_paren",
        "sol call entities show $(whoami)",
        "normal",
        None,
        "cogitate_policy.py:130-131",
        "command substitution, checked before the scanner",
    ),
    (
        "shell_backtick",
        "sol call entities show `whoami`",
        "normal",
        None,
        "cogitate_policy.py:130-131",
        "backtick substitution",
    ),
    (
        "shell_newline",
        "sol call activities list\nsol call entities list",
        "normal",
        None,
        "cogitate_policy.py:132-133",
        "newline is a composition character",
    ),
    (
        "shell_carriage_return",
        "sol call activities list\rsol call entities list",
        "normal",
        None,
        "cogitate_policy.py:132-133",
        "a lone CR is refused too",
    ),
    (
        "shell_operator_in_single_quotes",
        "sol call entities search --name 'a|b'",
        "normal",
        None,
        "cogitate_policy.py:140-142",
        "an operator inside single quotes is DATA",
    ),
    (
        "shell_operator_in_double_quotes",
        'sol call entities search --name "a|b"',
        "normal",
        None,
        "cogitate_policy.py:143-149",
        "an operator inside double quotes is DATA",
    ),
    (
        "shell_dollar_paren_in_single_quotes",
        "sol call entities search --name '$(x)'",
        "normal",
        None,
        "cogitate_policy.py:130-131",
        "the $( check runs BEFORE quote tracking - refused even when quoted",
    ),
    (
        "shell_backslash_escaped_pipe",
        "sol call entities search --name a\\|b",
        "normal",
        None,
        "cogitate_policy.py:151-153",
        "a backslash escape skips two characters",
    ),
    (
        "shell_unterminated_single_quote",
        "sol call entities search --name 'oops",
        "normal",
        None,
        "cogitate_policy.py:158",
        "an unterminated quote is a syntax violation",
    ),
    (
        "shell_unterminated_double_quote",
        'sol call entities search --name "oops',
        "normal",
        None,
        "cogitate_policy.py:158",
        "an unterminated quote is a syntax violation",
    ),
    (
        "empty_command",
        "",
        "normal",
        None,
        "cogitate_policy.py:216-217",
        "empty is its own refusal, distinct from restricted",
    ),
    (
        "whitespace_only_command",
        "   ",
        "normal",
        None,
        "cogitate_policy.py:216-217",
        "whitespace-only splits to an empty argv",
    ),
    # --- the shlex.split ValueError path, which the syntax scanner does NOT catch
    (
        "shlex_split_raises_trailing_backslash",
        "sol x" + BS,
        "normal",
        None,
        "cogitate_policy.py:211-214",
        "a trailing backslash passes _shell_syntax_violation (the index+1 guard) and then "
        "makes shlex.split RAISE - a distinct code path reaching the same refusal. A "
        "tolerant tokenizer ADMITS this command.",
    ),
    (
        "backslash_literal_inside_single_quotes",
        "sol call entities search --name 'a" + BS + "'",
        "normal",
        None,
        "cogitate_policy.py:140-142",
        "inside single quotes a backslash is LITERAL - no escape consumption - so this "
        "parses and the argument keeps the backslash. A port that escapes uniformly sees "
        "an unterminated quote and REFUSES.",
    ),
    (
        "escaped_dquote_inside_dquotes",
        'sol call entities search --name "a' + BS + '"b"',
        "normal",
        None,
        "cogitate_policy.py:143-149",
        "inside double quotes a backslash DOES escape",
    ),
    # --- the repair command is generated by shlex.join/shlex.quote, and the model reads
    #     it as an instruction. Pin the quoting rule in both directions.
    (
        "repair_quote_apostrophe",
        'journal facet set "it\'s mine"',
        "normal",
        None,
        "cogitate_policy.py:167-170",
        "shlex.quote's apostrophe form is not the obvious one",
    ),
    (
        "repair_quote_empty_string",
        "journal facet set ''",
        "normal",
        None,
        "cogitate_policy.py:167-170",
        "an empty argument must survive as ''",
    ),
    (
        "repair_quote_safe_chars",
        "journal facet set a/b.c@d=e:f,g+h-i%j",
        "normal",
        None,
        "cogitate_policy.py:167-170",
        "shlex.quote leaves [-\\w@%+=:,./] UNQUOTED",
    ),
    (
        "repair_quote_unsafe_star",
        "journal facet set 'a*b'",
        "normal",
        None,
        "cogitate_policy.py:167-170",
        "a glob char forces quoting",
    ),
    (
        "repair_quote_unsafe_tilde",
        "journal facet set 'a~b'",
        "normal",
        None,
        "cogitate_policy.py:167-170",
        "a tilde forces quoting",
    ),
    (
        "repair_quote_unsafe_brace",
        "journal facet set 'a{b'",
        "normal",
        None,
        "cogitate_policy.py:167-170",
        "a brace forces quoting",
    ),
    (
        "repair_quote_search_family",
        "journal search 'two words'",
        "normal",
        None,
        "cogitate_policy.py:167-170",
        "the search family's repair prefix differs from facet's",
    ),
    # --- outbound submit gating: 7 verbs x tier x approval
    *[
        (
            f"support_{verb}_normal_noapproval",
            f"sol call support {verb} --body x",
            "normal",
            None,
            "cogitate_policy.py:243-251",
            "submit verb denied for a non-submit tier",
        )
        for verb in (
            "create",
            "reply",
            "attach",
            "feedback",
            "close",
            "resolved",
            "still-need-help",
        )
    ],
    (
        "support_create_outbound_noapproval",
        "sol call support create --body x",
        "outbound",
        None,
        "cogitate_policy.py:253-259",
        "submit tier without a per-send approval",
    ),
    (
        "support_create_outbound_approved",
        "sol call support create --body x",
        "outbound",
        "owner-token",
        "cogitate_policy.py:243-261",
        "submit tier WITH approval",
    ),
    (
        "support_create_synthesis",
        "sol call support create --body x",
        "synthesis",
        "owner-token",
        "cogitate_policy.py:243-251",
        "synthesis has sol but not submit; approval does not help",
    ),
    (
        "support_create_system_read",
        "sol call support create --body x",
        "system-read",
        "owner-token",
        "cogitate_policy.py:243-251",
        "system-read has sol but not submit",
    ),
    (
        "support_read_verb_normal",
        "sol call support list",
        "normal",
        None,
        "cogitate_policy.py:264-271",
        "a non-send support verb is not gated",
    ),
    # the scan-anywhere subtlety: _support_send_verb searches EVERY index, not just 0..2
    (
        "support_send_verb_not_at_head",
        "sol call entities note sol call support create",
        "normal",
        None,
        "cogitate_policy.py:265-266",
        "the send-verb scan walks every offset, not just the command head",
    ),
    (
        "support_send_verb_trailing_short",
        "sol call support",
        "normal",
        None,
        "cogitate_policy.py:265-266",
        "range(len(argv)-3) means a 3-token argv is never scanned",
    ),
]


def build_policy_vectors() -> list[dict[str, Any]]:
    rows: list[dict[str, Any]] = []
    for case_id, command, tier, approval, citation, why in POLICY_CASES:
        policy = CogitatePolicy(
            allowed_roots=[Path("/journal")],
            access_tier=tier,
            outbound_approval=approval,
        )
        decision = policy.classify_command(command)
        rows.append(
            {
                "id": case_id,
                "command": command,
                "access_tier": tier,
                "outbound_approval": approval,
                "citation": citation,
                "why": why,
                "expect": {
                    "allowed": decision.allowed,
                    "reason": decision.reason,
                    "argv": decision.argv,
                },
            }
        )
    return rows


# `check()` is the tool-level gate that sits above classify_command.
CHECK_CASES: list[tuple[str, str, dict[str, Any], str, str]] = [
    (
        "check_write_file",
        "write_file",
        {"path": "x", "content": "y"},
        "normal",
        "cogitate_policy.py:195-196",
    ),
    ("check_replace", "replace", {"path": "x"}, "normal", "cogitate_policy.py:195-196"),
    (
        "check_read_file",
        "read_file",
        {"path": "x"},
        "normal",
        "cogitate_policy.py:202-203",
    ),
    ("check_glob", "glob", {"pattern": "*"}, "normal", "cogitate_policy.py:202-203"),
    (
        "check_emit_final",
        "emit_final",
        {"content": "x"},
        "normal",
        "cogitate_policy.py:205",
    ),
    ("check_finish", "finish", {}, "normal", "cogitate_policy.py:205"),
    (
        "check_unknown_tool",
        "browser",
        {},
        "normal",
        "cogitate_policy.py:205 - an UNKNOWN tool defaults to ALLOWED",
    ),
    (
        "check_run_shell_allowed",
        "run_shell_command",
        {"command": "sol call activities list"},
        "normal",
        "cogitate_policy.py:198-200",
    ),
    (
        "check_run_shell_denied",
        "run_shell_command",
        {"command": "rm -rf /"},
        "normal",
        "cogitate_policy.py:198-200",
    ),
    (
        "check_run_shell_missing_arg",
        "run_shell_command",
        {},
        "normal",
        "cogitate_policy.py:199 - a missing command arg becomes the empty string",
    ),
    (
        "check_write_file_outbound",
        "write_file",
        {},
        "outbound",
        "cogitate_policy.py:195-196 - write denial is tier-independent",
    ),
]


def build_check_vectors() -> list[dict[str, Any]]:
    rows: list[dict[str, Any]] = []
    for case_id, tool, args, tier, citation in CHECK_CASES:
        policy = CogitatePolicy(
            allowed_roots=[Path("/journal")], access_tier=tier, outbound_approval=None
        )
        allowed, reason = policy.check(tool, args)
        rows.append(
            {
                "id": case_id,
                "tool": tool,
                "args": args,
                "access_tier": tier,
                "citation": citation,
                "expect": {"allowed": allowed, "reason": reason},
            }
        )
    return rows


# ---------------------------------------------------------------------------
# B. read scope resolution
# ---------------------------------------------------------------------------

READ_SCOPE_CASES: list[tuple[str, dict[str, Any], str, int, str]] = [
    ("scope_default_no_span", {}, "20260809", 0, "cogitate_policy.py:308-310"),
    ("scope_span_2", {}, "20260809", 2, "cogitate_policy.py:312-316"),
    (
        "scope_config_span_3",
        {"read_scope_span": 3},
        "20260809",
        0,
        "cogitate_policy.py:308-316",
    ),
    (
        "scope_config_span_beats_arg",
        {"read_scope_span": 1},
        "20260809",
        5,
        "cogitate_policy.py:308 - the config value wins over the argument",
    ),
    (
        "scope_literal",
        {"read_scope": ["facets/work"]},
        "20260809",
        0,
        "cogitate_policy.py:302-307",
    ),
    (
        "scope_day_placeholder",
        {"read_scope": ["chronicle/<day>"]},
        "20260809",
        0,
        "cogitate_policy.py:286-293",
    ),
    (
        "scope_day_minus_1",
        {"read_scope": ["chronicle/<day-1>"]},
        "20260809",
        0,
        "cogitate_policy.py:286-293",
    ),
    (
        "scope_day_minus_7_month_boundary",
        {"read_scope": ["chronicle/<day-7>"]},
        "20260803",
        0,
        "cogitate_policy.py:286-293 - crosses a month boundary",
    ),
    (
        "scope_day_minus_1_year_boundary",
        {"read_scope": ["chronicle/<day-1>"]},
        "20260101",
        0,
        "cogitate_policy.py:286-293 - crosses a year boundary",
    ),
    (
        "scope_leap_day",
        {"read_scope": ["chronicle/<day-1>"]},
        "20260301",
        0,
        "cogitate_policy.py:286-293 - 2026 is not a leap year",
    ),
    (
        "scope_multiple_placeholders",
        {"read_scope": ["chronicle/<day>", "chronicle/<day-2>"]},
        "20260809",
        0,
        "cogitate_policy.py:302-307",
    ),
    (
        "scope_repeated_placeholder_one_string",
        {"read_scope": ["<day>/a/<day-1>"]},
        "20260809",
        0,
        "cogitate_policy.py:293 - re.sub replaces every occurrence",
    ),
    (
        "scope_span_zero_explicit",
        {"read_scope_span": 0},
        "20260809",
        0,
        "cogitate_policy.py:309-310",
    ),
    (
        "scope_span_negative",
        {"read_scope_span": -3},
        "20260809",
        0,
        "cogitate_policy.py:309-310 - a negative span collapses to the single day",
    ),
]


def build_read_scope_vectors() -> list[dict[str, Any]]:
    rows = []
    for case_id, config, day, span, citation in READ_SCOPE_CASES:
        rows.append(
            {
                "id": case_id,
                "talent_config": config,
                "day": day,
                "span": span,
                "citation": citation,
                "expect": resolve_read_scope(config, day, span=span),
            }
        )
    return rows


# ---------------------------------------------------------------------------
# C. finalization mode selection
# ---------------------------------------------------------------------------

EMIT_FINAL_CASES: list[tuple[str, dict[str, Any], str]] = [
    ("final_empty", {}, "cogitate_contract.py:110-116"),
    ("final_diagnostic", {"diagnostic": True}, "cogitate_contract.py:112"),
    (
        "final_diagnostic_truthy_not_true",
        {"diagnostic": 1},
        "cogitate_contract.py:112 - `is True`, so 1 does NOT match",
    ),
    (
        "final_output_path",
        {"output_path": "/j/talents/x.md"},
        "cogitate_contract.py:113",
    ),
    ("final_output_path_empty", {"output_path": ""}, "cogitate_contract.py:113"),
    ("final_schedule_daily", {"schedule": "daily"}, "cogitate_contract.py:114"),
    ("final_schedule_weekly", {"schedule": "weekly"}, "cogitate_contract.py:114"),
    ("final_schedule_activity", {"schedule": "activity"}, "cogitate_contract.py:114"),
    (
        "final_schedule_segment",
        {"schedule": "segment"},
        "cogitate_contract.py:114 - segment is NOT in the set",
    ),
    (
        "final_schedule_cadence",
        {"schedule": "cadence"},
        "cogitate_contract.py:114 - cadence is NOT in the set",
    ),
    # output_path goes through bool(), not is-present. A Rust `.is_some()` port diverges
    # on every falsy-but-present value. Same class as the `is True` case above.
    (
        "final_output_path_zero",
        {"output_path": 0},
        "cogitate_contract.py:113 - bool(0) is False",
    ),
    (
        "final_output_path_empty_list",
        {"output_path": []},
        "cogitate_contract.py:113 - bool([]) is False",
    ),
    (
        "final_output_path_empty_dict",
        {"output_path": {}},
        "cogitate_contract.py:113 - bool({}) is False",
    ),
    (
        "final_output_path_zero_string",
        {"output_path": "0"},
        "cogitate_contract.py:113 - a non-empty string is True",
    ),
    ("final_schedule_none", {"schedule": None}, "cogitate_contract.py:114"),
    (
        "final_all_three",
        {"diagnostic": True, "output_path": "/x", "schedule": "daily"},
        "cogitate_contract.py:110-116",
    ),
]


def build_emit_final_vectors() -> list[dict[str, Any]]:
    return [
        {"id": cid, "config": cfg, "citation": cit, "expect": expects_emit_final(cfg)}
        for cid, cfg, cit in EMIT_FINAL_CASES
    ]


# ---------------------------------------------------------------------------
# D. deterministic-failure caps
# ---------------------------------------------------------------------------


def build_failure_cap_vectors() -> list[dict[str, Any]]:
    rows = []
    codes = sorted(DETERMINISTIC_FAILURE_REASON_CODES) + ["not_a_reason_code", ""]
    for code in codes:
        for count in (0, 1, 2, 3, 4):
            rows.append(
                {
                    "id": f"cap_{code or 'EMPTY'}_{count}",
                    "reason_code": code,
                    "count": count,
                    "citation": "cogitate_policy.py:78-81",
                    "expect": failure_capped(code, count),
                }
            )
    rows.append(
        {
            "id": "cap_none_2",
            "reason_code": None,
            "count": 2,
            "citation": "cogitate_policy.py:80 - None coerces to the empty key",
            "expect": failure_capped(None, 2),
        }
    )
    return rows


# ---------------------------------------------------------------------------
# E. access tiers
# ---------------------------------------------------------------------------


def build_capability_vectors() -> dict[str, Any]:
    caps = {}
    for tier in COGITATE_ACCESS_TIERS:
        c = capabilities_for_access_tier(tier)
        caps[tier] = {"sol": c.sol, "reads": c.reads, "submit": c.submit}
    # Two unknown names, so a hardcoded error string cannot satisfy the vector.
    unknown: dict[str, dict[str, Any]] = {}
    for name in ("code-agent", "repair", ""):
        try:
            capabilities_for_access_tier(name)
            unknown[name] = {"raises": False}
        except ValueError as exc:
            unknown[name] = {"raises": True, "error": str(exc)}
    # The invariant nothing in the reference asserts: no tier holds reads AND submit,
    # so the only tier that can send off the machine has no raw-read tier.
    both = sorted(t for t, c in caps.items() if c["reads"] and c["submit"])
    return {
        "tiers": list(COGITATE_ACCESS_TIERS),
        "talent_tiers": list(TALENT_ACCESS_TIERS),
        "future_tiers": list(FUTURE_ACCESS_TIERS),
        "capabilities": caps,
        "unknown_tier": unknown,
        "tiers_with_reads_and_submit": both,
        "submit_tiers": sorted(t for t, c in caps.items() if c["submit"]),
        "citation": "cogitate_contract.py:79-107",
    }


# ---------------------------------------------------------------------------
# F. the raw-read tier, executed against a deterministic bed
# ---------------------------------------------------------------------------


def build_bed(root: Path) -> None:
    """A deterministic journal bed. Content is fixed so byte counts are stable."""
    (root / "chronicle" / "20260809" / "default" / "120000_60").mkdir(parents=True)
    (
        root / "chronicle" / "20260809" / "default" / "120000_60" / "audio.jsonl"
    ).write_text(
        '{"start":0,"text":"hello"}\n{"start":1,"text":"world"}\n', encoding="utf-8"
    )
    (root / "chronicle" / "20260809" / "default" / "120000_60" / "notes.md").write_text(
        "line one\nline two\nline three\n", encoding="utf-8"
    )
    (root / "facets").mkdir()
    (root / "facets" / "work.md").write_text(
        "work facet\nsunlight here\n", encoding="utf-8"
    )
    (root / "talents").mkdir()
    (root / "talents" / "partner").mkdir()
    (root / "talents" / "partner" / "abc.jsonl").write_text(
        '{"event":"finish"}\n', encoding="utf-8"
    )

    # denied components
    (root / ".git").mkdir()
    (root / ".git" / "config").write_text("[core]\n", encoding="utf-8")
    (root / ".cache").mkdir()
    (root / ".cache" / "x.txt").write_text("cached\n", encoding="utf-8")
    (root / "node_modules").mkdir()
    (root / "node_modules" / "pkg.json").write_text("{}\n", encoding="utf-8")
    (root / ".venv").mkdir()
    (root / ".venv" / "pyvenv.cfg").write_text("home=/usr\n", encoding="utf-8")

    # credential-shaped names
    (root / "secrets").mkdir()
    for name in (
        "id_rsa",
        "id_rsa.pub",
        ".env",
        ".env.local",
        "server.key",
        "key.pem",
        "my.credentials",
        "credentials",
        "credentials.json",
        "token.key",
        "api_secret.txt",
        "token.txt",
        "passwords.md",
        "secrets.yaml",
    ):
        (root / "secrets" / name).write_text("SECRET\n", encoding="utf-8")

    # a binary file and a special file
    (root / "blob.bin").write_bytes(bytes(range(256)) * 4)
    (root / "empty.txt").write_text("", encoding="utf-8")
    # FAIL LOUDLY, do not skip. A silently-skipped FIFO makes the fixture encode
    # "a host where mkfifo happened to work", and vectors depend on it existing.
    os.mkfifo(root / "fifo")

    # symlink escape + in-tree symlink. Same rule: loud, never skipped.
    outside = root.parent / "outside"
    outside.mkdir(exist_ok=True)
    (outside / "leak.txt").write_text("LEAKED\n", encoding="utf-8")
    (root / "escape").symlink_to(outside / "leak.txt")
    (root / "inside_link").symlink_to(root / "facets" / "work.md")

    # --- v3: a non-broad subtree, so glob and grep can be exercised for real.
    # Everything else sits at the journal root, chronicle/ or facets/, which the
    # broad-root rule refuses -- which left most of glob's and grep_search's
    # parameter surface pinned by nothing at all.
    probe = root / "probe"
    probe.mkdir()
    (probe / "alpha.md").write_text(
        "Sunlight on the water\nsunlight again\nplain line\n", encoding="utf-8"
    )
    (probe / "beta.txt").write_text(
        "before line\nSUNLIGHT shouting\nafter line\n", encoding="utf-8"
    )
    (probe / "gamma.log").write_text("no match here\n", encoding="utf-8")
    (probe / ".hidden_probe.md").write_text("sunlight hidden\n", encoding="utf-8")
    (probe / "binary.dat").write_bytes(bytes(range(256)) * 8)
    bulk = probe / "bulk"
    bulk.mkdir()
    for i in range(40):
        (bulk / f"row{i:03d}.txt").write_text(f"needle {i}\nfiller\n", encoding="utf-8")
    (probe / "long.txt").write_text(
        "".join(f"needle line {i}\n" for i in range(2000)), encoding="utf-8"
    )
    unreadable = probe / "locked.md"
    unreadable.write_text("cannot read me\n", encoding="utf-8")
    os.chmod(unreadable, 0o000)

    # a hidden file, and a large file for truncation
    (root / ".hidden.md").write_text("hidden\n", encoding="utf-8")
    (root / "big.txt").write_text(
        "".join(f"row {i}\n" for i in range(3000)), encoding="utf-8"
    )

    # a directory with many entries for listing truncation
    many = root / "many"
    many.mkdir()
    for i in range(250):
        (many / f"f{i:03d}.txt").write_text("x\n", encoding="utf-8")


def build_sol_execution() -> dict[str, Any]:
    """The `sol` tool's observation text is what the model reads back.

    Its caps, its section order, its truncation marker and its outcome strings
    were pinned by nothing before v5, so a rebuild would have been accepted
    against values transcribed from reading the reference.
    """
    from solstone.think.providers import openhands as oh

    fmt = oh._format_shell_output
    trunc = oh._truncate_output
    cap = oh._SHELL_STDOUT_CAP

    cases: list[tuple[str, dict[str, Any], str]] = [
        (
            "fmt_stdout_only",
            {"stdout": "hello\n", "stderr": "", "returncode": 0, "timed_out": False},
            "openhands.py:613-615",
        ),
        (
            "fmt_stderr_only",
            {"stdout": "", "stderr": "bad\n", "returncode": 0, "timed_out": False},
            "openhands.py:616-617",
        ),
        (
            "fmt_both",
            {"stdout": "out", "stderr": "err", "returncode": 0, "timed_out": False},
            "openhands.py:613-617 - stdout section first, blank line between",
        ),
        (
            "fmt_neither_ok",
            {"stdout": "", "stderr": "", "returncode": 0, "timed_out": False},
            "openhands.py:622-623 - the bare success token",
        ),
        (
            "fmt_nonzero_exit",
            {"stdout": "out", "stderr": "", "returncode": 3, "timed_out": False},
            "openhands.py:620-621",
        ),
        (
            "fmt_nonzero_exit_no_output",
            {"stdout": "", "stderr": "", "returncode": 1, "timed_out": False},
            "openhands.py:620-623 - exit_code alone, NOT 'ok'",
        ),
        (
            "fmt_timeout",
            {"stdout": "", "stderr": "", "returncode": None, "timed_out": True},
            "openhands.py:618-619 - the seconds value is INTERPOLATED, not literal",
        ),
        (
            "fmt_timeout_with_partial",
            {
                "stdout": "partial out",
                "stderr": "partial err",
                "returncode": None,
                "timed_out": True,
            },
            "openhands.py:613-619 - partial output survives a timeout",
        ),
        (
            "fmt_returncode_none_not_timed_out",
            {"stdout": "x", "stderr": "", "returncode": None, "timed_out": False},
            "openhands.py:620 - a None returncode adds no exit_code line",
        ),
    ]
    rows: list[dict[str, Any]] = []
    for case_id, kwargs, citation in cases:
        rows.append(
            {
                "id": case_id,
                "args": kwargs,
                "citation": citation,
                "expect": fmt(**kwargs),
            }
        )

    # truncation, including the multibyte boundary a byte-slicing port panics on
    trunc_cases: list[tuple[str, str, int, str]] = [
        (
            "trunc_under_cap",
            "abc",
            10,
            "openhands.py:627-630 - untouched under the cap",
        ),
        ("trunc_at_cap", "a" * 10, 10, "openhands.py:628 - == cap is NOT truncated"),
        ("trunc_over_cap", "a" * 11, 10, "openhands.py:630 - marker appended"),
        (
            "trunc_multibyte_boundary",
            "é" * 11,
            10,
            "openhands.py:629 - Python slices CODE POINTS; a byte-slicing port panics here",
        ),
        (
            "trunc_mixed_multibyte",
            "a" * 8 + "日本語",
            10,
            "openhands.py:629 - the cut lands mid-multibyte-sequence by byte count",
        ),
        ("trunc_empty", "", 10, "openhands.py:628"),
    ]
    trunc_rows: list[dict[str, Any]] = []
    for case_id, text, limit, citation in trunc_cases:
        out = trunc(text, limit)
        trunc_rows.append(
            {
                "id": case_id,
                "input_chars": len(text),
                "input_bytes": len(text.encode("utf-8")),
                "cap": limit,
                "citation": citation,
                "expect": out,
                "expect_chars": len(out),
                "expect_bytes": len(out.encode("utf-8")),
            }
        )

    # a real spawn, for the outcomes formatting alone cannot produce
    run_rows: list[dict[str, Any]] = []
    for case_id, argv, citation in [
        (
            "run_not_found",
            ["definitely-not-a-real-binary-xyz"],
            "openhands.py:569-570 - command_not_found, is_error",
        ),
    ]:
        result = oh._run_command(argv)
        run_rows.append(
            {
                "id": case_id,
                "argv": argv,
                "citation": citation,
                "expect": {"text": result["text"], "is_error": result["is_error"]},
            }
        )

    return {
        "note": (
            "The observation text the model reads back from the sol tool. "
            "Produced by executing the reference's own formatters, not "
            "transcribed. The seconds value in the timeout line and the caps are "
            "INTERPOLATED from constants, so a rebuild must derive them too."
        ),
        "limits": {
            "stdout_cap_chars": oh._SHELL_STDOUT_CAP,
            "stderr_cap_chars": oh._SHELL_STDERR_CAP,
            "timeout_seconds": oh._SHELL_TIMEOUT_SECONDS,
            "truncation_marker": trunc("x" * (cap + 1), cap)[cap:],
            "citation": "openhands.py:95-97,627-630",
        },
        "format_shell_output": rows,
        "truncate_output": trunc_rows,
        "run_command": run_rows,
    }


PART_SEPARATOR = "\n\n"


def _decompose_system_instruction(
    system: str | None,
    config: dict[str, Any],
    tool_name: str | None,
    diagnostic: bool,
) -> dict[str, Any] | None:
    """Record the system instruction as ORDERED PARTS, never as one blob.

    The blob would embed the runtime preamble, putting a second copy of it in
    this file -- at a DIFFERENT value from the frozen `preambles` block, and
    somewhere the divergence ledger cannot see. What this block is actually for
    is composition: which parts, in what order, joined how. So that is what it
    records, and the preamble appears only by identity.

    The decomposition is checked against the reference's real output before it is
    recorded, so it cannot drift into fiction.
    """
    from solstone.think.providers.cli import cogitate_sol_tool_hint

    if system is None:
        return None

    scope_hint = (
        "Limit filesystem reads to today's segment dir unless the task explicitly "
        "requires broader history. If you need broader scope, state what and why "
        "in your reasoning."
    )

    parts: list[dict[str, Any]] = []
    if diagnostic:
        parts.append({"role": "diagnostic_preamble", "text": None})
    elif tool_name:
        parts.append({"role": "runtime_preamble", "text": None})
    if config.get("system_instruction"):
        parts.append(
            {"role": "talent_system_instruction", "text": config["system_instruction"]}
        )
    if tool_name and not diagnostic:
        parts.append(
            {"role": "sol_tool_hint", "text": cogitate_sol_tool_hint(tool_name)}
        )
    if config.get("read_scope") and not diagnostic:
        parts.append({"role": "read_scope_hint", "text": scope_hint})

    order = [part["role"] for part in parts]
    return {
        "parts": parts,
        "order": order,
        "separator": PART_SEPARATOR,
        "byte_length": len(system.encode("utf-8")),
        "sha256": hashlib.sha256(system.encode("utf-8")).hexdigest(),
        "note": (
            "A part whose text is null is identified rather than duplicated: its "
            "bytes live in the preambles block, which is frozen against a "
            "recorded divergence. Reassemble by joining the parts with the "
            "separator, substituting the named constant."
        ),
    }


def build_prompt_vectors() -> list[dict[str, Any]]:
    """The assembled system instruction is model-visible contract.

    ``assemble_prompt`` decides what heads a talent's system prompt, in what
    order, and what the tool-routing hint says. A rebuild that gets the ORDER
    wrong produces a prompt that still contains every part and still reads
    plausibly, which no other check would notice.
    """
    from solstone.think.providers.cli import assemble_prompt, cogitate_sol_tool_hint

    cases: list[tuple[str, dict[str, Any], str | None, bool, str]] = [
        (
            "prompt_plain",
            {"prompt": "do the thing"},
            "sol",
            False,
            "cli.py:138-144 - preamble, then the talent instruction, then the hint",
        ),
        (
            "prompt_with_system_instruction",
            {"prompt": "body", "system_instruction": "TALENT RULES"},
            "sol",
            False,
            "cli.py:138-144 - the talent's own instruction sits BETWEEN preamble and hint",
        ),
        (
            "prompt_no_sol_tool",
            {"prompt": "body", "system_instruction": "TALENT RULES"},
            None,
            False,
            "cli.py:138 - no sol tool name means no preamble and no hint at all",
        ),
        (
            "prompt_diagnostic",
            {"prompt": "body", "system_instruction": "TALENT RULES"},
            "sol",
            True,
            "cli.py:133-137 - diagnostic swaps in the OTHER preamble and drops the hint",
        ),
        (
            "prompt_read_scope_hint",
            {"prompt": "body", "read_scope": ["chronicle/20260809"]},
            "sol",
            False,
            "cli.py:145-153 - read_scope appends a prose hint; it is its ONLY live effect",
        ),
        (
            "prompt_read_scope_hint_diagnostic",
            {"prompt": "body", "read_scope": ["chronicle/20260809"]},
            "sol",
            True,
            "cli.py:145 - diagnostic suppresses the read_scope hint",
        ),
        (
            "prompt_join_order",
            {
                "transcript": "TRANSCRIPT",
                "extra_context": "EXTRA",
                "user_instruction": "USER",
                "prompt": "PROMPT",
            },
            "sol",
            False,
            "cli.py:126-131 - the body joins these four keys in THIS order with blank lines",
        ),
        (
            "prompt_empty",
            {},
            "sol",
            False,
            "cli.py:126-131 - no parts means an empty body",
        ),
    ]
    rows: list[dict[str, Any]] = []
    for case_id, config, tool_name, diagnostic, citation in cases:
        body, system = assemble_prompt(
            config, sol_tool_name=tool_name, diagnostic=diagnostic
        )
        rows.append(
            {
                "id": case_id,
                "config": config,
                "sol_tool_name": tool_name,
                "diagnostic": diagnostic,
                "citation": citation,
                "expect": {
                    "prompt_body": body,
                    "system_instruction": _decompose_system_instruction(
                        system, config, tool_name, diagnostic
                    ),
                },
            }
        )
    rows.append(
        {
            "id": "sol_tool_hint",
            "config": {},
            "sol_tool_name": "sol",
            "diagnostic": False,
            "citation": "cli.py:89-102 - the routing hint, derived from the approved family list",
            "expect": {
                "prompt_body": None,
                "system_instruction": {
                    "parts": [
                        {
                            "role": "sol_tool_hint",
                            "text": cogitate_sol_tool_hint("sol"),
                        }
                    ],
                    "separator": PART_SEPARATOR,
                },
            },
        }
    )
    return rows


def _talent_finalization_branches() -> dict[str, Any]:
    """Which real talents take which finalization branch.

    Reachability, measured rather than assumed -- a surface no talent reaches is
    speculative work, and a surface most of them reach is load-bearing.
    """
    import glob

    rows: dict[str, str] = {}
    for path in sorted(
        glob.glob("solstone/talent/*.md") + glob.glob("solstone/apps/*/talent/*.md")
    ):
        text = Path(path).read_text(encoding="utf-8")
        depth = 0
        end = None
        for index, char in enumerate(text):
            if char == "{":
                depth += 1
            elif char == "}":
                depth -= 1
                if depth == 0:
                    end = index + 1
                    break
        if end is None:
            continue
        try:
            meta = json.loads(text[:end])
        except json.JSONDecodeError:
            continue
        if meta.get("type") != "cogitate":
            continue
        rows[Path(path).name] = "emit_final" if expects_emit_final(meta) else "finish"
    return {
        "by_talent": rows,
        "finish_branch_count": sum(1 for value in rows.values() if value == "finish"),
        "emit_final_branch_count": sum(
            1 for value in rows.values() if value == "emit_final"
        ),
        "citation": "cogitate_contract.py:110-116 over the shipped talent frontmatter",
    }


def build_tool_surface() -> dict[str, Any]:
    """What the model is actually handed, per access tier.

    Every string here reaches the model and is therefore contract: a tool's
    name, its description, and its argument's description. They are BUILT by
    calling the real constructors rather than copied, because two of them
    interpolate the approved journal-command list and a pasted copy would stop
    tracking it.
    """
    from solstone.think.cogitate_policy import CogitatePolicy
    from solstone.think.providers.emit_final_tool import (
        TOOL_DESCRIPTION as EMIT_FINAL_DESCRIPTION,
    )
    from solstone.think.providers.emit_final_tool import build_emit_final_tools
    from solstone.think.providers.openhands import _build_sol_tools
    from solstone.think.providers.read_tools import build_read_tools
    from solstone.think.providers.shared import JSONEventCallback

    def describe(tool: Any) -> dict[str, Any]:
        row: dict[str, Any] = {"name": tool.name, "description": tool.description}
        schema = None
        action_type = getattr(tool, "action_type", None)
        if action_type is not None and hasattr(action_type, "model_json_schema"):
            schema = action_type.model_json_schema()
        if schema:
            # `kind` is an SDK-injected discriminator with a null description.
            # A native tool must NOT reproduce it, so recording it would make an
            # equality assertion against this block unsatisfiable.
            row["action_properties"] = {
                key: {
                    "type": value.get("type"),
                    "description": value.get("description"),
                }
                for key, value in sorted(schema.get("properties", {}).items())
                if key not in SDK_INJECTED_ACTION_PROPERTIES
            }
            row["sdk_injected_properties_excluded"] = sorted(
                key
                for key in schema.get("properties", {})
                if key in SDK_INJECTED_ACTION_PROPERTIES
            )
            row["action_required"] = sorted(schema.get("required", []))
        return row

    policy = CogitatePolicy(allowed_roots=[Path("/journal")], access_tier="normal")
    sol_tools, _executor = _build_sol_tools(
        policy=policy,
        callback=JSONEventCallback(None),
        read_call_budget=DEFAULT_READ_CALL_BUDGET_VALUE,
    )
    read_tools = build_read_tools(journal=Path("/journal"), read_call_budget=200)
    emit_final_tools = build_emit_final_tools()

    tools = {
        "sol": describe(sol_tools[0]),
        "emit_final": describe(emit_final_tools[0]),
    }

    # The finish tool belongs to the agent SDK, not to solstone -- and 4 of the 6
    # cogitate talents finalize through it. The conversion removes that SDK, so
    # the rebuild must DEFINE this surface, and this is the only moment its
    # current text can be recorded rather than invented.
    try:
        from openhands.sdk.tool import FinishTool

        made = FinishTool.create()
        tools["finish"] = describe(made[0] if isinstance(made, list) else made)
        tools["finish"]["ownership"] = (
            "AGENT SDK, not solstone. Recorded so the rebuild has a reference "
            "instead of inventing one; the rebuild OWNS this surface afterwards."
        )
    except Exception as exc:  # pragma: no cover - depends on the installed SDK
        tools["finish"] = {
            "unavailable": True,
            "error": str(exc),
            "note": "the SDK finish tool could not be resolved on this host",
        }

    for tool in read_tools:
        tools[tool.name] = describe(tool)

    # Which tools each tier is HANDED. This is the binding contract: it is
    # decided by the capability table plus the finalization rule, and today it
    # lives only as two `if` statements in the runtime.
    bindings: dict[str, Any] = {}
    for tier in COGITATE_ACCESS_TIERS:
        caps = capabilities_for_access_tier(tier)
        names: list[str] = []
        if caps.sol:
            names.append("sol")
        if caps.reads:
            names.extend(sorted(COGITATE_READ_TOOL_NAMES))
        bindings[tier] = {
            "sol": caps.sol,
            "reads": caps.reads,
            "submit": caps.submit,
            "model_tools_excluding_finalization": sorted(names),
        }
    talent_branches = _talent_finalization_branches()
    finalization = {
        "emit_final": {
            "bound": ["emit_final"],
            "default_tools": [],
            "citation": "openhands.py:1373-1380 - emit_final REPLACES the built-in finish tool",
        },
        "finish": {
            "bound": [],
            "default_tools": ["FinishTool"],
            "citation": "openhands.py:1373 - otherwise the built-in finish tool is included",
        },
    }
    return {
        "note": (
            "Every name and description here reaches the model and is therefore "
            "contract. Built by calling the real constructors, not copied. "
            "NO WRITE TOOL APPEARS IN ANY TIER'S SET, and that structural fact -- "
            "not the dead policy gate -- is what makes 'there is no "
            "general-purpose write tool' true."
        ),
        "tools": tools,
        "tier_bindings": bindings,
        "finalization_binding": finalization,
        "talent_finalization_branches": talent_branches,
        "emit_final_description_source": EMIT_FINAL_DESCRIPTION,
        "citation": "openhands.py:1347-1380 - the whole binding sequence",
    }


def build_bed_manifest(root: Path) -> list[dict[str, Any]]:
    """A sorted census of the bed, so a reimplementation can prove its own bed
    matches BEFORE it asserts a single vector.

    Without this, a bed mismatch presents as a vector mismatch -- indistinguishable
    from an implementation bug, and the cheapest escape is to tweak the bed until
    the vectors go green, which silently fits the bed to the implementation and
    destroys the oracle's value.
    """
    rows: list[dict[str, Any]] = []
    for path in sorted(root.rglob("*"), key=lambda p: p.relative_to(root).as_posix()):
        rel = path.relative_to(root).as_posix()
        if path.is_symlink():
            rows.append(
                {
                    "path": rel,
                    "type": "symlink",
                    "target": os.readlink(path),
                    "escapes_root": not str(Path(os.path.realpath(path))).startswith(
                        str(Path(os.path.realpath(root))) + os.sep
                    ),
                }
            )
            continue
        mode = path.lstat().st_mode
        if stat.S_ISFIFO(mode):
            rows.append({"path": rel, "type": "fifo"})
        elif path.is_dir():
            rows.append({"path": rel, "type": "dir"})
        elif path.is_file():
            row: dict[str, Any] = {"path": rel, "type": "file"}
            readable = os.access(path, os.R_OK)
            row["mode"] = oct(stat.S_IMODE(mode))
            row["readable"] = readable
            if readable:
                data = path.read_bytes()
                row["byte_length"] = len(data)
                row["sha256"] = hashlib.sha256(data).hexdigest()
            rows.append(row)
        else:
            rows.append({"path": rel, "type": "other"})
    return rows


def _serialize(value: Any) -> Any:
    if is_dataclass(value) and not isinstance(value, type):
        return {k: _serialize(v) for k, v in asdict(value).items()}
    if isinstance(value, (list, tuple)):
        return [_serialize(v) for v in value]
    if isinstance(value, dict):
        return {k: _serialize(v) for k, v in value.items()}
    return value


def _result_row(
    case_id: str,
    tool: str,
    kwargs: dict[str, Any],
    citation: str,
    result: crt.ReadResult,
) -> dict[str, Any]:
    payload = _serialize(result.payload)
    # read_file payloads are text; hash long ones so the fixture stays diffable
    payload_repr: Any
    if isinstance(payload, str) and len(payload) > 400:
        payload_repr = {
            "kind": "sha256",
            "byte_length": len(payload.encode("utf-8")),
            "digest": hashlib.sha256(payload.encode("utf-8")).hexdigest(),
            "head": payload[:120],
            "tail": payload[-120:],
        }
    elif isinstance(payload, list) and len(payload) > 25:
        payload_repr = {
            "kind": "list_summary",
            "count": len(payload),
            "head": payload[:5],
            "tail": payload[-5:],
        }
    else:
        payload_repr = payload
    return {
        "id": case_id,
        "tool": tool,
        "args": _serialize(kwargs),
        "citation": citation,
        "expect": {
            "ok": result.ok,
            "refusal": result.refusal,
            "truncated": result.truncated,
            "notice": result.notice,
            "payload": payload_repr,
        },
    }


def build_read_tool_vectors(bed: Path) -> list[dict[str, Any]]:
    j = str(bed)
    rows: list[dict[str, Any]] = []

    def rf(case_id, citation, **kw):
        rows.append(
            _result_row(case_id, "read_file", kw, citation, crt.read_file(j, **kw))
        )

    def ld(case_id, citation, **kw):
        rows.append(
            _result_row(
                case_id, "list_directory", kw, citation, crt.list_directory(j, **kw)
            )
        )

    def gl(case_id, citation, **kw):
        rows.append(_result_row(case_id, "glob", kw, citation, crt.glob(j, **kw)))

    def gs(case_id, citation, **kw):
        rows.append(
            _result_row(case_id, "grep_search", kw, citation, crt.grep_search(j, **kw))
        )

    # --- read_file
    rf("rf_ok", "cogitate_read_tools.py:426", path="facets/work.md")
    rf(
        "rf_nested",
        "cogitate_read_tools.py:426",
        path="chronicle/20260809/default/120000_60/notes.md",
    )
    rf(
        "rf_date_prefixed",
        "cogitate_read_tools.py:437-439 - an 8-digit head resolves under chronicle/",
        path="20260809/default/120000_60/notes.md",
    )
    rf(
        "rf_start_line",
        "cogitate_read_tools.py:426",
        path="facets/work.md",
        start_line=2,
    )
    rf(
        "rf_start_line_past_end",
        "cogitate_read_tools.py:426",
        path="facets/work.md",
        start_line=99,
    )
    rf("rf_max_lines", "cogitate_read_tools.py:426", path="big.txt", max_lines=3)
    rf("rf_truncated_by_lines", "cogitate_read_tools.py:118-121", path="big.txt")
    rf(
        "rf_truncated_by_bytes",
        "cogitate_read_tools.py:118-121",
        path="big.txt",
        max_bytes=64,
    )
    rf("rf_empty_file", "cogitate_read_tools.py:426", path="empty.txt")
    rf("rf_missing", "cogitate_read_tools.py:92-95", path="nope.md")
    rf("rf_directory", "cogitate_read_tools.py:80-83", path="facets")
    rf("rf_binary", "cogitate_read_tools.py:84-87", path="blob.bin")
    rf("rf_fifo", "cogitate_read_tools.py:88-91", path="fifo")
    rf("rf_denied_git", "cogitate_read_tools.py:72-75", path=".git/config")
    rf("rf_denied_cache", "cogitate_read_tools.py:72-75", path=".cache/x.txt")
    rf(
        "rf_denied_node_modules",
        "cogitate_read_tools.py:72-75",
        path="node_modules/pkg.json",
    )
    rf("rf_denied_venv", "cogitate_read_tools.py:72-75", path=".venv/pyvenv.cfg")
    # The preamble tells the model the denylist covers "credentials". These pin what
    # DENIED_CREDENTIAL_PATTERNS actually matches - including the names it does not.
    for name in (
        "id_rsa",
        "id_rsa.pub",
        ".env",
        ".env.local",
        "server.key",
        "key.pem",
        "my.credentials",
        "credentials",
        "credentials.json",
        "token.key",
        "api_secret.txt",
        "token.txt",
        "passwords.md",
        "secrets.yaml",
    ):
        rf(
            f"rf_credential_{name.strip('.')}",
            "cogitate_read_tools.py:53-61,76-79",
            path=f"secrets/{name}",
        )
    rf("rf_traversal", "cogitate_read_tools.py:68-71", path="../outside/leak.txt")
    rf("rf_traversal_absolute", "cogitate_read_tools.py:68-71", path="/etc/passwd")
    rf("rf_symlink_escape", "cogitate_read_tools.py:68-71", path="escape")
    rf("rf_symlink_inside", "cogitate_read_tools.py:426", path="inside_link")
    rf("rf_dot", "cogitate_read_tools.py:437-439", path=".")
    rf("rf_empty_path", "cogitate_read_tools.py:437-439", path="")
    rf("rf_hidden", "cogitate_read_tools.py:426", path=".hidden.md")

    # --- list_directory
    ld("ld_root", "cogitate_read_tools.py:509", path=".")
    ld("ld_root_hidden", "cogitate_read_tools.py:509", path=".", include_hidden=True)
    ld("ld_facets", "cogitate_read_tools.py:509", path="facets")
    ld(
        "ld_recursive_chronicle",
        "cogitate_read_tools.py:509",
        path="chronicle",
        recursive=True,
    )
    ld(
        "ld_recursive_root_broad",
        "cogitate_read_tools.py:271-279 - broad recursive roots are refused",
        path=".",
        recursive=True,
    )
    ld(
        "ld_chronicle_nonrecursive_ok",
        "cogitate_read_tools.py:271-279 - the broad-root rule is about RECURSION",
        path="chronicle",
        recursive=False,
    )
    ld(
        "ld_recursive_facets_broad",
        "cogitate_read_tools.py:271 - facets/ is the third broad root",
        path="facets",
        recursive=True,
    )
    ld(
        "ld_recursive_day_ok",
        "cogitate_read_tools.py:271-279 - one level down is not broad",
        path="chronicle/20260809",
        recursive=True,
    )
    ld(
        "ld_recursive_talents_ok",
        "cogitate_read_tools.py:271 - talents/ is NOT a broad root",
        path="talents",
        recursive=True,
    )
    ld("ld_truncated", "cogitate_read_tools.py:122-125", path="many")
    ld("ld_max_entries", "cogitate_read_tools.py:122-125", path="many", max_entries=5)
    ld("ld_pattern", "cogitate_read_tools.py:509", path="many", pattern="f00*.txt")
    ld("ld_denied", "cogitate_read_tools.py:72-75", path=".git")
    ld("ld_missing", "cogitate_read_tools.py:92-95", path="nope")
    ld("ld_on_a_file", "cogitate_read_tools.py:80-83", path="facets/work.md")

    # --- glob
    gl("gl_md", "cogitate_read_tools.py:586", pattern="*.md")
    gl(
        "gl_crosses_slash",
        "cogitate_read_tools.py:596-598 - fnmatch `*` spans `/`",
        pattern="*.jsonl",
    )
    gl("gl_rooted", "cogitate_read_tools.py:586", pattern="*.jsonl", root="talents")
    gl("gl_no_match", "cogitate_read_tools.py:586", pattern="*.zzz")
    gl(
        "gl_truncated",
        "cogitate_read_tools.py:126-128",
        pattern="many/*.txt",
        max_matches=5,
    )
    gl("gl_denied_root", "cogitate_read_tools.py:72-75", pattern="*", root=".git")
    gl("gl_hidden_excluded", "cogitate_read_tools.py:335-337", pattern="*.md")
    gl(
        "gl_hidden_included",
        "cogitate_read_tools.py:335-337",
        pattern="*.md",
        include_hidden=True,
    )
    gl("gl_would_match_credential", "cogitate_read_tools.py:76-79", pattern="secrets/*")
    # glob is inherently recursive, so its DEFAULT root refuses. Pin where it works.
    gl(
        "gl_deep_root_ok",
        "cogitate_read_tools.py:271-279 - a non-broad root admits glob",
        pattern="*.jsonl",
        root="chronicle/20260809",
    )
    gl("gl_talents_root_ok", "cogitate_read_tools.py:271", pattern="*", root="talents")
    gl(
        "gl_secrets_root",
        "cogitate_read_tools.py:76-79 - rooted at secrets/, which credential names survive?",
        pattern="*",
        root="secrets",
    )

    # --- grep_search: same broad-root rule, pinned in both directions
    gs(
        "gs_deep_ok",
        "cogitate_read_tools.py:271-279 - a non-broad path admits grep",
        pattern="hello",
        path="chronicle/20260809",
    )
    gs("gs_talents_ok", "cogitate_read_tools.py:271", pattern="finish", path="talents")
    gs(
        "gs_facets_broad",
        "cogitate_read_tools.py:271 - facets/ is a broad root for grep too",
        pattern="sunlight",
        path="facets",
    )
    gs(
        "gs_single_file_path",
        "cogitate_read_tools.py:698 - a file path is not a broad root",
        pattern="sunlight",
        path="facets/work.md",
    )

    # --- grep_search
    gs("gs_literal", "cogitate_read_tools.py:698", pattern="sunlight")
    gs("gs_case_insensitive_default", "cogitate_read_tools.py:719", pattern="SUNLIGHT")
    gs(
        "gs_case_sensitive",
        "cogitate_read_tools.py:719",
        pattern="SUNLIGHT",
        case_sensitive=True,
    )
    gs("gs_regex", "cogitate_read_tools.py:720-723", pattern=r"line \w+", regex=True)
    gs("gs_regex_literal_escaped", "cogitate_read_tools.py:720-723", pattern="line .")
    gs(
        "gs_bad_regex",
        "cogitate_read_tools.py:104-107",
        pattern="[unclosed",
        regex=True,
    )
    gs("gs_context", "cogitate_read_tools.py:698", pattern="line two", context_lines=1)
    gs("gs_file_glob", "cogitate_read_tools.py:698", pattern="line", file_glob="*.md")
    gs(
        "gs_scoped_path",
        "cogitate_read_tools.py:698",
        pattern="hello",
        path="chronicle",
    )
    gs("gs_denied_path", "cogitate_read_tools.py:72-75", pattern="x", path=".git")
    gs("gs_no_match", "cogitate_read_tools.py:698", pattern="zzzznotpresent")
    gs("gs_max_matches", "cogitate_read_tools.py:129-132", pattern="row", max_matches=3)
    gs(
        "gs_bytes_per_file",
        "cogitate_read_tools.py:698",
        pattern="row",
        max_bytes_per_file=64,
    )

    # --- v3: glob and grep, rooted below the broad-root rule so they actually RUN.
    # 22 of v2's 85 vectors were the same `broad_root` refusal, which meant an
    # implementation ignoring case_sensitive, regex, context_lines, file_glob,
    # include_hidden and every truncation cap passed all of them.
    gl("gl_probe_md", "cogitate_read_tools.py:586", pattern="*.md", root="probe")
    gl(
        "gl_probe_hidden_excluded",
        "cogitate_read_tools.py:335-337 - hidden excluded by default",
        pattern="*.md",
        root="probe",
    )
    gl(
        "gl_probe_hidden_included",
        "cogitate_read_tools.py:335-337 - include_hidden admits the dotfile",
        pattern="*.md",
        root="probe",
        include_hidden=True,
    )
    # The pattern is matched against the JOURNAL-relative path, not the root-relative
    # one, so `root=` narrows the walk while the pattern must still carry the prefix.
    # Both directions pinned, because a rebuild will plausibly get this backwards.
    gl(
        "gl_probe_pattern_is_journal_relative",
        "cogitate_read_tools.py:596-598 - `probe/bulk/*.txt` MATCHES under root=probe",
        pattern="probe/bulk/*.txt",
        root="probe",
    )
    gl(
        "gl_probe_pattern_not_root_relative",
        "cogitate_read_tools.py:596-598 - the same pattern WITHOUT the root prefix matches nothing",
        pattern="bulk/*.txt",
        root="probe",
    )
    gl(
        "gl_probe_star_crosses_dir",
        "cogitate_read_tools.py:596-598 - a bare `*.txt` reaches INTO bulk/",
        pattern="*.txt",
        root="probe",
    )
    gl(
        "gl_probe_truncated",
        "cogitate_read_tools.py:126-128 - NOTICE_GLOB_TRUNCATED",
        pattern="probe/bulk/*.txt",
        root="probe",
        max_matches=5,
    )
    gl(
        "gl_probe_on_a_file",
        "cogitate_read_tools.py:586 - a file as root",
        pattern="*",
        root="probe/alpha.md",
    )

    gs(
        "gs_probe_case_insensitive_default",
        "cogitate_read_tools.py:719 - default folds case, proven where it can run",
        pattern="sunlight",
        path="probe",
    )
    gs(
        "gs_probe_case_sensitive",
        "cogitate_read_tools.py:719 - case_sensitive=True must miss the other spellings",
        pattern="sunlight",
        path="probe",
        case_sensitive=True,
    )
    gs(
        "gs_probe_regex",
        "cogitate_read_tools.py:720-723 - regex=True compiles the pattern",
        pattern=r"needle \d+",
        regex=True,
        path="probe/bulk",
    )
    gs(
        "gs_probe_regex_off_is_literal",
        "cogitate_read_tools.py:720-723 - regex=False escapes, so the same pattern MISSES",
        pattern=r"needle \d+",
        path="probe/bulk",
    )
    gs(
        "gs_probe_context",
        "cogitate_read_tools.py:698 - context_lines fills before/after",
        pattern="SUNLIGHT shouting",
        path="probe",
        context_lines=1,
        case_sensitive=True,
    )
    gs(
        "gs_probe_file_glob",
        "cogitate_read_tools.py:698 - file_glob narrows the file set",
        pattern="sunlight",
        path="probe",
        file_glob="*.md",
    )
    gs(
        "gs_probe_max_matches",
        "cogitate_read_tools.py:129-132 - NOTICE_GREP_TRUNCATED",
        pattern="needle",
        path="probe/bulk",
        max_matches=4,
    )
    gs(
        "gs_probe_max_files",
        "cogitate_read_tools.py:698 - max_files bounds the walk",
        pattern="needle",
        path="probe/bulk",
        max_files=3,
    )
    gs(
        "gs_probe_max_bytes_per_file",
        "cogitate_read_tools.py:698 - only the first max_bytes_per_file of each file is searched",
        pattern="needle line 1999",
        path="probe",
        max_bytes_per_file=64,
    )
    gs(
        "gs_probe_binary_is_skipped",
        "cogitate_read_tools.py:650-697 - a non-UTF-8 file is skipped, not refused",
        pattern="needle",
        path="probe",
        file_glob="binary.dat",
    )
    gs(
        "gs_probe_hidden_excluded",
        "cogitate_read_tools.py:335-337",
        pattern="sunlight",
        path="probe",
        file_glob="*.md",
        include_hidden=True,
    )
    rf(
        "rf_permission_denied",
        "cogitate_read_tools.py:96-99 - REFUSAL_PERMISSION_DENIED",
        path="probe/locked.md",
    )
    ld("ld_probe", "cogitate_read_tools.py:509 - a non-broad directory", path="probe")

    # --- the shared budget
    budget = crt.ReadBudget(cap=2)
    rows.append(
        _result_row(
            "budget_call_1",
            "read_file",
            {"path": "facets/work.md", "budget_cap": 2},
            "cogitate_read_tools.py:218-222",
            crt.read_file(j, "facets/work.md", budget=budget),
        )
    )
    rows.append(
        _result_row(
            "budget_call_2",
            "list_directory",
            {"path": "facets", "budget_cap": 2},
            "cogitate_read_tools.py:218-222",
            crt.list_directory(j, "facets", budget=budget),
        )
    )
    rows.append(
        _result_row(
            "budget_call_3_exhausted",
            "glob",
            {"pattern": "*", "budget_cap": 2},
            "cogitate_read_tools.py:108-111",
            crt.glob(j, "*", budget=budget),
        )
    )
    return rows


# ---------------------------------------------------------------------------
# assemble
# ---------------------------------------------------------------------------


def reference_commit() -> str:
    try:
        return subprocess.run(
            ["git", "rev-parse", "HEAD"],
            cwd=REPO_ROOT,
            capture_output=True,
            text=True,
            check=True,
        ).stdout.strip()
    except Exception:
        return "unknown"


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--out", required=True, type=Path)
    parser.add_argument(
        "--rethaw-preambles",
        action="store_true",
        help=(
            "Re-derive the preamble block from live source instead of carrying "
            "forward the frozen one. Only correct before any divergence has been "
            "recorded against it."
        ),
    )
    args = parser.parse_args()

    # A section is only FROZEN if the generator refuses to re-derive it.
    #
    # The preamble block records what the reference said BEFORE the runtime
    # preamble was deliberately corrected. Re-deriving it from live source would
    # silently overwrite the "before" side of a recorded divergence and make the
    # divergence itself disappear -- which is exactly the rot the ledger exists
    # to prevent. Discovered the hard way: a regeneration did precisely this and
    # the ledger's stale-entry check caught it.
    carried_preambles: dict[str, Any] | None = None
    if args.out.exists() and not args.rethaw_preambles:
        try:
            existing = json.loads(args.out.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError):
            existing = {}
        if isinstance(existing.get("preambles"), dict):
            carried_preambles = existing["preambles"]

    tmp = Path(tempfile.mkdtemp(prefix="cogitate-oracle-"))
    try:
        bed = tmp / "journal"
        bed.mkdir()
        build_bed(bed)
        bed_manifest = build_bed_manifest(bed)
        read_tool_rows = build_read_tool_vectors(bed)
    finally:
        # the 0o000 probe file would otherwise defeat rmtree
        for path in tmp.rglob("*"):
            try:
                if path.is_file() and not path.is_symlink():
                    os.chmod(path, 0o600)
            except OSError:
                pass
        shutil.rmtree(tmp, ignore_errors=True)

    runtime = COGITATE_RUNTIME_PREAMBLE.encode("utf-8")
    diagnostic = COGITATE_DIAGNOSTIC_PREAMBLE.encode("utf-8")

    doc = {
        "fixture": FIXTURE_NAME,
        "fixture_version": FIXTURE_VERSION,
        "generated_by": "scripts/cogitate_oracle_corpus.py",
        "reference_commit": reference_commit(),
        "provenance": (
            "Every vector below was produced by EXECUTING the Python reference, "
            "one call per case. Nothing here was written by hand from reading the "
            "source. CLOCK: this file is a FROZEN RECORD - it can only be "
            "regenerated while the Python reference is still importable, and it "
            "stops being reproducible when Python is cut. Tests read it; they "
            "never execute Python."
        ),
        "preambles": carried_preambles
        if carried_preambles is not None
        else {
            "runtime": {
                "algorithm": "sha256",
                "encoding": "utf-8",
                "byte_length": len(runtime),
                "digest": hashlib.sha256(runtime).hexdigest(),
                "text": COGITATE_RUNTIME_PREAMBLE,
                "citation": "cogitate_contract.py:26-35",
            },
            "diagnostic": {
                "algorithm": "sha256",
                "encoding": "utf-8",
                "byte_length": len(diagnostic),
                "digest": hashlib.sha256(diagnostic).hexdigest(),
                "text": COGITATE_DIAGNOSTIC_PREAMBLE,
                "citation": "cogitate_contract.py:37-43",
            },
        },
        "vocabularies": {
            "journal_commands": list(COGITATE_JOURNAL_COMMANDS),
            "read_tools": list(COGITATE_READ_TOOL_NAMES),
            "finalization_modes": list(TALENT_FINALIZATION_MODES),
            "deterministic_failure_reason_codes": sorted(
                DETERMINISTIC_FAILURE_REASON_CODES
            ),
            "deterministic_failure_caps": dict(
                sorted(DETERMINISTIC_FAILURE_CAPS.items())
            ),
            "citation": "cogitate_contract.py:18,66,69 - cogitate_policy.py:41-75",
        },
        "access_tiers": build_capability_vectors(),
        "policy_commands": build_policy_vectors(),
        "policy_check": build_check_vectors(),
        "read_scope": build_read_scope_vectors(),
        "expects_emit_final": build_emit_final_vectors(),
        "failure_caps": build_failure_cap_vectors(),
        "bed_manifest": {
            "note": (
                "A sorted census of the filesystem bed every read_tools vector was "
                "measured against. A reimplementation reconstructs the bed and "
                "asserts this manifest BEFORE asserting any vector -- otherwise a "
                "bed mismatch is indistinguishable from an implementation bug, and "
                "the cheapest escape is to fit the bed to the implementation."
            ),
            "root_relative": True,
            "entries": bed_manifest,
        },
        "prompt_assembly": build_prompt_vectors(),
        "tool_surface": build_tool_surface(),
        "sol_execution": build_sol_execution(),
        "read_tools": read_tool_rows,
        "read_tool_limits": {
            "read_file_max_lines": crt.READ_FILE_MAX_LINES,
            "read_file_max_bytes": crt.READ_FILE_MAX_BYTES,
            "list_directory_max_entries": crt.LIST_DIRECTORY_MAX_ENTRIES,
            "glob_max_matches": crt.GLOB_MAX_MATCHES,
            "grep_max_matches": crt.GREP_MAX_MATCHES,
            "grep_max_files": crt.GREP_MAX_FILES,
            "grep_max_bytes_per_file": crt.GREP_MAX_BYTES_PER_FILE,
            "default_read_call_budget": crt.DEFAULT_READ_CALL_BUDGET,
            "denied_path_components": sorted(crt.DENIED_PATH_COMPONENTS),
            "denied_credential_patterns": list(crt.DENIED_CREDENTIAL_PATTERNS),
            "citation": "cogitate_read_tools.py:27-61",
        },
        "refusal_strings": {
            name: getattr(crt, name)
            for name in sorted(
                n for n in dir(crt) if n.startswith(("REFUSAL_", "NOTICE_"))
            )
        },
    }

    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(
        json.dumps(doc, indent=2, sort_keys=False) + "\n", encoding="utf-8"
    )

    counts = {
        "policy_commands": len(doc["policy_commands"]),
        "policy_check": len(doc["policy_check"]),
        "read_scope": len(doc["read_scope"]),
        "expects_emit_final": len(doc["expects_emit_final"]),
        "failure_caps": len(doc["failure_caps"]),
        "read_tools": len(doc["read_tools"]),
        "prompt_assembly": len(doc["prompt_assembly"]),
        "sol_execution": (
            len(doc["sol_execution"]["format_shell_output"])
            + len(doc["sol_execution"]["truncate_output"])
            + len(doc["sol_execution"]["run_command"])
        ),
    }
    total = sum(counts.values())
    print(f"wrote {args.out} ({args.out.stat().st_size} bytes)")
    for key, value in counts.items():
        print(f"  {key}: {value}")
    print(f"  TOTAL VECTORS: {total}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
