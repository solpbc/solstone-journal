# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""CLI wrapper tests for speaker commands."""

from __future__ import annotations

from typer.testing import CliRunner

from solstone.apps.speakers.call import app as speakers_app

runner = CliRunner()


def _bootstrap_stats() -> dict:
    return {
        "segments_scanned": 0,
        "single_speaker_segments": 0,
        "speakers_found": {},
        "entities_created": 0,
        "embeddings_saved": 0,
        "embeddings_skipped_owner": 0,
        "embeddings_skipped_duplicate": 0,
        "errors": [],
    }


def _resolve_names_stats() -> dict:
    return {
        "entities_with_voiceprints": 0,
        "pairs_compared": 0,
        "matches_found": [],
        "auto_merged": [],
        "ambiguous": [],
        "errors": [],
    }


def _backfill_stats() -> dict:
    return {
        "total_segments": 0,
        "total_eligible": 0,
        "skipped_no_embed": 0,
        "already_labeled": 0,
        "processed": 0,
        "speakers_seen": {},
        "errors": [],
    }


def _seed_stats() -> dict:
    return {
        "segments_scanned": 0,
        "segments_with_speakers": 0,
        "speakers_found": {},
        "embeddings_saved": 0,
        "embeddings_skipped_owner": 0,
        "embeddings_skipped_duplicate": 0,
        "speakers_unmatched": [],
        "errors": [],
    }


def _attribute_result() -> dict:
    return {
        "labels": [
            {
                "sentence_id": 1,
                "speaker": "alice",
                "confidence": "high",
                "method": "acoustic",
            }
        ],
        "unmatched": [],
        "source": "mic_audio",
        "metadata": {},
    }


def test_bootstrap_default_is_preview(speakers_env, monkeypatch):
    speakers_env()
    seen: dict[str, bool] = {}

    def fake_bootstrap_voiceprints(*, dry_run: bool) -> dict:
        seen["dry_run"] = dry_run
        return _bootstrap_stats()

    monkeypatch.setattr(
        "solstone.apps.speakers.bootstrap.bootstrap_voiceprints",
        fake_bootstrap_voiceprints,
    )

    result = runner.invoke(speakers_app, ["bootstrap"])

    assert result.exit_code == 0
    assert seen["dry_run"] is True


def test_bootstrap_commit_writes(speakers_env, monkeypatch):
    speakers_env()
    seen: dict[str, bool] = {}

    def fake_bootstrap_voiceprints(*, dry_run: bool) -> dict:
        seen["dry_run"] = dry_run
        return _bootstrap_stats()

    monkeypatch.setattr(
        "solstone.apps.speakers.bootstrap.bootstrap_voiceprints",
        fake_bootstrap_voiceprints,
    )

    result = runner.invoke(speakers_app, ["bootstrap", "--commit"])

    assert result.exit_code == 0
    assert seen["dry_run"] is False


def test_resolve_names_default_is_preview(speakers_env, monkeypatch):
    speakers_env()
    seen: dict[str, bool] = {}

    def fake_resolve_name_variants(*, dry_run: bool) -> dict:
        seen["dry_run"] = dry_run
        return _resolve_names_stats()

    monkeypatch.setattr(
        "solstone.apps.speakers.bootstrap.resolve_name_variants",
        fake_resolve_name_variants,
    )

    result = runner.invoke(speakers_app, ["resolve-names"])

    assert result.exit_code == 0
    assert seen["dry_run"] is True


def test_resolve_names_commit_writes(speakers_env, monkeypatch):
    speakers_env()
    seen: dict[str, bool] = {}

    def fake_resolve_name_variants(*, dry_run: bool) -> dict:
        seen["dry_run"] = dry_run
        return _resolve_names_stats()

    monkeypatch.setattr(
        "solstone.apps.speakers.bootstrap.resolve_name_variants",
        fake_resolve_name_variants,
    )

    result = runner.invoke(speakers_app, ["resolve-names", "--commit"])

    assert result.exit_code == 0
    assert seen["dry_run"] is False


