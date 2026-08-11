# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

from pathlib import Path

import pytest

from solstone.think import cogitate_policy


def _policy(
    tmp_path,
    access_tier: str = "normal",
    outbound_approval: str | None = None,
):
    return cogitate_policy.CogitatePolicy(
        allowed_roots=[tmp_path],
        access_tier=access_tier,
        outbound_approval=outbound_approval,
    )


def test_resolve_read_scope_defaults_to_current_day_chronicle():
    assert cogitate_policy.resolve_read_scope({}, "20260427") == ["chronicle/20260427"]


def test_resolve_read_scope_expands_override_placeholders():
    assert cogitate_policy.resolve_read_scope(
        {"read_scope": ["chronicle/<day>", "chronicle/<day-2>", "facets"]},
        "20260427",
    ) == ["chronicle/20260427", "chronicle/20260425", "facets"]


def test_resolve_read_scope_span_is_inclusive():
    assert cogitate_policy.resolve_read_scope(
        {"read_scope_span": 2},
        "20260427",
    ) == ["chronicle/20260425", "chronicle/20260426", "chronicle/20260427"]


def test_policy_denies_write_tools(tmp_path):
    policy = _policy(tmp_path)

    allowed, reason = policy.check("write_file", {"file_path": "x"})

    assert allowed is False
    assert reason.startswith("policy_deny:")


@pytest.mark.parametrize(
    "command",
    [
        "journal identity partner",
        "journal health logs --since 1h",
        "journal talent logs --daily -c 10",
    ],
)
def test_policy_allows_approved_journal_invocations(tmp_path, command):
    policy = _policy(tmp_path)

    allowed, reason = policy.check("run_shell_command", {"command": command})

    assert allowed is True
    assert reason == "ok"


@pytest.mark.parametrize(
    ("command", "reason"),
    [
        (
            "sol call journal identity partner",
            "policy_deny: `sol call journal identity` is not a `sol call` verb; "
            "`identity` is an approved host command family — run it directly as "
            "`journal identity partner`",
        ),
        (
            "sol call journal identity partner --update-section 'work patterns' --value x",
            "policy_deny: `sol call journal identity` is not a `sol call` verb; "
            "`identity` is an approved host command family — run it directly as "
            "`journal identity partner --update-section 'work patterns' --value x`",
        ),
        (
            "sol call journal health",
            "policy_deny: `sol call journal health` is not a `sol call` verb; "
            "`health` is an approved host command family — run it directly as "
            "`journal health`",
        ),
        (
            "sol call journal talent logs",
            "policy_deny: `sol call journal talent` is not a `sol call` verb; "
            "`talent` is an approved host command family — run it directly as "
            "`journal talent logs`",
        ),
    ],
)
def test_policy_denies_hybrid_journal_invocations_with_repair(
    tmp_path, command, reason
):
    policy = _policy(tmp_path)

    allowed, actual_reason = policy.check("run_shell_command", {"command": command})

    assert allowed is False
    assert actual_reason == reason


@pytest.mark.parametrize(
    ("command", "reason"),
    [
        (
            'journal search "partner"',
            "policy_deny: `journal search` is not a host command; run "
            "`sol call journal search partner` instead",
        ),
        (
            "journal facet show work",
            "policy_deny: `journal facet` is not a host command; run "
            "`sol call journal facet show work` instead",
        ),
    ],
)
def test_policy_denies_bare_journal_search_and_facet_with_repair(
    tmp_path, command, reason
):
    policy = _policy(tmp_path)

    allowed, actual_reason = policy.check("run_shell_command", {"command": command})

    assert allowed is False
    assert actual_reason == reason


def test_policy_allows_sol_call_journal_search_with_hybrid_text(tmp_path):
    policy = _policy(tmp_path)

    allowed, reason = policy.check(
        "run_shell_command",
        {"command": 'sol call journal search "sol call journal identity"'},
    )

    assert allowed is True
    assert reason == "ok"


@pytest.mark.parametrize(
    "command",
    [
        "journal think --segment",
        "journal navigate --path /app/support",
        "journal supervisor status",
        "journal indexer --rescan-full",
        "journal identity ; rm -rf journal",
    ],
)
def test_policy_denies_unapproved_journal_invocations(tmp_path, command):
    policy = _policy(tmp_path)

    allowed, reason = policy.check("run_shell_command", {"command": command})

    assert allowed is False
    assert reason.startswith("policy_deny:")


@pytest.mark.parametrize(
    "command",
    [
        "journal identity ; rm -rf journal",
        "sol call journal search x > out",
        "sol call journal search x 2>&1",
        "sol call journal search x <(journal health)",
        "sol call journal search x\nsol call entities list",
        "sol call journal search 'unterminated",
    ],
)
def test_policy_denies_shell_composition(tmp_path, command):
    policy = _policy(tmp_path)

    allowed, reason = policy.check("run_shell_command", {"command": command})

    assert allowed is False
    assert reason == cogitate_policy.SHELL_COMPOSITION_DENY


@pytest.mark.parametrize(
    "command",
    [
        "bash -lc 'sol call journal search x'",
        "env sol call journal search x",
        "./sol call journal search x",
        "python -m solstone.think.sol_cli call journal search x",
    ],
)
def test_policy_denies_wrapped_or_nonliteral_sol_invocations(tmp_path, command):
    policy = _policy(tmp_path)

    allowed, reason = policy.check("run_shell_command", {"command": command})

    assert allowed is False
    assert reason == cogitate_policy.RESTRICTED_COMMAND_DENY


