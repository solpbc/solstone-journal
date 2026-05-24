# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from pathlib import Path


def _briefing_prompt() -> str:
    return Path("solstone/talent/morning_briefing.md").read_text(encoding="utf-8")


def test_morning_briefing_reads_steward_health_surface():
    prompt = _briefing_prompt()

    assert "`sol call identity health`" in prompt
    assert "`sol call health pipeline --yesterday`" not in prompt


def test_morning_briefing_omits_migrated_pipeline_phrasings():
    prompt = _briefing_prompt()

    assert "Pipeline gap:" not in prompt
    assert "Pipeline issue:" not in prompt
    assert "steward health surface unavailable" in prompt
