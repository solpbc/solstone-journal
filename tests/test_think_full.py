# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Tests for the think module unified priority system."""

import importlib
import logging


def test_main_runs_with_mocked_prompts(journal_copy, monkeypatch):
    """Test that main() runs pre/post phases and prompts by priority."""
    mod = importlib.import_module("solstone.think.thinking")

    commands_run = []
    prompts_run = False

    def mock_run_command(cmd, day):
        commands_run.append(cmd)
        return True

    def mock_run_queued_command(cmd, day, timeout=600):
        commands_run.append(cmd)
        return True

    def mock_run_daily_prompts(day, verbose, **kwargs):
        nonlocal prompts_run
        prompts_run = True
        return (5, 0, [], set())  # 5 success, 0 failures, no failed names

    monkeypatch.setattr(mod, "run_command", mock_run_command)
    monkeypatch.setattr(mod, "run_queued_command", mock_run_queued_command)
    monkeypatch.setattr(mod, "run_daily_prompts", mock_run_daily_prompts)
    monkeypatch.setattr(
        "sys.argv",
        ["sol think", "--day", "20240101", "--refresh", "--verbose"],
    )

    mod.main()

    # Verify pre-phase: sense ran
    assert any(c[0] == "journal" and c[1] == "sense" for c in commands_run)

    # Verify main phase: prompts ran
    assert prompts_run, "run_daily_prompts should have been called"

    # Verify post-phase: indexer rescan ran
    indexer_cmds = [c for c in commands_run if c[0] == "journal" and c[1] == "indexer"]
    assert len(indexer_cmds) >= 1
    assert any("--rescan" in cmd for cmd in indexer_cmds)


def test_main_runs_segment_think_prephase_before_daily_synthesis(
    journal_copy,
    monkeypatch,
):
    """Test segment-think repair runs between sense repair and daily prompts."""
    mod = importlib.import_module("solstone.think.thinking")

    commands_run = []

    def mock_run_command(cmd, day):
        commands_run.append(cmd)
        return True

    def mock_run_queued_command(cmd, day, timeout=600):
        commands_run.append(cmd)
        return True

    def mock_run_daily_prompts(day, verbose, **kwargs):
        commands_run.append(["__daily_synthesis__"])
        return (5, 0, [], set())

    monkeypatch.setattr(mod, "run_command", mock_run_command)
    monkeypatch.setattr(mod, "run_queued_command", mock_run_queued_command)
    monkeypatch.setattr(mod, "run_daily_prompts", mock_run_daily_prompts)
    monkeypatch.setattr(
        "sys.argv",
        ["sol think", "--day", "20240101", "--refresh", "--verbose"],
    )

    mod.main()

    sense_index = next(
        i
        for i, cmd in enumerate(commands_run)
        if len(cmd) >= 2 and cmd[:2] == ["journal", "sense"]
    )
    segment_index = next(
        i
        for i, cmd in enumerate(commands_run)
        if len(cmd) >= 4 and cmd[:4] == ["journal", "think", "--segments", "--day"]
    )
    daily_index = commands_run.index(["__daily_synthesis__"])
    indexer_index = next(
        i
        for i, cmd in enumerate(commands_run)
        if len(cmd) >= 2 and cmd[:2] == ["journal", "indexer"] and "--rescan" in cmd
    )

    segment_cmd = commands_run[segment_index]
    assert "--refresh" not in segment_cmd
    assert "-v" in segment_cmd
    assert sense_index < segment_index < daily_index < indexer_index


