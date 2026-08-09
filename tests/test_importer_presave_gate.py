# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import datetime as dt
import json
from argparse import Namespace
from copy import deepcopy
from importlib import import_module
from pathlib import Path

import pytest

from solstone.think.importers.file_importer import ImportResult
from solstone.think.importers.health_schema import (
    SOURCE_DEXCOM_CLARITY,
    SOURCE_OURA_API,
)
from solstone.think.importers.pre_save_gate import (
    APPROVAL_SCHEMA,
    CHECKLIST_DESTINATIONS,
    CHECKLIST_VERSION,
    OURA_SYNC_APPROVAL_SCHEMA,
    OURA_SYNC_CHECKLIST_VERSION,
    SENSITIVE_IMPORTERS,
    PreSaveGateError,
    RawRetentionDecision,
    approval_path_for_journal,
    enforce_oura_sync_gate,
    enforce_pre_save_gate,
    oura_sync_approval_path_for_journal,
    read_oura_sync_approval,
)
from tests.conftest import write_health_approval_artifact

FIXED_NOW = dt.datetime(2026, 7, 13, 12, 0, tzinfo=dt.UTC)
FUTURE_VALID_UNTIL = "2026-08-01T00:00:00Z"

APPLE_HEALTH_FIXTURE = (
    Path(__file__).parent
    / "fixtures"
    / "importers"
    / "health"
    / "apple_health_synthetic"
)
OURA_FIXTURE = (
    Path(__file__).parent / "fixtures" / "importers" / "health" / "oura_synthetic"
)


def _use_journal(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> Path:
    journal = tmp_path / "journal"
    journal.mkdir()
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))
    return journal


def _replication_destinations() -> dict:
    return {
        destination: {
            "decision": "approved" if destination == "time_machine" else "excluded",
            "notes": "Synthetic test decision.",
        }
        for destination in CHECKLIST_DESTINATIONS
    }


def _raw_retention(
    decision: str = RawRetentionDecision.RETAIN_PARSED.value,
    *,
    acknowledged: bool | None = None,
) -> dict:
    raw_retention = {
        "decision": decision,
        "notes": "Synthetic test decision.",
    }
    if acknowledged is not None:
        raw_retention["unparsed_sensitive_modalities_acknowledged"] = acknowledged
    return raw_retention


def _scheduled_sync_consent(
    *,
    approved: bool = True,
    cadence: str = "every 6 hours",
    valid_until: str = FUTURE_VALID_UNTIL,
) -> dict:
    return {
        "approved": approved,
        "cadence": cadence,
        "valid_until": valid_until,
    }


def _valid_artifact(
    journal: Path,
    *,
    raw_retention_decision: str = RawRetentionDecision.RETAIN_PARSED.value,
    acknowledged: bool | None = None,
) -> dict:
    return {
        "schema": APPROVAL_SCHEMA,
        "checklist_version": CHECKLIST_VERSION,
        "approved_by": "Jack",
        "approved_at": "2026-07-03T23:22:00-06:00",
        "journal_root": str(journal.resolve()),
        "approved_importers": ["apple_health"],
        "replication_destinations": _replication_destinations(),
        "raw_retention": _raw_retention(
            raw_retention_decision,
            acknowledged=acknowledged,
        ),
        "requires_per_run_confirmation": True,
        "no_real_health_data_in_artifact": True,
    }


def _write_artifact(journal: Path, payload: dict) -> Path:
    approval_path = approval_path_for_journal(journal)
    approval_path.parent.mkdir(parents=True, exist_ok=True)
    approval_path.write_text(json.dumps(payload), encoding="utf-8")
    return approval_path


def _valid_sync_artifact(
    journal: Path,
    *,
    raw_retention_decision: str = RawRetentionDecision.RETAIN_PARSED.value,
) -> dict:
    return {
        "schema": OURA_SYNC_APPROVAL_SCHEMA,
        "checklist_version": OURA_SYNC_CHECKLIST_VERSION,
        "approved_by": "Jack",
        "approved_at": "2026-07-06T09:00:00-06:00",
        "journal_root": str(journal.resolve()),
        "replication_destinations": _replication_destinations(),
        "raw_retention": _raw_retention(raw_retention_decision),
        "requires_per_run_confirmation": True,
        "no_real_health_data_in_artifact": True,
    }


def _write_sync_artifact(journal: Path, payload: dict) -> Path:
    approval_path = oura_sync_approval_path_for_journal(journal)
    approval_path.parent.mkdir(parents=True, exist_ok=True)
    approval_path.write_text(json.dumps(payload), encoding="utf-8")
    return approval_path


