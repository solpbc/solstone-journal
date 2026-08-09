"""Falsification coverage for the speaker-identity durable-write gate."""

from __future__ import annotations

import json
from dataclasses import replace
from pathlib import Path

from scripts import check_speaker_identity_cutover as gate


def _write(root: Path, relative: str, text: str) -> None:
    path = root / relative
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8")


def _write_census(root: Path, entries: list[gate.CensusEntry]) -> Path:
    path = root / "census.json"
    path.write_text(
        json.dumps(gate.census_payload(entries), indent=2),
        encoding="utf-8",
    )
    return path


def test_direct_guarded_writer_is_reported(tmp_path: Path) -> None:
    _write(
        tmp_path,
        "violator.py",
        "from journal_io import update_npz\n"
        "update_npz('entities/x/voiceprints.npz', lambda current: current)\n",
    )
    census = _write_census(tmp_path, [])

    findings = gate.check(tmp_path, census, all_files=True)

    assert any(
        finding.rule == "unexpected-speaker-identity-access"
        and "update_npz -> entities/<id>/voiceprints.npz" in finding.detail
        for finding in findings
    )


def test_clean_native_transport_and_explicit_roles_pass(tmp_path: Path) -> None:
    _write(
        tmp_path,
        "native.py",
        "def write_speaker_identity(request):\n    return request\n",
    )
    _write(
        tmp_path,
        "runtime.py",
        "from native import write_speaker_identity\n"
        "write_speaker_identity({'target': 'entities/x/voiceprints.npz'})\n",
    )
    _write(
        tmp_path,
        "scripts/entity_corpus.py",
        "SPEAKER_IDENTITY_CUTOVER_ROLE = 'differential_fixture_oracle_builder'\n"
        "from voiceprints import save_voiceprints_batch\n"
        "save_voiceprints_batch('entities/x/voiceprints.npz')\n",
    )
    _write(
        tmp_path,
        "scripts/build_core_fixtures.py",
        "import entity_corpus\n",
    )
    _write(
        tmp_path,
        "Makefile",
        "core-fixtures:\n\tpython scripts/build_core_fixtures.py\n"
        "check-core-fixtures:\n\tpython scripts/build_core_fixtures.py --check\n",
    )
    _write(
        tmp_path,
        "solstone/think/entities/merge.py",
        "from attribution import update_speaker_labels\n"
        "def _apply_segment_plan():\n"
        "    update_speaker_labels('chronicle/d/s/k/talents/speaker_labels.json', None)\n",
    )
    _write(
        tmp_path,
        "tests/verify_speaker_verdict.py",
        "from voiceprints import normalize_embedding\nnormalize_embedding([1.0])\n",
    )

    entries, findings, _roles = gate.scan(tmp_path, all_files=True)
    assert not findings
    expected: list[gate.CensusEntry] = []
    for entry in entries:
        if entry.classification == "read":
            expected.append(entry)
        elif entry.file == "scripts/entity_corpus.py":
            expected.append(replace(entry, role=gate.FIXTURE_ROLE))
        elif entry.file == "solstone/think/entities/merge.py":
            expected.append(replace(entry, role=gate.LANE_B_ROLE))
    census = _write_census(tmp_path, expected)

    assert gate.check(tmp_path, census, all_files=True) == []

    _write(tmp_path, "solstone/runtime.py", "import entity_corpus\n")
    assert any(
        finding.rule == "invalid-differential-fixture-role"
        for finding in gate.check(tmp_path, census, all_files=True)
    )


def test_native_wrapper_name_collision_does_not_flag_its_callers(
    tmp_path: Path,
) -> None:
    _write(
        tmp_path,
        "transport.py",
        "def restore_label_rows(request):\n    return request\n",
    )
    _write(
        tmp_path,
        "product.py",
        "import transport as native_speakers\n"
        "def restore_label_rows(segment, rows):\n"
        "    return native_speakers.restore_label_rows({'segment': segment, 'rows': rows})\n",
    )
    _write(
        tmp_path,
        "caller.py",
        "from product import restore_label_rows\n"
        "restore_label_rows('chronicle/d/s/k', [])\n",
    )
    census = _write_census(tmp_path, [])

    assert gate.check(tmp_path, census, all_files=True) == []
