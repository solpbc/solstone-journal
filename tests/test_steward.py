# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

import json
import os
import time
from pathlib import Path

import pytest

from solstone.think.identity import (
    STEWARD_SECTION_ATTENTION,
    STEWARD_SECTION_AUTO_REPAIRS,
    STEWARD_SECTION_STATUS,
    STEWARD_SECTION_TRENDS,
    ensure_identity_directory,
)
from solstone.think.steward import (
    RecipeOutcome,
    StalePendingTarget,
    detect_stale_pending_segments,
    load_steward_log,
    read_steward_health,
    run_recipe_pass,
    validate_steward_health,
    write_health_md,
)
from solstone.think.utils import now_ms


def _set_journal(monkeypatch: pytest.MonkeyPatch, journal: Path) -> None:
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))


def _valid_body(*, status: str = "Sol is well.", needs: str = "") -> str:
    return "\n".join(
        [
            STEWARD_SECTION_STATUS,
            "<!-- generated_at: 2026-05-26T17:32:18Z -->",
            status,
            "",
            STEWARD_SECTION_ATTENTION,
            needs,
            "",
            STEWARD_SECTION_AUTO_REPAIRS,
            "",
            STEWARD_SECTION_TRENDS,
            "",
        ]
    )


def _seed_stale_pending_segment(
    journal: Path,
    day: str,
    stream: str,
    segment_key: str,
    modality: str,
    age_seconds: int,
) -> Path:
    segment_dir = journal / "chronicle" / day / stream / segment_key
    segment_dir.mkdir(parents=True, exist_ok=True)
    suffix = ".flac" if modality == "audio" else ".webm"
    raw_path = segment_dir / f"{segment_key}_{modality}{suffix}"
    raw_path.write_bytes(b"raw")
    mtime = time.time() - age_seconds
    os.utime(raw_path, (mtime, mtime))
    return segment_dir


def _seed_steward_log(journal: Path, rows: list[dict]) -> None:
    path = journal / "health" / "steward.log"
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(
        "".join(json.dumps(row, separators=(",", ":")) + "\n" for row in rows),
        encoding="utf-8",
    )


def _recipe_row(target: str, outcome: str, ts: int) -> dict:
    return {
        "event": "recipe.outcome",
        "ts": ts,
        "recipe": "stale_pending_segment_reprocess",
        "target": target,
        "outcome": outcome,
        "detail": None,
    }


def test_recipe_detects_stale_pending_segment(tmp_path, monkeypatch):
    _set_journal(monkeypatch, tmp_path)
    _seed_stale_pending_segment(
        tmp_path, "20260526", "archon", "120000_300", "audio", 7 * 60 * 60
    )

    targets = detect_stale_pending_segments("20260526", "20260525")

    assert [target.target for target in targets] == ["20260526/archon/120000_300:audio"]


def test_recipe_skips_fresh_pending_segment(tmp_path, monkeypatch):
    _set_journal(monkeypatch, tmp_path)
    _seed_stale_pending_segment(
        tmp_path, "20260526", "archon", "120000_300", "audio", 60
    )

    assert detect_stale_pending_segments("20260526", "20260525") == []


def test_recipe_skips_already_analyzing(tmp_path, monkeypatch):
    _set_journal(monkeypatch, tmp_path)
    segment_dir = _seed_stale_pending_segment(
        tmp_path, "20260526", "archon", "120000_300", "audio", 7 * 60 * 60
    )
    (segment_dir / ".analyzing_audio").write_text("{}", encoding="utf-8")

    assert detect_stale_pending_segments("20260526", "20260525") == []


def test_recipe_fire_success_appends_log_entry(tmp_path, monkeypatch):
    _set_journal(monkeypatch, tmp_path)
    _seed_stale_pending_segment(
        tmp_path, "20260526", "archon", "120000_300", "audio", 7 * 60 * 60
    )

    def fake_fire(target: StalePendingTarget, *, port: int) -> RecipeOutcome:
        return RecipeOutcome(
            recipe="stale_pending_segment_reprocess",
            target=target.target,
            outcome="success",
            detail=None,
            ts=now_ms(),
        )

    monkeypatch.setattr("solstone.think.steward.fire_stale_pending_recipe", fake_fire)

    result = run_recipe_pass("20260526")

    assert result["fired"][0].outcome == "success"
    assert load_steward_log()[0]["outcome"] == "success"


def test_recipe_fire_failure_appends_log_entry(tmp_path, monkeypatch):
    _set_journal(monkeypatch, tmp_path)
    _seed_stale_pending_segment(
        tmp_path, "20260526", "archon", "120000_300", "audio", 7 * 60 * 60
    )

    def fake_fire(target: StalePendingTarget, *, port: int) -> RecipeOutcome:
        return RecipeOutcome(
            recipe="stale_pending_segment_reprocess",
            target=target.target,
            outcome="failure",
            detail="500",
            ts=now_ms(),
        )

    monkeypatch.setattr("solstone.think.steward.fire_stale_pending_recipe", fake_fire)

    run_recipe_pass("20260526")

    row = load_steward_log()[0]
    assert row["outcome"] == "failure"
    assert row["detail"] == "500"