def _run_gate_then_save(
    importer: str,
    *,
    dry_run: bool,
    confirm_health_save: bool = False,
    setup,
    process,
):
    decision = enforce_pre_save_gate(
        importer,
        dry_run=dry_run,
        confirm_health_save=confirm_health_save,
    )
    if dry_run:
        return decision
    setup()
    return process()


def test_sensitive_importers_are_importer_names_not_source_families():
    assert SENSITIVE_IMPORTERS == frozenset({"apple_health", "oura"})
    assert SOURCE_OURA_API not in SENSITIVE_IMPORTERS
    assert SOURCE_DEXCOM_CLARITY not in SENSITIVE_IMPORTERS


def test_apple_health_save_missing_artifact_blocks_before_setup(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
):
    journal = _use_journal(tmp_path, monkeypatch)

    with pytest.raises(PreSaveGateError) as exc_info:
        _run_gate_then_save(
            "apple_health",
            dry_run=False,
            setup=lambda: pytest.fail("setup should not run before gate approval"),
            process=lambda: pytest.fail("process should not run before setup"),
        )

    payload = exc_info.value.to_dict()
    assert exc_info.value.exit_code == 2
    assert payload["reason"] == "health_pre_save_gate_required"
    assert payload["gate_reason"] == "missing_approval_artifact"
    assert payload["importer"] == "apple_health"
    assert payload["approval_path"] == str(approval_path_for_journal(journal.resolve()))
    assert payload["missing_fields"] == ["approval_artifact"]
    assert not (journal / "imports" / "20260102_123000").exists()


def test_blocked_health_gate_creates_no_directories(tmp_path: Path):
    journal = tmp_path / "missing-journal"

    with pytest.raises(PreSaveGateError) as exc_info:
        enforce_pre_save_gate(
            "apple_health",
            dry_run=False,
            confirm_health_save=True,
            journal_root=journal,
        )

    assert exc_info.value.to_dict()["gate_reason"] == "missing_approval_artifact"
    assert not journal.exists()


def test_cli_apple_health_save_delegates_before_python_setup(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
):
    journal = _use_journal(tmp_path, monkeypatch)
    cli = import_module("solstone.think.importers.cli")
    body_native = import_module("solstone.think.body_native")
    file_importer = import_module("solstone.think.importers.file_importer")
    monkeypatch.setitem(
        file_importer.FILE_IMPORTER_REGISTRY,
        "apple_health",
        "solstone.think.importers.apple_health",
    )
    monkeypatch.setattr(
        cli,
        "_setup_file_import",
        lambda import_id: pytest.fail("_setup_file_import should not run"),
    )
    calls = []

    def native_apple(source, journal_root, **kwargs):
        calls.append((source, journal_root, kwargs))
        return {
            "schema": "solstone.body.ingest.result.v1",
            "source": "apple_health",
            "mode": "save",
            "rows": 0,
            "days": [],
            "skipped": False,
            "bundle_id": None,
        }

    monkeypatch.setattr(body_native, "apple_health", native_apple)

    args = Namespace(
        media=str(APPLE_HEALTH_FIXTURE),
        timestamp="20260102_123000",
        facet=None,
        setting=None,
        source="apple_health",
        force=False,
        auto=None,
        dry_run=False,
        json=False,
        verbose=False,
        wait_for_processing=True,
        deterministic_only=False,
        confirm_health_save=False,
        date_from=None,
        date_to=None,
        with_day_summaries=False,
    )

    result = cli._import_one_from_args(args)

    assert result is not None
    assert result["rows"] == 0
    assert calls == [
        (
            APPLE_HEALTH_FIXTURE,
            journal,
            {
                "save": True,
                "confirm_body_save": False,
                "date_from": None,
                "date_to": None,
                "force": False,
            },
        )
    ]
    assert not (journal / "imports" / "20260102_123000").exists()


def test_apple_health_save_missing_confirm_flag_blocks_with_artifact(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
):
    journal = _use_journal(tmp_path, monkeypatch)
    _write_artifact(journal, _valid_artifact(journal))

    with pytest.raises(PreSaveGateError) as exc_info:
        _run_gate_then_save(
            "apple_health",
            dry_run=False,
            setup=lambda: pytest.fail("setup should not run without per-run confirm"),
            process=lambda: pytest.fail("process should not run before setup"),
        )

    payload = exc_info.value.to_dict()
    assert payload["reason"] == "health_pre_save_gate_required"
    assert payload["gate_reason"] == "per_run_confirmation_missing"
    assert payload["target_journal"] == str(journal.resolve())
    assert payload["missing_fields"] == ["confirm_health_save"]


# ---------------------------------------------------------------------------
# Journal-root binding and old-version rejection
# ---------------------------------------------------------------------------


