# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

import asyncio
import logging
from pathlib import Path

import pytest

from solstone.think import talent_provenance, talents


def _set_journal(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> Path:
    journal = tmp_path / "journal"
    journal.mkdir()
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal.resolve()))
    return journal


def _minimal_config(output_path: Path, *, day: str = "20260101") -> dict:
    return {
        "name": "daily",
        "type": "cogitate",
        "provider": "google",
        "model": "gemini-test",
        "output": "md",
        "output_path": str(output_path),
        "day": day,
        "schedule": "daily",
        "prompt": "prompt",
        "user_instruction": "",
        "system_instruction": "",
        "sources": {},
        "health_stale": False,
    }


def test_weekly_reflection_provenance_path(tmp_path, monkeypatch):
    journal = _set_journal(tmp_path, monkeypatch)
    output_path = journal / "reflections" / "weekly" / "20260622.md"

    assert talent_provenance._day_and_logical_output(output_path) == (
        "20260622",
        Path("reflections", "weekly", "20260622.md"),
    )
    assert talent_provenance.provenance_path_for_output(output_path) == (
        journal
        / "chronicle"
        / "20260622"
        / "health"
        / "talent-provenance"
        / "reflections"
        / "weekly"
        / "20260622.md.json"
    )


def test_facet_news_provenance_path(tmp_path, monkeypatch):
    journal = _set_journal(tmp_path, monkeypatch)
    output_path = journal / "facets" / "work" / "news" / "20260622.md"

    assert talent_provenance._day_and_logical_output(output_path) == (
        "20260622",
        Path("facets", "work", "news", "20260622.md"),
    )
    assert talent_provenance.provenance_path_for_output(output_path) == (
        journal
        / "chronicle"
        / "20260622"
        / "health"
        / "talent-provenance"
        / "facets"
        / "work"
        / "news"
        / "20260622.md.json"
    )


@pytest.mark.parametrize(
    "relative",
    [
        Path("reflections", "weekly", "draft.md"),
        Path("facets", "work", "news", "draft.md"),
    ],
)
def test_stem_day_requires_date_without_day_dir_side_effect(
    tmp_path,
    monkeypatch,
    relative,
):
    journal = _set_journal(tmp_path, monkeypatch)

    with pytest.raises(talent_provenance.UnsupportedProvenancePath):
        talent_provenance.provenance_path_for_output(journal / relative)

    assert not (journal / "chronicle" / "draft").exists()


def test_existing_chronicle_mapping_unchanged(tmp_path, monkeypatch):
    journal = _set_journal(tmp_path, monkeypatch)
    output_path = journal / "chronicle" / "20260101" / "talents" / "flow.md"

    assert talent_provenance._day_and_logical_output(output_path) == (
        "20260101",
        Path("talents", "flow.md"),
    )


def test_existing_activity_mapping_unchanged(tmp_path, monkeypatch):
    journal = _set_journal(tmp_path, monkeypatch)
    output_path = (
        journal / "facets" / "work" / "activities" / "20260101" / "abc" / "summary.md"
    )

    assert talent_provenance._day_and_logical_output(output_path) == (
        "20260101",
        Path("facets", "work", "activities", "abc", "summary.md"),
    )


def test_write_clean_provenance_skips_unmapped_path_without_error(
    tmp_path,
    monkeypatch,
    caplog,
):
    journal = _set_journal(tmp_path, monkeypatch)
    output_path = journal / "apps" / "chat" / "talents" / "support.md"
    output_path.parent.mkdir(parents=True)
    output_path.write_text("support", encoding="utf-8")
    caplog.set_level(logging.WARNING, logger="solstone.think.talents")

    talents._write_clean_provenance(
        _minimal_config(output_path),
        output_path,
        "support",
        None,
        123,
    )

    assert any(record.levelno == logging.WARNING for record in caplog.records)
    assert not any(record.levelno >= logging.ERROR for record in caplog.records)


def test_write_clean_provenance_logs_unexpected_write_failure(
    tmp_path,
    monkeypatch,
    caplog,
):
    journal = _set_journal(tmp_path, monkeypatch)
    output_path = journal / "chronicle" / "20260101" / "talents" / "daily.md"
    output_path.parent.mkdir(parents=True)
    output_path.write_text("daily", encoding="utf-8")

    def raise_os_error(*args, **kwargs):
        raise OSError("disk full")

    monkeypatch.setattr(talents, "write_provenance", raise_os_error)
    caplog.set_level(logging.WARNING, logger="solstone.think.talents")

    talents._write_clean_provenance(
        _minimal_config(output_path),
        output_path,
        "daily",
        None,
        123,
    )

    assert any(record.levelno == logging.ERROR for record in caplog.records)
    assert not any(record.levelno == logging.WARNING for record in caplog.records)


def test_execute_with_tools_weekly_reflection_finish_writes_sidecar(
    tmp_path,
    monkeypatch,
):
    journal = _set_journal(tmp_path, monkeypatch)
    output_path = journal / "reflections" / "weekly" / "20260622.md"
    result = "Full reflection text."
    events: list[dict] = []

    async def run_cogitate(config, on_event):
        on_event({"event": "finish", "result": result})
        return ""

    monkeypatch.setattr("solstone.think.cogitate_client.run_cogitate", run_cogitate)

    config = _minimal_config(output_path, day="20260622")
    asyncio.run(talents._execute_with_tools(config, events.append))

    assert any(event.get("event") == "finish" for event in events)
    assert not any(event.get("event") == "error" for event in events)
    assert output_path.read_text(encoding="utf-8") == result
    assert (
        journal
        / "chronicle"
        / "20260622"
        / "health"
        / "talent-provenance"
        / "reflections"
        / "weekly"
        / "20260622.md.json"
    ).exists()


def test_execute_with_tools_unmapped_output_still_finishes(
    tmp_path,
    monkeypatch,
):
    journal = _set_journal(tmp_path, monkeypatch)
    output_path = journal / "apps" / "chat" / "talents" / "support.md"
    result = "Support response."
    events: list[dict] = []

    async def run_cogitate(config, on_event):
        on_event({"event": "finish", "result": result})
        return ""

    monkeypatch.setattr("solstone.think.cogitate_client.run_cogitate", run_cogitate)

    config = _minimal_config(output_path, day="20260622")
    asyncio.run(talents._execute_with_tools(config, events.append))

    assert any(event.get("event") == "finish" for event in events)
    assert not any(event.get("event") == "error" for event in events)
    assert output_path.read_text(encoding="utf-8") == result
    assert not any((journal / "chronicle").glob("*/health/talent-provenance/**/*.json"))
