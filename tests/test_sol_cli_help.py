# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import pytest

from solstone.think import sol_cli

HELP_HEADINGS = [
    sol_cli.SOL_HELP_GROUP_CONVERSATION,
    sol_cli.SOL_HELP_GROUP_YOUR_JOURNAL,
    sol_cli.SOL_HELP_GROUP_DIAGNOSE,
    sol_cli.SOL_HELP_GROUP_TOOLS,
    sol_cli.SOL_HELP_GROUP_SERVICE_HEADING,
    sol_cli.SOL_HELP_GROUP_ALIASES,
]


def _assigned_groups(command_name: str) -> list[str]:
    return [
        group.heading
        for group in sol_cli.help_groups()
        if command_name in group.commands
    ]


@pytest.mark.parametrize("command_name", sorted(sol_cli.COMMANDS))
def test_sol_help_group_assignment_is_exact_partition(command_name: str) -> None:
    assigned = _assigned_groups(command_name)
    assert len(assigned) == 1, f"{command_name!r} appears in {assigned!r}"


def test_sol_help_groups_reference_only_registered_commands() -> None:
    grouped = []
    for group in sol_cli.help_groups():
        grouped.extend(group.commands)
        for command_name in group.commands:
            assert command_name in sol_cli.COMMANDS

    assert set(grouped) == set(sol_cli.COMMANDS)
    assert len(grouped) == len(set(grouped))


def test_sol_help_heading_order_and_apps_position(monkeypatch, capsys) -> None:
    monkeypatch.setattr(sol_cli, "print_status", lambda: None)

    sol_cli.print_help()
    lines = capsys.readouterr().out.splitlines()

    heading_positions = {heading: lines.index(heading) for heading in HELP_HEADINGS}
    assert list(heading_positions) == HELP_HEADINGS
    assert [heading_positions[heading] for heading in HELP_HEADINGS] == sorted(
        heading_positions.values()
    )

    apps_position = lines.index("Apps (sol call <app>):")
    assert heading_positions[sol_cli.SOL_HELP_GROUP_SERVICE_HEADING] < apps_position
    assert apps_position < heading_positions[sol_cli.SOL_HELP_GROUP_ALIASES]
    assert "Direct module syntax: sol <module.path> [args]" not in lines


def test_access_help_groups_match_canonical_membership() -> None:
    assert sol_cli.ACCESS_HELP_GROUPS == (
        sol_cli.HelpGroup(sol_cli.SOL_HELP_GROUP_CONVERSATION, ("chat", "engage")),
        sol_cli.HelpGroup(
            sol_cli.SOL_HELP_GROUP_YOUR_JOURNAL,
            ("call", "import", "journal-stats", "segment", "streams", "indexer"),
        ),
        sol_cli.HelpGroup(
            sol_cli.SOL_HELP_GROUP_DIAGNOSE, ("top", "health", "notify", "doctor")
        ),
        sol_cli.HelpGroup(
            sol_cli.SOL_HELP_GROUP_TOOLS,
            ("providers", "observer", "skills", "restart-convey", "link"),
        ),
    )