def test_apple_health_save_journal_root_mismatch_blocks(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
):
    journal = _use_journal(tmp_path, monkeypatch)
    artifact = _valid_artifact(journal)
    artifact["journal_root"] = str((tmp_path / "other-journal").resolve())
    _write_artifact(journal, artifact)

    with pytest.raises(PreSaveGateError) as exc_info:
        _run_gate_then_save(
            "apple_health",
            dry_run=False,
            confirm_health_save=True,
            setup=lambda: pytest.fail("setup should not run on binding mismatch"),
            process=lambda: pytest.fail("process should not run before setup"),
        )

    payload = exc_info.value.to_dict()
    assert payload["reason"] == "health_pre_save_gate_required"
    assert payload["gate_reason"] == "journal_root_binding_mismatch"
    assert payload["invalid_fields"] == ["journal_root"]
    assert not (journal / "imports" / "20260102_123000").exists()


def test_gate_is_root_explicit_artifact_must_live_in_target_journal(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
):
    # SOLSTONE_JOURNAL points at journal A, fully approved. The caller
    # targets journal B: the gate must read (and miss) B's artifact, never
    # silently substitute A's.
    journal_a = _use_journal(tmp_path, monkeypatch)
    _write_artifact(journal_a, _valid_artifact(journal_a))
    journal_b = tmp_path / "journal-b"
    journal_b.mkdir()

    with pytest.raises(PreSaveGateError) as exc_info:
        enforce_pre_save_gate(
            "apple_health",
            dry_run=False,
            confirm_health_save=True,
            journal_root=journal_b,
        )

    payload = exc_info.value.to_dict()
    assert payload["gate_reason"] == "missing_approval_artifact"
    assert payload["approval_path"] == str(
        approval_path_for_journal(journal_b.resolve())
    )
    assert not (journal_b / "imports").exists()


def test_gate_artifact_copied_from_another_journal_never_authorizes(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
):
    journal_a = _use_journal(tmp_path, monkeypatch)
    journal_b = tmp_path / "journal-b"
    journal_b.mkdir()
    # B holds a byte-copy of A's artifact: the recorded binding still
    # names A, so it must not authorize writes into B.
    _write_artifact(journal_b, _valid_artifact(journal_a))

    with pytest.raises(PreSaveGateError) as exc_info:
        enforce_pre_save_gate(
            "apple_health",
            dry_run=False,
            confirm_health_save=True,
            journal_root=journal_b,
        )

    assert exc_info.value.to_dict()["gate_reason"] == "journal_root_binding_mismatch"


def test_gate_passes_for_explicit_target_with_its_own_artifact(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
):
    _use_journal(tmp_path, monkeypatch)
    journal_b = tmp_path / "journal-b"
    journal_b.mkdir()
    _write_artifact(journal_b, _valid_artifact(journal_b))

    decision = enforce_pre_save_gate(
        "apple_health",
        dry_run=False,
        confirm_health_save=True,
        journal_root=journal_b,
    )

    assert decision.enforced is True
    assert decision.approval_path == str(approval_path_for_journal(journal_b.resolve()))
    assert decision.raw_retention is RawRetentionDecision.RETAIN_PARSED


@pytest.mark.parametrize(
    "old_version",
    [
        "solstone.health_import_preflight.checklist.v1",
        "solstone.health_import_preflight.checklist.v2",
    ],
)
def test_old_health_checklist_versions_fail_closed(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    old_version: str,
):
    journal = _use_journal(tmp_path, monkeypatch)
    artifact = _valid_artifact(journal)
    artifact["checklist_version"] = old_version
    _write_artifact(journal, artifact)

    with pytest.raises(PreSaveGateError) as exc_info:
        enforce_pre_save_gate(
            "apple_health",
            dry_run=False,
            confirm_health_save=True,
        )

    payload = exc_info.value.to_dict()
    assert payload["gate_reason"] == "checklist_version_mismatch"
    assert payload["invalid_fields"] == ["checklist_version"]


def test_target_journal_path_legacy_binding_no_longer_authorizes(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
):
    journal = _use_journal(tmp_path, monkeypatch)
    artifact = _valid_artifact(journal)
    del artifact["journal_root"]
    artifact["target_journal_path"] = str(journal.resolve())
    _write_artifact(journal, artifact)

    with pytest.raises(PreSaveGateError) as exc_info:
        enforce_pre_save_gate("apple_health", dry_run=False, confirm_health_save=True)

    payload = exc_info.value.to_dict()
    assert payload["gate_reason"] == "journal_root_binding_missing"
    assert payload["missing_fields"] == ["journal_root"]