@pytest.mark.parametrize("access_tier", ["normal", "system-read"])
@pytest.mark.parametrize(
    ("verb", "arguments"),
    [
        ("create", "--subject x --description y"),
        ("reply", "--subject x --description y"),
        ("attach", "--subject x --description y"),
        ("feedback", "--subject x --description y"),
        ("close", "7"),
        ("resolved", "7"),
        ("still-need-help", "7"),
    ],
)
def test_policy_denies_support_send_verbs_without_submit_tier(
    tmp_path, access_tier, verb, arguments
):
    policy = _policy(tmp_path, access_tier)

    allowed, reason = policy.check(
        "run_shell_command",
        {"command": f"sol call support {verb} {arguments}"},
    )

    assert allowed is False
    assert reason.startswith("policy_deny:")
    assert "outbound" in reason
    assert access_tier in reason


@pytest.mark.parametrize(
    ("verb", "arguments"),
    [
        ("create", "--subject x --description y"),
        ("reply", "--subject x --description y"),
        ("attach", "--subject x --description y"),
        ("feedback", "--subject x --description y"),
        ("close", "7"),
        ("resolved", "7"),
        ("still-need-help", "7"),
    ],
)
@pytest.mark.parametrize("outbound_approval", [None, ""])
def test_policy_denies_support_send_verbs_for_outbound_without_approval(
    tmp_path, verb, arguments, outbound_approval
):
    policy = _policy(tmp_path, "outbound", outbound_approval=outbound_approval)

    allowed, reason = policy.check(
        "run_shell_command",
        {"command": f"sol call support {verb} {arguments}"},
    )

    assert allowed is False
    assert "per-send owner approval" in reason


def test_policy_denies_outbound_support_send_with_yes_without_approval(tmp_path):
    policy = _policy(tmp_path, "outbound")

    allowed, reason = policy.check(
        "run_shell_command",
        {"command": ("sol call support feedback --body 'please fix this' --yes")},
    )

    assert allowed is False
    assert "per-send owner approval" in reason


@pytest.mark.parametrize(
    ("verb", "arguments"),
    [
        ("create", "--subject x --description y"),
        ("reply", "--subject x --description y"),
        ("attach", "--subject x --description y"),
        ("feedback", "--subject x --description y"),
        ("close", "7"),
        ("resolved", "7"),
        ("still-need-help", "7"),
    ],
)
def test_policy_allows_support_send_verbs_for_outbound_with_approval(
    tmp_path, verb, arguments
):
    policy = _policy(tmp_path, "outbound", outbound_approval="approval-token")

    allowed, reason = policy.check(
        "run_shell_command",
        {"command": f"sol call support {verb} {arguments}"},
    )

    assert allowed is True
    assert reason == "ok"


@pytest.mark.parametrize("access_tier", ["normal", "system-read", "outbound"])
@pytest.mark.parametrize(
    "command",
    [
        "sol call support register",
        "sol call support search foo",
        "sol call support article getting-started",
        "sol call support list",
        "sol call support show 42",
        "sol call support history",
        "sol call support announcements",
        "sol call support diagnose",
    ],
)
def test_policy_allows_support_read_verbs_for_all_tiers(tmp_path, access_tier, command):
    policy = _policy(tmp_path, access_tier)

    allowed, reason = policy.check("run_shell_command", {"command": command})

    assert allowed is True
    assert reason == "ok"


def test_policy_denies_chained_support_send_for_normal(tmp_path):
    policy = _policy(tmp_path, "normal")

    allowed, reason = policy.check(
        "run_shell_command",
        {
            "command": (
                "sol call support search foo && sol call support create "
                "--subject x --description y"
            )
        },
    )

    assert allowed is False
    assert reason == cogitate_policy.SHELL_COMPOSITION_DENY


@pytest.mark.parametrize(
    "command",
    [
        "echo $(sol call support create --subject x)",
        "sol call support search foo | grep bar",
    ],
)
def test_policy_denies_wrapped_or_chained_support_command_for_normal(tmp_path, command):
    policy = _policy(tmp_path, "normal")

    allowed, reason = policy.check("run_shell_command", {"command": command})

    assert allowed is False
    assert reason == cogitate_policy.SHELL_COMPOSITION_DENY


def test_policy_allows_non_support_chain_for_normal(tmp_path):
    policy = _policy(tmp_path, "normal")

    allowed, reason = policy.check(
        "run_shell_command",
        {"command": "sol call activities list && sol call entities list"},
    )

    assert allowed is False
    assert reason == cogitate_policy.SHELL_COMPOSITION_DENY


def test_policy_denies_chained_support_send_for_outbound_without_approval(tmp_path):
    policy = _policy(tmp_path, "outbound")

    allowed, reason = policy.check(
        "run_shell_command",
        {
            "command": (
                "sol call support search foo && sol call support create --subject x"
            )
        },
    )

    assert allowed is False
    assert reason == cogitate_policy.SHELL_COMPOSITION_DENY


def test_policy_allows_chained_support_send_for_outbound_with_approval(tmp_path):
    policy = _policy(tmp_path, "outbound", outbound_approval="approval-token")

    allowed, reason = policy.check(
        "run_shell_command",
        {
            "command": (
                "sol call support search foo && sol call support create --subject x"
            )
        },
    )

    assert allowed is False
    assert reason == cogitate_policy.SHELL_COMPOSITION_DENY


def test_cogitate_toml_removed_and_build_policy_import_fails():
    # AC 19: TOML policy generation is removed.
    policy_path = (
        Path(__file__).parents[1] / "solstone" / "think" / "policies" / "cogitate.toml"
    )
    assert not policy_path.exists()
    missing_symbol = "build" + "_per_task_policy"
    with pytest.raises(ImportError):
        exec(f"from solstone.think.cogitate_policy import {missing_symbol}", {})
