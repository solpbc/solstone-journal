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

FIXTURE_NAME = "solstone-cogitate-oracle"
FIXTURE_VERSION = 1

# ---------------------------------------------------------------------------
# A. the command policy gate
# ---------------------------------------------------------------------------

# (case_id, command, access_tier, outbound_approval, citation, why)
POLICY_CASES: list[tuple[str, str, str, str | None, str, str]] = [
    # --- the happy paths
    ("sol_bare", "sol", "normal", None,
     "cogitate_policy.py:233-241", "a bare `sol` is admitted"),
    ("sol_call", "sol call activities list", "normal", None,
     "cogitate_policy.py:233-241", "the documented `sol call` form"),
    ("sol_call_quoted_arg", "sol call entities search --name 'Ada Lovelace'", "normal", None,
     "cogitate_policy.py:212", "shlex splits quoted data, quotes are not operators"),
    ("sol_call_double_quoted", 'sol call entities search --name "Ada Lovelace"', "normal", None,
     "cogitate_policy.py:150-152", "double quotes are data"),

    # --- approved direct host families (the closed set)
    ("journal_identity", "journal identity partner", "normal", None,
     "cogitate_policy.py:236-240", "approved host family"),
    ("journal_health", "journal health", "normal", None,
     "cogitate_policy.py:236-240", "approved host family"),
    ("journal_talent", "journal talent inventory", "normal", None,
     "cogitate_policy.py:236-240", "approved host family"),
    ("journal_bare", "journal", "normal", None,
     "cogitate_policy.py:236-240", "bare `journal` has no family -> restricted"),
    ("journal_unapproved_family", "journal doctor", "normal", None,
     "cogitate_policy.py:233-241", "an unapproved family is restricted, not repaired"),

    # --- the two repair denials (they carry a *suggested* command, so the text is contract)
    ("hybrid_identity", "sol call journal identity partner", "normal", None,
     "cogitate_policy.py:219-224", "hybrid form is refused WITH a repair"),
    ("hybrid_health", "sol call journal health --json", "normal", None,
     "cogitate_policy.py:219-224", "hybrid form, flags survive into the repair"),
    ("hybrid_talent", "sol call journal talent show read", "normal", None,
     "cogitate_policy.py:219-224", "hybrid form"),
    ("hybrid_not_a_family", "sol call journal search foo", "normal", None,
     "cogitate_policy.py:219-224", "`search` is NOT in COGITATE_JOURNAL_COMMANDS -> falls through to allowed"),
    ("bare_journal_search", "journal search sunlight", "normal", None,
     "cogitate_policy.py:226-231", "bare `journal search` -> repair to `sol call journal search`"),
    ("bare_journal_facet", "journal facet list", "normal", None,
     "cogitate_policy.py:226-231", "bare `journal facet` -> repair"),
    ("bare_journal_facet_quoting", "journal facet set 'my facet'", "normal", None,
     "cogitate_policy.py:167-170", "shlex.join re-quotes the repair suggestion"),

    # --- restricted commands
    ("restricted_cat", "cat /etc/passwd", "normal", None,
     "cogitate_policy.py:233-241", "not sol, not journal"),
    ("restricted_ls", "ls", "normal", None,
     "cogitate_policy.py:233-241", "not sol, not journal"),
    ("restricted_python", "python -c 'print(1)'", "normal", None,
     "cogitate_policy.py:233-241", "not sol, not journal"),
    ("restricted_solstone_core", "solstone-core generate", "normal", None,
     "cogitate_policy.py:233-241", "the native binary is NOT reachable from a talent"),

    # --- shell composition (each operator, and each quoting posture)
    ("shell_pipe", "sol call activities list | head", "normal", None,
     "cogitate_policy.py:129-158", "pipe outside quotes"),
    ("shell_semicolon", "sol call activities list; sol call entities list", "normal", None,
     "cogitate_policy.py:129-158", "chaining"),
    ("shell_and", "sol a && sol b", "normal", None,
     "cogitate_policy.py:129-158", "chaining"),
    ("shell_redirect_out", "sol call activities list > /tmp/x", "normal", None,
     "cogitate_policy.py:129-158", "redirect"),
    ("shell_redirect_in", "sol call activities list < /tmp/x", "normal", None,
     "cogitate_policy.py:129-158", "redirect"),
    ("shell_subshell", "(sol call activities list)", "normal", None,
     "cogitate_policy.py:129-158", "subshell"),
    ("shell_dollar_paren", "sol call entities show $(whoami)", "normal", None,
     "cogitate_policy.py:130-131", "command substitution, checked before the scanner"),
    ("shell_backtick", "sol call entities show `whoami`", "normal", None,
     "cogitate_policy.py:130-131", "backtick substitution"),
    ("shell_newline", "sol call activities list\nsol call entities list", "normal", None,
     "cogitate_policy.py:132-133", "newline is a composition character"),
    ("shell_carriage_return", "sol call activities list\rsol call entities list", "normal", None,
     "cogitate_policy.py:132-133", "a lone CR is refused too"),
    ("shell_operator_in_single_quotes", "sol call entities search --name 'a|b'", "normal", None,
     "cogitate_policy.py:140-142", "an operator inside single quotes is DATA"),
    ("shell_operator_in_double_quotes", 'sol call entities search --name "a|b"', "normal", None,
     "cogitate_policy.py:143-149", "an operator inside double quotes is DATA"),
    ("shell_dollar_paren_in_single_quotes", "sol call entities search --name '$(x)'", "normal", None,
     "cogitate_policy.py:130-131", "the $( check runs BEFORE quote tracking - refused even when quoted"),
    ("shell_backslash_escaped_pipe", "sol call entities search --name a\\|b", "normal", None,
     "cogitate_policy.py:151-153", "a backslash escape skips two characters"),
    ("shell_unterminated_single_quote", "sol call entities search --name 'oops", "normal", None,
     "cogitate_policy.py:158", "an unterminated quote is a syntax violation"),
    ("shell_unterminated_double_quote", 'sol call entities search --name "oops', "normal", None,
     "cogitate_policy.py:158", "an unterminated quote is a syntax violation"),
    ("empty_command", "", "normal", None,
     "cogitate_policy.py:216-217", "empty is its own refusal, distinct from restricted"),
    ("whitespace_only_command", "   ", "normal", None,
     "cogitate_policy.py:216-217", "whitespace-only splits to an empty argv"),

    # --- outbound submit gating: 7 verbs x tier x approval
    *[
        (f"support_{verb}_normal_noapproval", f"sol call support {verb} --body x", "normal", None,
         "cogitate_policy.py:243-251", "submit verb denied for a non-submit tier")
        for verb in ("create", "reply", "attach", "feedback", "close", "resolved", "still-need-help")
    ],
    ("support_create_outbound_noapproval", "sol call support create --body x", "outbound", None,
     "cogitate_policy.py:253-259", "submit tier without a per-send approval"),
    ("support_create_outbound_approved", "sol call support create --body x", "outbound", "owner-token",
     "cogitate_policy.py:243-261", "submit tier WITH approval"),
    ("support_create_synthesis", "sol call support create --body x", "synthesis", "owner-token",
     "cogitate_policy.py:243-251", "synthesis has sol but not submit; approval does not help"),
    ("support_create_system_read", "sol call support create --body x", "system-read", "owner-token",
     "cogitate_policy.py:243-251", "system-read has sol but not submit"),
    ("support_read_verb_normal", "sol call support list", "normal", None,
     "cogitate_policy.py:264-271", "a non-send support verb is not gated"),
    # the scan-anywhere subtlety: _support_send_verb searches EVERY index, not just 0..2
    ("support_send_verb_not_at_head", "sol call entities note sol call support create", "normal", None,
     "cogitate_policy.py:265-266", "the send-verb scan walks every offset, not just the command head"),
    ("support_send_verb_trailing_short", "sol call support", "normal", None,
     "cogitate_policy.py:265-266", "range(len(argv)-3) means a 3-token argv is never scanned"),
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
    ("check_write_file", "write_file", {"path": "x", "content": "y"}, "normal",
     "cogitate_policy.py:195-196"),
    ("check_replace", "replace", {"path": "x"}, "normal", "cogitate_policy.py:195-196"),
    ("check_read_file", "read_file", {"path": "x"}, "normal", "cogitate_policy.py:202-203"),
    ("check_glob", "glob", {"pattern": "*"}, "normal", "cogitate_policy.py:202-203"),
    ("check_emit_final", "emit_final", {"content": "x"}, "normal", "cogitate_policy.py:205"),
    ("check_finish", "finish", {}, "normal", "cogitate_policy.py:205"),
    ("check_unknown_tool", "browser", {}, "normal",
     "cogitate_policy.py:205 - an UNKNOWN tool defaults to ALLOWED"),
    ("check_run_shell_allowed", "run_shell_command", {"command": "sol call activities list"},
     "normal", "cogitate_policy.py:198-200"),
    ("check_run_shell_denied", "run_shell_command", {"command": "rm -rf /"},
     "normal", "cogitate_policy.py:198-200"),
    ("check_run_shell_missing_arg", "run_shell_command", {}, "normal",
     "cogitate_policy.py:199 - a missing command arg becomes the empty string"),
    ("check_write_file_outbound", "write_file", {}, "outbound",
     "cogitate_policy.py:195-196 - write denial is tier-independent"),
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
    ("scope_config_span_3", {"read_scope_span": 3}, "20260809", 0, "cogitate_policy.py:308-316"),
    ("scope_config_span_beats_arg", {"read_scope_span": 1}, "20260809", 5,
     "cogitate_policy.py:308 - the config value wins over the argument"),
    ("scope_literal", {"read_scope": ["facets/work"]}, "20260809", 0, "cogitate_policy.py:302-307"),
    ("scope_day_placeholder", {"read_scope": ["chronicle/<day>"]}, "20260809", 0,
     "cogitate_policy.py:286-293"),
    ("scope_day_minus_1", {"read_scope": ["chronicle/<day-1>"]}, "20260809", 0,
     "cogitate_policy.py:286-293"),
    ("scope_day_minus_7_month_boundary", {"read_scope": ["chronicle/<day-7>"]}, "20260803", 0,
     "cogitate_policy.py:286-293 - crosses a month boundary"),
    ("scope_day_minus_1_year_boundary", {"read_scope": ["chronicle/<day-1>"]}, "20260101", 0,
     "cogitate_policy.py:286-293 - crosses a year boundary"),
    ("scope_leap_day", {"read_scope": ["chronicle/<day-1>"]}, "20260301", 0,
     "cogitate_policy.py:286-293 - 2026 is not a leap year"),
    ("scope_multiple_placeholders", {"read_scope": ["chronicle/<day>", "chronicle/<day-2>"]},
     "20260809", 0, "cogitate_policy.py:302-307"),
    ("scope_repeated_placeholder_one_string",
     {"read_scope": ["<day>/a/<day-1>"]}, "20260809", 0,
     "cogitate_policy.py:293 - re.sub replaces every occurrence"),
    ("scope_span_zero_explicit", {"read_scope_span": 0}, "20260809", 0,
     "cogitate_policy.py:309-310"),
    ("scope_span_negative", {"read_scope_span": -3}, "20260809", 0,
     "cogitate_policy.py:309-310 - a negative span collapses to the single day"),
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
    ("final_diagnostic_truthy_not_true", {"diagnostic": 1}, "cogitate_contract.py:112 - `is True`, so 1 does NOT match"),
    ("final_output_path", {"output_path": "/j/talents/x.md"}, "cogitate_contract.py:113"),
    ("final_output_path_empty", {"output_path": ""}, "cogitate_contract.py:113"),
    ("final_schedule_daily", {"schedule": "daily"}, "cogitate_contract.py:114"),
    ("final_schedule_weekly", {"schedule": "weekly"}, "cogitate_contract.py:114"),
    ("final_schedule_activity", {"schedule": "activity"}, "cogitate_contract.py:114"),
    ("final_schedule_segment", {"schedule": "segment"}, "cogitate_contract.py:114 - segment is NOT in the set"),
    ("final_schedule_cadence", {"schedule": "cadence"}, "cogitate_contract.py:114 - cadence is NOT in the set"),
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
    unknown: dict[str, Any]
    try:
        capabilities_for_access_tier("code-agent")
        unknown = {"raises": False}
    except ValueError as exc:
        unknown = {"raises": True, "error": str(exc)}
    return {
        "tiers": list(COGITATE_ACCESS_TIERS),
        "talent_tiers": list(TALENT_ACCESS_TIERS),
        "future_tiers": list(FUTURE_ACCESS_TIERS),
        "capabilities": caps,
        "unknown_tier": unknown,
        "citation": "cogitate_contract.py:79-107",
    }


# ---------------------------------------------------------------------------
# F. the raw-read tier, executed against a deterministic bed
# ---------------------------------------------------------------------------

def build_bed(root: Path) -> None:
    """A deterministic journal bed. Content is fixed so byte counts are stable."""
    (root / "chronicle" / "20260809" / "default" / "120000_60").mkdir(parents=True)
    (root / "chronicle" / "20260809" / "default" / "120000_60" / "audio.jsonl").write_text(
        '{"start":0,"text":"hello"}\n{"start":1,"text":"world"}\n', encoding="utf-8"
    )
    (root / "chronicle" / "20260809" / "default" / "120000_60" / "notes.md").write_text(
        "line one\nline two\nline three\n", encoding="utf-8"
    )
    (root / "facets").mkdir()
    (root / "facets" / "work.md").write_text("work facet\nsunlight here\n", encoding="utf-8")
    (root / "talents").mkdir()
    (root / "talents" / "partner").mkdir()
    (root / "talents" / "partner" / "abc.jsonl").write_text('{"event":"finish"}\n', encoding="utf-8")

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
        "id_rsa", "id_rsa.pub", ".env", ".env.local", "server.key", "key.pem",
        "my.credentials", "credentials", "credentials.json", "token.key",
        "api_secret.txt", "token.txt", "passwords.md", "secrets.yaml",
    ):
        (root / "secrets" / name).write_text("SECRET\n", encoding="utf-8")

    # a binary file and a special file
    (root / "blob.bin").write_bytes(bytes(range(256)) * 4)
    (root / "empty.txt").write_text("", encoding="utf-8")
    try:
        os.mkfifo(root / "fifo")
    except (OSError, AttributeError):
        pass

    # symlink escape + in-tree symlink
    outside = root.parent / "outside"
    outside.mkdir(exist_ok=True)
    (outside / "leak.txt").write_text("LEAKED\n", encoding="utf-8")
    try:
        (root / "escape").symlink_to(outside / "leak.txt")
        (root / "inside_link").symlink_to(root / "facets" / "work.md")
    except OSError:
        pass

    # a hidden file, and a large file for truncation
    (root / ".hidden.md").write_text("hidden\n", encoding="utf-8")
    (root / "big.txt").write_text("".join(f"row {i}\n" for i in range(3000)), encoding="utf-8")

    # a directory with many entries for listing truncation
    many = root / "many"
    many.mkdir()
    for i in range(250):
        (many / f"f{i:03d}.txt").write_text("x\n", encoding="utf-8")