def test_relative_target_journal_root_fails_before_resolution(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
):
    journal = _use_journal(tmp_path, monkeypatch)
    _write_artifact(journal, _valid_artifact(journal))
    monkeypatch.chdir(journal)

    with pytest.raises(PreSaveGateError) as exc_info:
        enforce_pre_save_gate(
            "apple_health",
            dry_run=False,
            confirm_health_save=True,
            journal_root=Path("."),
        )

    payload = exc_info.value.to_dict()
    assert payload["gate_reason"] == "target_journal_not_absolute"
    assert payload["invalid_fields"] == ["journal_root"]
    assert payload["target_journal"] == "."


def test_relative_artifact_journal_root_binding_fails_before_resolution(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
):
    journal = _use_journal(tmp_path, monkeypatch)
    artifact = _valid_artifact(journal)
    artifact["journal_root"] = "."
    _write_artifact(journal, artifact)
    monkeypatch.chdir(journal)

    with pytest.raises(PreSaveGateError) as exc_info:
        enforce_pre_save_gate(
            "apple_health",
            dry_run=False,
            confirm_health_save=True,
            journal_root=journal,
        )

    payload = exc_info.value.to_dict()
    assert payload["gate_reason"] == "journal_root_binding_not_absolute"
    assert payload["invalid_fields"] == ["journal_root"]


@pytest.mark.parametrize(
    ("mutate", "reason", "missing_fields", "invalid_fields"),
    [
        (
            lambda artifact: artifact["replication_destinations"].pop("icloud"),
            "replication_decision_incomplete",
            ["replication_destinations.icloud"],
            [],
        ),
        (
            lambda artifact: artifact.update({"checklist_version": "future.v3"}),
            "checklist_version_mismatch",
            [],
            ["checklist_version"],
        ),
    ],
)
def test_apple_health_save_incomplete_replication_decisions_block(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    mutate,
    reason: str,
    missing_fields: list[str],
    invalid_fields: list[str],
):
    journal = _use_journal(tmp_path, monkeypatch)
    artifact = _valid_artifact(journal)
    mutate(artifact)
    _write_artifact(journal, artifact)

    with pytest.raises(PreSaveGateError) as exc_info:
        _run_gate_then_save(
            "apple_health",
            dry_run=False,
            confirm_health_save=True,
            setup=lambda: pytest.fail("setup should not run before checklist passes"),
            process=lambda: pytest.fail("process should not run before setup"),
        )

    payload = exc_info.value.to_dict()
    assert payload["reason"] == "health_pre_save_gate_required"
    assert payload["gate_reason"] == reason
    assert payload["missing_fields"] == missing_fields
    assert payload["invalid_fields"] == invalid_fields
    assert payload["checklist_version"] == CHECKLIST_VERSION


def test_apple_health_save_missing_raw_retention_blocks(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
):
    journal = _use_journal(tmp_path, monkeypatch)
    artifact = _valid_artifact(journal)
    artifact.pop("raw_retention")
    _write_artifact(journal, artifact)

    with pytest.raises(PreSaveGateError) as exc_info:
        _run_gate_then_save(
            "apple_health",
            dry_run=False,
            confirm_health_save=True,
            setup=lambda: pytest.fail(
                "setup should not run without retention decision"
            ),
            process=lambda: pytest.fail("process should not run before setup"),
        )

    payload = exc_info.value.to_dict()
    assert payload["reason"] == "health_pre_save_gate_required"
    assert payload["gate_reason"] == "raw_retention_decision_missing"
    assert payload["missing_fields"] == ["raw_retention.decision"]


def test_unknown_raw_retention_string_blocks(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
):
    journal = _use_journal(tmp_path, monkeypatch)
    artifact = _valid_artifact(journal, raw_retention_decision="retain_compressed_zip")
    _write_artifact(journal, artifact)

    with pytest.raises(PreSaveGateError) as exc_info:
        enforce_pre_save_gate(
            "apple_health",
            dry_run=False,
            confirm_health_save=True,
        )

    payload = exc_info.value.to_dict()
    assert payload["gate_reason"] == "raw_retention_decision_invalid"
    assert payload["invalid_fields"] == ["raw_retention.decision"]


@pytest.mark.parametrize(
    ("acknowledged", "missing_fields", "invalid_fields"),
    [
        (None, ["raw_retention.unparsed_sensitive_modalities_acknowledged"], []),
        (False, [], ["raw_retention.unparsed_sensitive_modalities_acknowledged"]),
    ],
)
def test_retain_complete_without_sensitive_modalities_ack_blocks(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    acknowledged: bool | None,
    missing_fields: list[str],
    invalid_fields: list[str],
):
    journal = _use_journal(tmp_path, monkeypatch)
    artifact = _valid_artifact(
        journal,
        raw_retention_decision=RawRetentionDecision.RETAIN_COMPLETE.value,
        acknowledged=acknowledged,
    )
    _write_artifact(journal, artifact)

    with pytest.raises(PreSaveGateError) as exc_info:
        enforce_pre_save_gate(
            "apple_health",
            dry_run=False,
            confirm_health_save=True,
        )

    payload = exc_info.value.to_dict()
    assert payload["gate_reason"] == "raw_retention_acknowledgement_missing"
    assert payload["missing_fields"] == missing_fields
    assert payload["invalid_fields"] == invalid_fields