def test_escalation_after_two_consecutive_failures(tmp_path, monkeypatch):
    _set_journal(monkeypatch, tmp_path)
    target = "20260526/archon/120000_300:audio"
    _seed_stale_pending_segment(
        tmp_path, "20260526", "archon", "120000_300", "audio", 7 * 60 * 60
    )
    _seed_steward_log(
        tmp_path,
        [
            _recipe_row(target, "failure", now_ms() - 2000),
            _recipe_row(target, "failure", now_ms() - 1000),
        ],
    )
    calls = []
    monkeypatch.setattr(
        "solstone.think.steward.fire_stale_pending_recipe",
        lambda target, *, port: calls.append(target),
    )

    result = run_recipe_pass("20260526")

    assert result["escalated_targets"] == [target]
    assert calls == []


def test_escalation_resets_after_success(tmp_path, monkeypatch):
    _set_journal(monkeypatch, tmp_path)
    target = "20260526/archon/120000_300:audio"
    _seed_stale_pending_segment(
        tmp_path, "20260526", "archon", "120000_300", "audio", 7 * 60 * 60
    )
    _seed_steward_log(
        tmp_path,
        [
            _recipe_row(target, "failure", now_ms() - 3000),
            _recipe_row(target, "failure", now_ms() - 2000),
            _recipe_row(target, "success", now_ms() - 1000),
        ],
    )

    def fake_fire(target: StalePendingTarget, *, port: int) -> RecipeOutcome:
        return RecipeOutcome(
            recipe="stale_pending_segment_reprocess",
            target=target.target,
            outcome="success",
            detail=None,
            ts=now_ms(),
        )

    monkeypatch.setattr("solstone.think.steward.fire_stale_pending_recipe", fake_fire)

    result = run_recipe_pass("20260526")

    assert result["escalated_targets"] == []
    assert result["fired"][0].target == target


def test_validator_rejects_missing_section():
    body = _valid_body().replace(f"\n{STEWARD_SECTION_TRENDS}\n", "\n")

    assert validate_steward_health(body) == f"missing section: {STEWARD_SECTION_TRENDS}"


def test_validator_rejects_wrong_order():
    body = "\n".join(
        [
            STEWARD_SECTION_STATUS,
            "<!-- generated_at: 2026-05-26T17:32:18Z -->",
            "Sol is well.",
            "",
            STEWARD_SECTION_AUTO_REPAIRS,
            "",
            STEWARD_SECTION_ATTENTION,
            "",
            STEWARD_SECTION_TRENDS,
            "",
        ]
    )

    assert validate_steward_health(body) == "sections out of order"


def test_validator_rejects_extra_section():
    body = _valid_body() + "\n## Extra\n"

    assert validate_steward_health(body) == "unexpected section: ## Extra"


def test_validator_rejects_empty_status():
    body = _valid_body(status="")

    assert validate_steward_health(body) == "empty status section"


def test_validator_rejects_missing_generated_at():
    body = _valid_body().replace("<!-- generated_at: 2026-05-26T17:32:18Z -->\n", "")

    assert validate_steward_health(body) == "missing or invalid generated_at"


def test_validator_accepts_well_formed():
    assert validate_steward_health(_valid_body()) is None


def test_read_steward_health_returns_none_when_missing(tmp_path):
    assert read_steward_health(tmp_path) is None


def test_read_steward_health_returns_none_when_healthy(tmp_path):
    path = tmp_path / "identity" / "health.md"
    path.parent.mkdir()
    path.write_text(_valid_body(), encoding="utf-8")

    assert read_steward_health(tmp_path) is None


def test_read_steward_health_surfaces_first_attention_bullet(tmp_path):
    path = tmp_path / "identity" / "health.md"
    path.parent.mkdir()
    path.write_text(
        _valid_body(
            status="Sol found a pipeline gap.",
            needs="- Foo bar\n- Baz",
        ),
        encoding="utf-8",
    )

    assert read_steward_health(tmp_path) == {"status": "warning", "message": "Foo bar"}


def test_read_steward_health_needs_wins_over_status_mismatch(tmp_path):
    path = tmp_path / "identity" / "health.md"
    path.parent.mkdir()
    path.write_text(_valid_body(needs="- Foo bar"), encoding="utf-8")

    assert read_steward_health(tmp_path) == {"status": "warning", "message": "Foo bar"}


def test_read_steward_health_returns_none_when_malformed(tmp_path):
    path = tmp_path / "identity" / "health.md"
    path.parent.mkdir()
    path.write_text("not markdown", encoding="utf-8")

    assert read_steward_health(tmp_path) is None


def test_write_health_md_logs_render_failed_and_preserves_prior_file(
    tmp_path, monkeypatch
):
    _set_journal(monkeypatch, tmp_path)
    path = tmp_path / "identity" / "health.md"
    path.parent.mkdir()
    prior = _valid_body()
    path.write_text(prior, encoding="utf-8")

    reason = write_health_md("## Status\nbroken\n")

    assert reason is not None
    assert path.read_text(encoding="utf-8") == prior
    assert load_steward_log()[0]["event"] == "render.failed"


def test_health_md_history_has_only_steward_and_bootstrap_writers(
    tmp_path, monkeypatch
):
    _set_journal(monkeypatch, tmp_path)
    ensure_identity_directory()
    assert write_health_md(_valid_body()) is None
    assert read_steward_health(tmp_path) is None

    history_path = tmp_path / "identity" / "history.jsonl"
    rows = [json.loads(line) for line in history_path.read_text().splitlines()]
    actors = {row["actor"] for row in rows if row["file"] == "health.md"}

    assert actors <= {"steward", "ensure_identity_directory"}