def _serialize(value: Any) -> Any:
    if is_dataclass(value) and not isinstance(value, type):
        return {k: _serialize(v) for k, v in asdict(value).items()}
    if isinstance(value, (list, tuple)):
        return [_serialize(v) for v in value]
    if isinstance(value, dict):
        return {k: _serialize(v) for k, v in value.items()}
    return value


def _result_row(case_id: str, tool: str, kwargs: dict[str, Any], citation: str,
                result: crt.ReadResult) -> dict[str, Any]:
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
        rows.append(_result_row(case_id, "read_file", kw, citation, crt.read_file(j, **kw)))

    def ld(case_id, citation, **kw):
        rows.append(_result_row(case_id, "list_directory", kw, citation, crt.list_directory(j, **kw)))

    def gl(case_id, citation, **kw):
        rows.append(_result_row(case_id, "glob", kw, citation, crt.glob(j, **kw)))

    def gs(case_id, citation, **kw):
        rows.append(_result_row(case_id, "grep_search", kw, citation, crt.grep_search(j, **kw)))

    # --- read_file
    rf("rf_ok", "cogitate_read_tools.py:426", path="facets/work.md")
    rf("rf_nested", "cogitate_read_tools.py:426",
       path="chronicle/20260809/default/120000_60/notes.md")
    rf("rf_date_prefixed", "cogitate_read_tools.py:437-439 - an 8-digit head resolves under chronicle/",
       path="20260809/default/120000_60/notes.md")
    rf("rf_start_line", "cogitate_read_tools.py:426", path="facets/work.md", start_line=2)
    rf("rf_start_line_past_end", "cogitate_read_tools.py:426", path="facets/work.md", start_line=99)
    rf("rf_max_lines", "cogitate_read_tools.py:426", path="big.txt", max_lines=3)
    rf("rf_truncated_by_lines", "cogitate_read_tools.py:118-121", path="big.txt")
    rf("rf_truncated_by_bytes", "cogitate_read_tools.py:118-121", path="big.txt", max_bytes=64)
    rf("rf_empty_file", "cogitate_read_tools.py:426", path="empty.txt")
    rf("rf_missing", "cogitate_read_tools.py:92-95", path="nope.md")
    rf("rf_directory", "cogitate_read_tools.py:80-83", path="facets")
    rf("rf_binary", "cogitate_read_tools.py:84-87", path="blob.bin")
    rf("rf_fifo", "cogitate_read_tools.py:88-91", path="fifo")
    rf("rf_denied_git", "cogitate_read_tools.py:72-75", path=".git/config")
    rf("rf_denied_cache", "cogitate_read_tools.py:72-75", path=".cache/x.txt")
    rf("rf_denied_node_modules", "cogitate_read_tools.py:72-75", path="node_modules/pkg.json")
    rf("rf_denied_venv", "cogitate_read_tools.py:72-75", path=".venv/pyvenv.cfg")
    # The preamble tells the model the denylist covers "credentials". These pin what
    # DENIED_CREDENTIAL_PATTERNS actually matches - including the names it does not.
    for name in (
        "id_rsa", "id_rsa.pub", ".env", ".env.local", "server.key", "key.pem",
        "my.credentials", "credentials", "credentials.json", "token.key",
        "api_secret.txt", "token.txt", "passwords.md", "secrets.yaml",
    ):
        rf(f"rf_credential_{name.strip('.')}", "cogitate_read_tools.py:53-61,76-79",
           path=f"secrets/{name}")
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
    ld("ld_recursive_chronicle", "cogitate_read_tools.py:509",
       path="chronicle", recursive=True)
    ld("ld_recursive_root_broad", "cogitate_read_tools.py:271-279 - broad recursive roots are refused",
       path=".", recursive=True)
    ld("ld_chronicle_nonrecursive_ok", "cogitate_read_tools.py:271-279 - the broad-root rule is about RECURSION",
       path="chronicle", recursive=False)
    ld("ld_recursive_facets_broad", "cogitate_read_tools.py:271 - facets/ is the third broad root",
       path="facets", recursive=True)
    ld("ld_recursive_day_ok", "cogitate_read_tools.py:271-279 - one level down is not broad",
       path="chronicle/20260809", recursive=True)
    ld("ld_recursive_talents_ok", "cogitate_read_tools.py:271 - talents/ is NOT a broad root",
       path="talents", recursive=True)
    ld("ld_truncated", "cogitate_read_tools.py:122-125", path="many")
    ld("ld_max_entries", "cogitate_read_tools.py:122-125", path="many", max_entries=5)
    ld("ld_pattern", "cogitate_read_tools.py:509", path="many", pattern="f00*.txt")
    ld("ld_denied", "cogitate_read_tools.py:72-75", path=".git")
    ld("ld_missing", "cogitate_read_tools.py:92-95", path="nope")
    ld("ld_on_a_file", "cogitate_read_tools.py:80-83", path="facets/work.md")

    # --- glob
    gl("gl_md", "cogitate_read_tools.py:586", pattern="*.md")
    gl("gl_crosses_slash", "cogitate_read_tools.py:596-598 - fnmatch `*` spans `/`", pattern="*.jsonl")
    gl("gl_rooted", "cogitate_read_tools.py:586", pattern="*.jsonl", root="talents")
    gl("gl_no_match", "cogitate_read_tools.py:586", pattern="*.zzz")
    gl("gl_truncated", "cogitate_read_tools.py:126-128", pattern="many/*.txt", max_matches=5)
    gl("gl_denied_root", "cogitate_read_tools.py:72-75", pattern="*", root=".git")
    gl("gl_hidden_excluded", "cogitate_read_tools.py:335-337", pattern="*.md")
    gl("gl_hidden_included", "cogitate_read_tools.py:335-337", pattern="*.md", include_hidden=True)
    gl("gl_would_match_credential", "cogitate_read_tools.py:76-79", pattern="secrets/*")
    # glob is inherently recursive, so its DEFAULT root refuses. Pin where it works.
    gl("gl_deep_root_ok", "cogitate_read_tools.py:271-279 - a non-broad root admits glob",
       pattern="*.jsonl", root="chronicle/20260809")
    gl("gl_talents_root_ok", "cogitate_read_tools.py:271", pattern="*", root="talents")
    gl("gl_secrets_root", "cogitate_read_tools.py:76-79 - rooted at secrets/, which credential names survive?",
       pattern="*", root="secrets")

    # --- grep_search: same broad-root rule, pinned in both directions
    gs("gs_deep_ok", "cogitate_read_tools.py:271-279 - a non-broad path admits grep",
       pattern="hello", path="chronicle/20260809")
    gs("gs_talents_ok", "cogitate_read_tools.py:271", pattern="finish", path="talents")
    gs("gs_facets_broad", "cogitate_read_tools.py:271 - facets/ is a broad root for grep too",
       pattern="sunlight", path="facets")
    gs("gs_single_file_path", "cogitate_read_tools.py:698 - a file path is not a broad root",
       pattern="sunlight", path="facets/work.md")

    # --- grep_search
    gs("gs_literal", "cogitate_read_tools.py:698", pattern="sunlight")
    gs("gs_case_insensitive_default", "cogitate_read_tools.py:719", pattern="SUNLIGHT")
    gs("gs_case_sensitive", "cogitate_read_tools.py:719", pattern="SUNLIGHT", case_sensitive=True)
    gs("gs_regex", "cogitate_read_tools.py:720-723", pattern=r"line \w+", regex=True)
    gs("gs_regex_literal_escaped", "cogitate_read_tools.py:720-723", pattern="line .")
    gs("gs_bad_regex", "cogitate_read_tools.py:104-107", pattern="[unclosed", regex=True)
    gs("gs_context", "cogitate_read_tools.py:698", pattern="line two", context_lines=1)
    gs("gs_file_glob", "cogitate_read_tools.py:698", pattern="line", file_glob="*.md")
    gs("gs_scoped_path", "cogitate_read_tools.py:698", pattern="hello", path="chronicle")
    gs("gs_denied_path", "cogitate_read_tools.py:72-75", pattern="x", path=".git")
    gs("gs_no_match", "cogitate_read_tools.py:698", pattern="zzzznotpresent")
    gs("gs_max_matches", "cogitate_read_tools.py:129-132", pattern="row", max_matches=3)
    gs("gs_bytes_per_file", "cogitate_read_tools.py:698", pattern="row", max_bytes_per_file=64)

    # --- the shared budget
    budget = crt.ReadBudget(cap=2)
    rows.append(_result_row("budget_call_1", "read_file", {"path": "facets/work.md", "budget_cap": 2},
                            "cogitate_read_tools.py:218-222",
                            crt.read_file(j, "facets/work.md", budget=budget)))
    rows.append(_result_row("budget_call_2", "list_directory", {"path": "facets", "budget_cap": 2},
                            "cogitate_read_tools.py:218-222",
                            crt.list_directory(j, "facets", budget=budget)))
    rows.append(_result_row("budget_call_3_exhausted", "glob", {"pattern": "*", "budget_cap": 2},
                            "cogitate_read_tools.py:108-111",
                            crt.glob(j, "*", budget=budget)))
    return rows