def test_apple_health_retain_complete_with_ack_passes(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
):
    journal = _use_journal(tmp_path, monkeypatch)
    artifact = _valid_artifact(
        journal,
        raw_retention_decision=RawRetentionDecision.RETAIN_COMPLETE.value,
        acknowledged=True,
    )
    _write_artifact(journal, artifact)

    decision = enforce_pre_save_gate(
        "apple_health",
        dry_run=False,
        confirm_health_save=True,
    )

    assert decision.raw_retention is RawRetentionDecision.RETAIN_COMPLETE


def test_oura_sync_retain_complete_blocks_as_source_incompatible(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
):
    journal = _use_journal(tmp_path, monkeypatch)
    artifact = _valid_sync_artifact(
        journal,
        raw_retention_decision=RawRetentionDecision.RETAIN_COMPLETE.value,
    )
    _write_sync_artifact(journal, artifact)

    with pytest.raises(PreSaveGateError) as exc_info:
        enforce_oura_sync_gate(journal, confirm_health_save=True, now=FIXED_NOW)

    payload = exc_info.value.to_dict()
    assert payload["gate_reason"] == "raw_retention_decision_incompatible"
    assert payload["invalid_fields"] == ["raw_retention.decision"]


def test_apple_health_save_with_valid_gate_reaches_process(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
):
    journal = _use_journal(tmp_path, monkeypatch)
    _write_artifact(journal, _valid_artifact(journal))
    calls: list[str] = []

    def setup() -> None:
        calls.append("setup")

    def process() -> ImportResult:
        calls.append("process")
        return ImportResult(
            entries_written=0,
            entities_seeded=0,
            files_created=[],
            errors=[],
            summary="synthetic apple health import",
            segments=None,
        )

    result = _run_gate_then_save(
        "apple_health",
        dry_run=False,
        confirm_health_save=True,
        setup=setup,
        process=process,
    )

    assert calls == ["setup", "process"]
    assert result.files_created == []
    assert result.segments is None


def test_cli_apple_health_save_dispatches_to_native_owner(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
):
    journal = _use_journal(tmp_path, monkeypatch)
    write_health_approval_artifact(journal, importers=["apple_health"])
    before = {
        path.relative_to(journal): path.read_bytes() if path.is_file() else None
        for path in journal.rglob("*")
    }
    cli = import_module("solstone.think.importers.cli")
    body_native = import_module("solstone.think.body_native")
    expected = {
        "schema": "solstone.body.ingest.result.v1",
        "source": "apple_health",
        "mode": "save",
        "rows": 2,
        "days": ["20260102"],
        "skipped": False,
        "bundle_id": "body-01KZZZZZZZZZZZZZZZZZZZZZZZ",
    }
    calls = []

    def native_apple(source, journal_root, **kwargs):
        calls.append((source, journal_root, kwargs))
        return expected

    monkeypatch.setattr(body_native, "apple_health", native_apple)

    result = cli._import_one_from_args(
        Namespace(
            media=str(APPLE_HEALTH_FIXTURE),
            timestamp="20260102_123000",
            facet=None,
            setting=None,
            source="apple_health",
            force=False,
            auto=None,
            dry_run=False,
            json=False,
            verbose=False,
            wait_for_processing=True,
            deterministic_only=False,
            confirm_health_save=True,
            date_from=None,
            date_to=None,
            with_day_summaries=False,
        )
    )

    assert result == expected
    assert calls == [
        (
            APPLE_HEALTH_FIXTURE,
            journal,
            {
                "save": True,
                "confirm_body_save": True,
                "date_from": None,
                "date_to": None,
                "force": False,
            },
        )
    ]
    assert {
        path.relative_to(journal): path.read_bytes() if path.is_file() else None
        for path in journal.rglob("*")
    } == before


def test_cli_oura_file_save_refuses_before_python_setup(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
):
    journal = _use_journal(tmp_path, monkeypatch)
    write_health_approval_artifact(journal, importers=["oura"])
    before = {
        path.relative_to(journal): path.read_bytes() if path.is_file() else None
        for path in journal.rglob("*")
    }
    cli = import_module("solstone.think.importers.cli")
    monkeypatch.setattr(
        cli,
        "_setup_file_import",
        lambda import_id: pytest.fail("_setup_file_import should not run"),
    )

    with pytest.raises(ValueError, match="imports through sync"):
        cli._import_one_from_args(
            Namespace(
                media=str(OURA_FIXTURE),
                timestamp="20260102_123000",
                facet=None,
                setting=None,
                source="oura",
                force=False,
                auto=None,
                dry_run=False,
                json=False,
                verbose=False,
                wait_for_processing=True,
                deterministic_only=False,
                confirm_health_save=True,
                date_from=None,
                date_to=None,
                with_day_summaries=False,
            )
        )
    assert {
        path.relative_to(journal): path.read_bytes() if path.is_file() else None
        for path in journal.rglob("*")
    } == before