def test_backfill_default_is_preview(speakers_env, monkeypatch):
    speakers_env()
    seen: dict[str, bool] = {}

    def fake_backfill_segments(*, dry_run: bool, progress_callback=None) -> dict:
        seen["dry_run"] = dry_run
        return _backfill_stats()

    monkeypatch.setattr(
        "solstone.apps.speakers.attribution.backfill_segments",
        fake_backfill_segments,
    )

    result = runner.invoke(speakers_app, ["backfill"])

    assert result.exit_code == 0
    assert seen["dry_run"] is True


def test_backfill_commit_writes(speakers_env, monkeypatch):
    speakers_env()
    seen: dict[str, bool] = {}

    def fake_backfill_segments(*, dry_run: bool, progress_callback=None) -> dict:
        seen["dry_run"] = dry_run
        return _backfill_stats()

    monkeypatch.setattr(
        "solstone.apps.speakers.attribution.backfill_segments",
        fake_backfill_segments,
    )

    result = runner.invoke(speakers_app, ["backfill", "--commit"])

    assert result.exit_code == 0
    assert seen["dry_run"] is False


def test_backfill_last_seen_dry_run_by_default(speakers_env, monkeypatch):
    speakers_env()
    seen: dict[str, bool] = {}

    def fake_backfill_last_seen(*, dry_run: bool) -> dict:
        seen["dry_run"] = dry_run
        return {
            "labels_read": 0,
            "entities_seen": 0,
            "rows_scanned": 0,
            "rows_pending": 0,
            "rows_written": 0,
            "pending": {},
            "errors": [],
        }

    monkeypatch.setattr(
        "solstone.apps.speakers.attribution.backfill_last_seen",
        fake_backfill_last_seen,
    )

    result = runner.invoke(speakers_app, ["backfill-last-seen"])

    assert result.exit_code == 0
    assert seen["dry_run"] is True


def test_backfill_last_seen_commit_writes(speakers_env, monkeypatch):
    speakers_env()
    seen: dict[str, bool] = {}

    def fake_backfill_last_seen(*, dry_run: bool) -> dict:
        seen["dry_run"] = dry_run
        return {
            "labels_read": 0,
            "entities_seen": 0,
            "rows_scanned": 0,
            "rows_pending": 0,
            "rows_written": 0,
            "pending": {},
            "errors": [],
        }

    monkeypatch.setattr(
        "solstone.apps.speakers.attribution.backfill_last_seen",
        fake_backfill_last_seen,
    )

    result = runner.invoke(speakers_app, ["backfill-last-seen", "--commit"])

    assert result.exit_code == 0
    assert seen["dry_run"] is False


def test_seed_from_imports_default_is_preview(speakers_env, monkeypatch):
    speakers_env()
    seen: dict[str, bool] = {}

    def fake_seed_from_imports(*, dry_run: bool) -> dict:
        seen["dry_run"] = dry_run
        return _seed_stats()

    monkeypatch.setattr(
        "solstone.apps.speakers.bootstrap.seed_from_imports",
        fake_seed_from_imports,
    )

    result = runner.invoke(speakers_app, ["seed-from-imports"])

    assert result.exit_code == 0
    assert seen["dry_run"] is True


def test_seed_from_imports_commit_writes(speakers_env, monkeypatch):
    speakers_env()
    seen: dict[str, bool] = {}

    def fake_seed_from_imports(*, dry_run: bool) -> dict:
        seen["dry_run"] = dry_run
        return _seed_stats()

    monkeypatch.setattr(
        "solstone.apps.speakers.bootstrap.seed_from_imports",
        fake_seed_from_imports,
    )

    result = runner.invoke(speakers_app, ["seed-from-imports", "--commit"])

    assert result.exit_code == 0
    assert seen["dry_run"] is False


def test_attribute_segment_default_skips_writes(speakers_env, monkeypatch):
    speakers_env()
    save_calls: list[tuple] = []
    accumulate_calls: list[tuple] = []

    monkeypatch.setattr(
        "solstone.apps.speakers.attribution.attribute_segment",
        lambda *_args, **_kwargs: _attribute_result(),
    )
    monkeypatch.setattr(
        "solstone.apps.speakers.attribution.save_speaker_labels",
        lambda *args, **kwargs: save_calls.append((args, kwargs)),
    )
    monkeypatch.setattr(
        "solstone.apps.speakers.attribution.accumulate_voiceprints",
        lambda *args, **kwargs: accumulate_calls.append((args, kwargs)),
    )

    result = runner.invoke(
        speakers_app,
        ["attribute-segment", "20240101", "test", "090000_300"],
    )

    assert result.exit_code == 0
    assert save_calls == []
    assert accumulate_calls == []


