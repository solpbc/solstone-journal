# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import json

import pytest

from solstone.think import features


@pytest.fixture
def doctor():
    from solstone.think import doctor as doctor_module

    return doctor_module


def args(doctor, *, port: int = 5015, feature: str | None = None):
    return doctor.Args(
        verbose=False, json=False, jsonl=False, port=port, feature=feature
    )


def run_check(doctor, name: str):
    _check, runner = doctor.FEATURE_CHECKS[name.removeprefix("feature:")]
    return runner(args(doctor))


def test_feature_checks_registered(doctor):
    assert set(doctor.FEATURE_CHECKS) == set(features.FEATURES)


def test_feature_checks_in_check_map(doctor):
    assert "feature:pdf" in doctor.CHECK_MAP
    assert "feature:whisper" in doctor.CHECK_MAP


def test_pdf_feature_check_ok_when_available(doctor):
    result = run_check(doctor, "feature:pdf")

    assert result.status == "ok"


def test_pdf_feature_check_warns_when_missing(doctor, monkeypatch):
    monkeypatch.setattr(features, "is_available", lambda name: name != "pdf")

    result = run_check(doctor, "feature:pdf")

    assert result.status == "warn"
    assert result.fix == features.install_hint("pdf", doctor.platform_tag())


def test_parse_args_feature(doctor):
    parsed = doctor.parse_args(["--feature", "pdf"])

    assert parsed.feature == "pdf"


def test_parse_args_rejects_unknown_feature(doctor, capsys):
    with pytest.raises(SystemExit) as error:
        doctor.parse_args(["--feature", "bogus"])

    assert error.value.code == 2
    stderr = capsys.readouterr().err
    assert "known features" in stderr
    assert "pdf" in stderr
    assert "whisper" in stderr


def test_run_checks_filters_to_feature(doctor):
    results = doctor.run_checks(args(doctor, feature="whisper"))

    assert len(results) == 1
    assert results[0].name.startswith("feature:whisper")


def test_emit_json_filtered_summary(doctor, capsys):
    results = doctor.run_checks(args(doctor, feature="whisper"))

    doctor.emit_json(results)
    payload = json.loads(capsys.readouterr().out)

    assert payload["summary"]["total"] == 1
    assert len(payload["checks"]) == 1