def test_file_importer_dry_run_does_not_require_health_gate(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
):
    journal = _use_journal(tmp_path, monkeypatch)

    decision = _run_gate_then_save(
        "apple_health",
        dry_run=True,
        setup=lambda: pytest.fail("dry run should not save"),
        process=lambda: pytest.fail("dry run should not process save path"),
    )

    assert decision.enforced is False
    assert decision.approval_path is None
    assert APPLE_HEALTH_FIXTURE.exists()
    assert not (journal / "imports").exists()


def test_non_sensitive_importer_save_does_not_require_health_gate(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
):
    _use_journal(tmp_path, monkeypatch)
    calls: list[str] = []

    result = _run_gate_then_save(
        "ics",
        dry_run=False,
        setup=lambda: calls.append("setup"),
        process=lambda: calls.append("process"),
    )

    assert result is None
    assert calls == ["setup", "process"]


def test_json_failure_shape_contains_no_source_health_path_or_values(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
):
    journal = _use_journal(tmp_path, monkeypatch)
    artifact = _valid_artifact(journal)
    del artifact["replication_destinations"]["solbase"]
    artifact_with_extra = deepcopy(artifact)
    artifact_with_extra["notes"] = "No source health values live here."
    _write_artifact(journal, artifact_with_extra)

    with pytest.raises(PreSaveGateError) as exc_info:
        enforce_pre_save_gate(
            "apple_health",
            dry_run=False,
            confirm_health_save=True,
        )

    failure_json = json.dumps(exc_info.value.to_dict(), sort_keys=True)
    assert '"reason": "health_pre_save_gate_required"' in failure_json
    assert '"gate_reason": "replication_decision_incomplete"' in failure_json
    assert '"importer": "apple_health"' in failure_json
    assert str(approval_path_for_journal(journal.resolve())) in failure_json
    assert str(APPLE_HEALTH_FIXTURE) not in failure_json
    assert "HKQuantityTypeIdentifierStepCount" not in failure_json
    assert "synthetic-route.gpx" not in failure_json


# ---------------------------------------------------------------------------
# Oura sync gate — its own artifact + scheduled-sync standing consent
# ---------------------------------------------------------------------------


def test_sync_gate_missing_artifact_blocks(tmp_path: Path, monkeypatch):
    journal = _use_journal(tmp_path, monkeypatch)

    with pytest.raises(PreSaveGateError) as exc_info:
        enforce_oura_sync_gate(journal, confirm_health_save=True)

    payload = exc_info.value.to_dict()
    assert payload["gate_reason"] == "missing_approval_artifact"
    assert payload["flow"] == "sync"
    assert payload["approval_path"] == str(
        oura_sync_approval_path_for_journal(journal.resolve())
    )
    assert payload["checklist_version"] == OURA_SYNC_CHECKLIST_VERSION


def test_old_oura_sync_checklist_version_fails_closed(tmp_path: Path, monkeypatch):
    journal = _use_journal(tmp_path, monkeypatch)
    artifact = _valid_sync_artifact(journal)
    artifact["checklist_version"] = "solstone.oura_sync_preflight.checklist.v1"
    _write_sync_artifact(journal, artifact)

    with pytest.raises(PreSaveGateError) as exc_info:
        enforce_oura_sync_gate(journal, confirm_health_save=True, now=FIXED_NOW)

    payload = exc_info.value.to_dict()
    assert payload["gate_reason"] == "checklist_version_mismatch"
    assert payload["invalid_fields"] == ["checklist_version"]


def test_sync_gate_one_shot_passes_with_confirmation(tmp_path: Path, monkeypatch):
    journal = _use_journal(tmp_path, monkeypatch)
    _write_sync_artifact(journal, _valid_sync_artifact(journal))

    decision = enforce_oura_sync_gate(journal, confirm_health_save=True, now=FIXED_NOW)

    assert decision.enforced is True
    assert decision.importer == "oura"
    assert decision.checklist_version == OURA_SYNC_CHECKLIST_VERSION
    assert decision.raw_retention is RawRetentionDecision.RETAIN_PARSED
    assert decision.scheduled_sync is None