def test_attribute_segment_commit_writes_both(speakers_env, monkeypatch, tmp_path):
    speakers_env()
    save_calls: list[tuple] = []
    accumulate_calls: list[tuple] = []

    monkeypatch.setattr(
        "solstone.apps.speakers.attribution.attribute_segment",
        lambda *_args, **_kwargs: _attribute_result(),
    )
    monkeypatch.setattr(
        "solstone.apps.speakers.attribution.save_speaker_labels",
        lambda *args, **kwargs: (
            save_calls.append((args, kwargs)) or (tmp_path / "speaker_labels.json")
        ),
    )
    monkeypatch.setattr(
        "solstone.apps.speakers.attribution.accumulate_voiceprints",
        lambda *args, **kwargs: accumulate_calls.append((args, kwargs)) or {"alice": 1},
    )
    monkeypatch.setattr("solstone.think.utils.segment_path", lambda *_args: tmp_path)

    result = runner.invoke(
        speakers_app,
        ["attribute-segment", "20240101", "test", "090000_300", "--commit"],
    )

    assert result.exit_code == 0
    assert len(save_calls) == 1
    assert len(accumulate_calls) == 1


def test_attribute_segment_commit_no_save(speakers_env, monkeypatch, tmp_path):
    speakers_env()
    save_calls: list[tuple] = []
    accumulate_calls: list[tuple] = []

    monkeypatch.setattr(
        "solstone.apps.speakers.attribution.attribute_segment",
        lambda *_args, **_kwargs: _attribute_result(),
    )
    monkeypatch.setattr(
        "solstone.apps.speakers.attribution.save_speaker_labels",
        lambda *args, **kwargs: (
            save_calls.append((args, kwargs)) or (tmp_path / "speaker_labels.json")
        ),
    )
    monkeypatch.setattr(
        "solstone.apps.speakers.attribution.accumulate_voiceprints",
        lambda *args, **kwargs: accumulate_calls.append((args, kwargs)) or {"alice": 1},
    )
    monkeypatch.setattr("solstone.think.utils.segment_path", lambda *_args: tmp_path)

    result = runner.invoke(
        speakers_app,
        [
            "attribute-segment",
            "20240101",
            "test",
            "090000_300",
            "--commit",
            "--no-save",
        ],
    )

    assert result.exit_code == 0
    assert save_calls == []
    assert len(accumulate_calls) == 1


def test_attribute_segment_commit_no_accumulate(speakers_env, monkeypatch, tmp_path):
    speakers_env()
    save_calls: list[tuple] = []
    accumulate_calls: list[tuple] = []

    monkeypatch.setattr(
        "solstone.apps.speakers.attribution.attribute_segment",
        lambda *_args, **_kwargs: _attribute_result(),
    )
    monkeypatch.setattr(
        "solstone.apps.speakers.attribution.save_speaker_labels",
        lambda *args, **kwargs: (
            save_calls.append((args, kwargs)) or (tmp_path / "speaker_labels.json")
        ),
    )
    monkeypatch.setattr(
        "solstone.apps.speakers.attribution.accumulate_voiceprints",
        lambda *args, **kwargs: accumulate_calls.append((args, kwargs)) or {"alice": 1},
    )
    monkeypatch.setattr("solstone.think.utils.segment_path", lambda *_args: tmp_path)

    result = runner.invoke(
        speakers_app,
        [
            "attribute-segment",
            "20240101",
            "test",
            "090000_300",
            "--commit",
            "--no-accumulate",
        ],
    )

    assert result.exit_code == 0
    assert len(save_calls) == 1
    assert accumulate_calls == []


def test_speaker_commands_reject_removed_dry_run_flag(speakers_env):
    speakers_env()
    commands = [
        ["bootstrap", "--dry-run"],
        ["resolve-names", "--dry-run"],
        ["backfill", "--dry-run"],
        ["seed-from-imports", "--dry-run"],
        ["attribute-segment", "20240101", "test", "090000_300", "--dry-run"],
    ]

    for argv in commands:
        result = runner.invoke(speakers_app, argv)
        assert result.exit_code != 0