def test_from_scratch_refreshes_segment_think_prephase(
    journal_copy,
    monkeypatch,
):
    """Test from-scratch daily think refreshes segment-think repair."""
    mod = importlib.import_module("solstone.think.thinking")

    commands_run = []

    def mock_run_command(cmd, day):
        commands_run.append(cmd)
        return True

    def mock_run_queued_command(cmd, day, timeout=600):
        commands_run.append(cmd)
        return True

    def mock_run_daily_prompts(day, verbose, **kwargs):
        commands_run.append(["__daily_synthesis__", kwargs])
        return (5, 0, [], set())

    monkeypatch.setattr(mod, "run_command", mock_run_command)
    monkeypatch.setattr(mod, "run_queued_command", mock_run_queued_command)
    monkeypatch.setattr(mod, "run_daily_prompts", mock_run_daily_prompts)
    monkeypatch.setattr(
        "sys.argv",
        ["sol think", "--day", "20240101", "--from-scratch", "--verbose"],
    )

    mod.main()

    segment_cmd = next(
        cmd
        for cmd in commands_run
        if len(cmd) >= 4 and cmd[:4] == ["journal", "think", "--segments", "--day"]
    )
    assert "--refresh" in segment_cmd
    assert "-v" in segment_cmd


def test_segment_think_prephase_failure_is_non_fatal(
    journal_copy,
    monkeypatch,
    caplog,
):
    """Test daily synthesis still runs when segment-think repair fails."""
    mod = importlib.import_module("solstone.think.thinking")

    commands_run = []

    def mock_run_command(cmd, day):
        commands_run.append(cmd)
        if len(cmd) >= 2 and cmd[:2] == ["journal", "think"] and "--segments" in cmd:
            return False
        return True

    def mock_run_queued_command(cmd, day, timeout=600):
        commands_run.append(cmd)
        return True

    def mock_run_daily_prompts(day, verbose, **kwargs):
        commands_run.append(["__daily_synthesis__"])
        return (5, 0, [], set())

    monkeypatch.setattr(mod, "run_command", mock_run_command)
    monkeypatch.setattr(mod, "run_queued_command", mock_run_queued_command)
    monkeypatch.setattr(mod, "run_daily_prompts", mock_run_daily_prompts)
    monkeypatch.setattr(
        "sys.argv",
        ["sol think", "--day", "20240101", "--refresh", "--verbose"],
    )

    caplog.set_level(logging.WARNING)
    mod.main()

    assert ["__daily_synthesis__"] in commands_run
    assert "Segment-think repair failed, continuing anyway" in caplog.text


def test_segment_mode_skips_pre_post_phases(journal_copy, monkeypatch):
    """Test that segment mode skips sense and journal-stats."""
    mod = importlib.import_module("solstone.think.thinking")

    # Create segment directory
    segment_dir = journal_copy / "chronicle" / "20240101" / "default" / "120000_300"
    segment_dir.mkdir(parents=True, exist_ok=True)

    commands_run = []

    def mock_run_command(cmd, day):
        commands_run.append(cmd)
        return True

    def mock_run_queued_command(cmd, day, timeout=600):
        commands_run.append(cmd)
        return True

    def mock_run_segment_sense(day, segment, refresh, verbose, **kwargs):
        return (1, 0, [])

    monkeypatch.setattr(mod, "run_command", mock_run_command)
    monkeypatch.setattr(mod, "run_queued_command", mock_run_queued_command)
    monkeypatch.setattr(mod, "run_segment_sense", mock_run_segment_sense)
    monkeypatch.setattr(
        "sys.argv",
        ["sol think", "--day", "20240101", "--segment", "120000_300"],
    )

    mod.main()

    # Segment mode should NOT run sense or journal-stats
    assert not any(c[1] == "sense" for c in commands_run if len(c) > 1)
    assert not any(c[1] == "journal-stats" for c in commands_run if len(c) > 1)


def test_priority_validation_required():
    """Test that get_talent_configs raises error for scheduled prompts without priority."""
    from solstone.think.talent import get_talent_configs

    # This test verifies the validation exists - actual validation tested in test_utils.py
    # Here we just confirm all existing scheduled prompts have priority
    configs = get_talent_configs(schedule="daily")
    for name, config in configs.items():
        assert "priority" in config, f"Scheduled prompt '{name}' missing priority"