def test_sync_gate_one_shot_without_confirmation_blocks(tmp_path: Path, monkeypatch):
    journal = _use_journal(tmp_path, monkeypatch)
    _write_sync_artifact(journal, _valid_sync_artifact(journal))

    with pytest.raises(PreSaveGateError) as exc_info:
        enforce_oura_sync_gate(journal, now=FIXED_NOW)

    payload = exc_info.value.to_dict()
    assert payload["gate_reason"] == "per_run_confirmation_missing"
    assert payload["missing_fields"] == ["confirm_health_save"]


def test_sync_gate_one_shot_does_not_treat_scheduled_consent_as_confirmation(
    tmp_path: Path, monkeypatch
):
    journal = _use_journal(tmp_path, monkeypatch)
    artifact = _valid_sync_artifact(journal)
    artifact["scheduled_sync"] = _scheduled_sync_consent()
    _write_sync_artifact(journal, artifact)

    with pytest.raises(PreSaveGateError) as exc_info:
        enforce_oura_sync_gate(journal, now=FIXED_NOW)

    payload = exc_info.value.to_dict()
    assert payload["gate_reason"] == "per_run_confirmation_missing"
    assert payload["missing_fields"] == ["confirm_health_save"]


def test_sync_gate_requires_journal_root_binding(tmp_path: Path, monkeypatch):
    journal = _use_journal(tmp_path, monkeypatch)
    artifact = _valid_sync_artifact(journal)
    del artifact["journal_root"]
    _write_sync_artifact(journal, artifact)

    with pytest.raises(PreSaveGateError) as exc_info:
        enforce_oura_sync_gate(journal, confirm_health_save=True, now=FIXED_NOW)

    payload = exc_info.value.to_dict()
    assert payload["gate_reason"] == "journal_root_binding_missing"
    assert payload["missing_fields"] == ["journal_root"]


def test_sync_gate_binding_mismatch_blocks(tmp_path: Path, monkeypatch):
    journal = _use_journal(tmp_path, monkeypatch)
    artifact = _valid_sync_artifact(journal)
    artifact["journal_root"] = str((tmp_path / "elsewhere").resolve())
    _write_sync_artifact(journal, artifact)

    with pytest.raises(PreSaveGateError) as exc_info:
        enforce_oura_sync_gate(journal, confirm_health_save=True, now=FIXED_NOW)

    assert exc_info.value.to_dict()["gate_reason"] == "journal_root_binding_mismatch"


def test_sync_gate_relative_target_journal_root_fails_before_resolution(
    tmp_path: Path, monkeypatch
):
    journal = _use_journal(tmp_path, monkeypatch)
    _write_sync_artifact(journal, _valid_sync_artifact(journal))
    monkeypatch.chdir(journal)

    with pytest.raises(PreSaveGateError) as exc_info:
        enforce_oura_sync_gate(
            Path("."),
            confirm_health_save=True,
            now=FIXED_NOW,
        )

    payload = exc_info.value.to_dict()
    assert payload["gate_reason"] == "target_journal_not_absolute"
    assert payload["invalid_fields"] == ["journal_root"]
    assert payload["target_journal"] == "."


def test_sync_gate_relative_artifact_binding_fails_before_resolution(
    tmp_path: Path, monkeypatch
):
    journal = _use_journal(tmp_path, monkeypatch)
    artifact = _valid_sync_artifact(journal)
    artifact["journal_root"] = "."
    _write_sync_artifact(journal, artifact)
    monkeypatch.chdir(journal)

    with pytest.raises(PreSaveGateError) as exc_info:
        enforce_oura_sync_gate(journal, confirm_health_save=True, now=FIXED_NOW)

    payload = exc_info.value.to_dict()
    assert payload["gate_reason"] == "journal_root_binding_not_absolute"
    assert payload["invalid_fields"] == ["journal_root"]


def test_sync_gate_scheduled_requires_standing_consent(tmp_path: Path, monkeypatch):
    journal = _use_journal(tmp_path, monkeypatch)
    _write_sync_artifact(journal, _valid_sync_artifact(journal))

    # Even a per-run confirmation flag cannot stand in for the recorded
    # scheduled_sync consent — a cron job is not a person clicking yes.
    with pytest.raises(PreSaveGateError) as exc_info:
        enforce_oura_sync_gate(
            journal,
            confirm_health_save=True,
            scheduled=True,
            now=FIXED_NOW,
        )

    payload = exc_info.value.to_dict()
    assert payload["gate_reason"] == "scheduled_sync_consent_missing"
    assert payload["missing_fields"] == ["scheduled_sync"]