# ---------------------------------------------------------------------------
# assemble
# ---------------------------------------------------------------------------

def reference_commit() -> str:
    try:
        return subprocess.run(
            ["git", "rev-parse", "HEAD"], cwd=REPO_ROOT, capture_output=True,
            text=True, check=True,
        ).stdout.strip()
    except Exception:
        return "unknown"


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--out", required=True, type=Path)
    args = parser.parse_args()

    tmp = Path(tempfile.mkdtemp(prefix="cogitate-oracle-"))
    try:
        bed = tmp / "journal"
        bed.mkdir()
        build_bed(bed)
        read_tool_rows = build_read_tool_vectors(bed)
    finally:
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
        "preambles": {
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
            "deterministic_failure_reason_codes": sorted(DETERMINISTIC_FAILURE_REASON_CODES),
            "deterministic_failure_caps": dict(sorted(DETERMINISTIC_FAILURE_CAPS.items())),
            "citation": "cogitate_contract.py:18,66,69 - cogitate_policy.py:41-75",
        },
        "access_tiers": build_capability_vectors(),
        "policy_commands": build_policy_vectors(),
        "policy_check": build_check_vectors(),
        "read_scope": build_read_scope_vectors(),
        "expects_emit_final": build_emit_final_vectors(),
        "failure_caps": build_failure_cap_vectors(),
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
            for name in sorted(n for n in dir(crt) if n.startswith(("REFUSAL_", "NOTICE_")))
        },
    }

    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(doc, indent=2, sort_keys=False) + "\n", encoding="utf-8")

    counts = {
        "policy_commands": len(doc["policy_commands"]),
        "policy_check": len(doc["policy_check"]),
        "read_scope": len(doc["read_scope"]),
        "expects_emit_final": len(doc["expects_emit_final"]),
        "failure_caps": len(doc["failure_caps"]),
        "read_tools": len(doc["read_tools"]),
    }
    total = sum(counts.values())
    print(f"wrote {args.out} ({args.out.stat().st_size} bytes)")
    for key, value in counts.items():
        print(f"  {key}: {value}")
    print(f"  TOTAL VECTORS: {total}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