def test_sync_gate_scheduled_blocks_unapproved_or_missing_cadence(
    tmp_path: Path, monkeypatch
):
    journal = _use_journal(tmp_path, monkeypatch)

    artifact = _valid_sync_artifact(journal)
    artifact["scheduled_sync"] = _scheduled_sync_consent(approved=False)
    _write_sync_artifact(journal, artifact)
    with pytest.raises(PreSaveGateError) as exc_info:
        enforce_oura_sync_gate(journal, scheduled=True, now=FIXED_NOW)
    assert exc_info.value.to_dict()["gate_reason"] == "scheduled_sync_not_approved"

    artifact["scheduled_sync"] = _scheduled_sync_consent(cadence="   ")
    _write_sync_artifact(journal, artifact)
    with pytest.raises(PreSaveGateError) as exc_info:
        enforce_oura_sync_gate(journal, scheduled=True, now=FIXED_NOW)
    assert exc_info.value.to_dict()["gate_reason"] == "scheduled_sync_cadence_invalid"


@pytest.mark.parametrize(
    ("valid_until", "reason", "missing_fields", "invalid_fields"),
    [
        (
            None,
            "scheduled_sync_valid_until_missing",
            ["scheduled_sync.valid_until"],
            [],
        ),
        (
            "not-a-date",
            "scheduled_sync_valid_until_invalid",
            [],
            ["scheduled_sync.valid_until"],
        ),
        (
            "2026-08-01T00:00:00",
            "scheduled_sync_valid_until_naive",
            [],
            ["scheduled_sync.valid_until"],
        ),
        (
            "2026-07-13T11:59:59Z",
            "scheduled_sync_consent_expired",
            [],
            ["scheduled_sync.valid_until"],
        ),
        (
            "2026-07-13T12:00:00Z",
            "scheduled_sync_consent_expired",
            [],
            ["scheduled_sync.valid_until"],
        ),
    ],
)
def test_sync_gate_scheduled_valid_until_blocks_bad_or_expired_values(
    tmp_path: Path,
    monkeypatch,
    valid_until: str | None,
    reason: str,
    missing_fields: list[str],
    invalid_fields: list[str],
):
    journal = _use_journal(tmp_path, monkeypatch)
    artifact = _valid_sync_artifact(journal)
    consent = _scheduled_sync_consent()
    if valid_until is None:
        consent.pop("valid_until")
    else:
        consent["valid_until"] = valid_until
    artifact["scheduled_sync"] = consent
    _write_sync_artifact(journal, artifact)

    with pytest.raises(PreSaveGateError) as exc_info:
        enforce_oura_sync_gate(journal, scheduled=True, now=FIXED_NOW)

    payload = exc_info.value.to_dict()
    assert payload["gate_reason"] == reason
    assert payload["missing_fields"] == missing_fields
    assert payload["invalid_fields"] == invalid_fields
    failure_json = json.dumps(payload, sort_keys=True)
    assert "access_token" not in failure_json
    assert "refresh_token" not in failure_json


def test_sync_gate_scheduled_passes_on_standing_consent_without_flag(
    tmp_path: Path, monkeypatch
):
    journal = _use_journal(tmp_path, monkeypatch)
    artifact = _valid_sync_artifact(journal)
    artifact["scheduled_sync"] = _scheduled_sync_consent(
        valid_until="2026-07-13T12:00:01Z"
    )
    _write_sync_artifact(journal, artifact)

    decision = enforce_oura_sync_gate(journal, scheduled=True, now=FIXED_NOW)

    assert decision.enforced is True
    assert decision.scheduled_sync is not None
    assert decision.scheduled_sync.cadence == "every 6 hours"


def test_sync_gate_rejects_health_import_artifact_schema(tmp_path: Path, monkeypatch):
    journal = _use_journal(tmp_path, monkeypatch)
    # A (valid) health *import* artifact copied to the sync artifact path
    # must not authorize sync: wrong schema, fail closed.
    _write_sync_artifact(journal, _valid_artifact(journal))

    with pytest.raises(PreSaveGateError) as exc_info:
        enforce_oura_sync_gate(journal, confirm_health_save=True, now=FIXED_NOW)

    assert exc_info.value.to_dict()["gate_reason"] == "unsupported_approval_schema"


def test_read_oura_sync_approval_is_read_only_and_lenient(tmp_path: Path, monkeypatch):
    journal = _use_journal(tmp_path, monkeypatch)
    assert read_oura_sync_approval(journal) is None

    artifact = _valid_sync_artifact(journal)
    artifact["scheduled_sync"] = _scheduled_sync_consent()
    _write_sync_artifact(journal, artifact)
    loaded = read_oura_sync_approval(journal)
    assert loaded is not None
    assert loaded["scheduled_sync"]["approved"] is True

    oura_sync_approval_path_for_journal(journal).write_text(
        "{corrupt", encoding="utf-8"
    )
    assert read_oura_sync_approval(journal) is None
