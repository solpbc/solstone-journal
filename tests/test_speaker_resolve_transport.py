# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Tests for the speaker-identity native transport."""

from __future__ import annotations

import json
import subprocess
from pathlib import Path
from typing import Any

import pytest

from solstone.apps.speakers import speaker_resolve_transport
from solstone.think import core_handshake


def _ok() -> core_handshake.CoreHandshakeResult:
    return core_handshake.CoreHandshakeResult("ok")


def _transport_kwargs(runner) -> dict[str, Any]:
    return {
        "handshake_checker": _ok,
        "helper_locator": lambda: Path("/tmp/bin/solstone-core"),
        "native_runner": runner,
        "platform_covered": lambda: True,
    }


def _runner(returncode: int, *, stdout: str = '{"status":"written"}', stderr: str = ""):
    def run(argv, *, input: str, capture_output: bool, text: bool, check: bool):
        assert argv[:2] == ["/tmp/bin/solstone-core", "speaker-resolve"]
        assert json.loads(input)
        assert capture_output is True
        assert text is True
        assert check is False
        return subprocess.CompletedProcess(
            argv, returncode, stdout=stdout, stderr=stderr
        )

    return run


@pytest.mark.parametrize(
    ("invoke", "returncode", "message"),
    [
        (
            lambda kwargs: speaker_resolve_transport.append_correction(
                "/tmp/journal",
                Path("/tmp/journal/chronicle/20260101/audio/segment"),
                {"sentence_id": 1, "speaker": "person"},
                **kwargs,
            ),
            64,
            speaker_resolve_transport.NATIVE_USAGE_MESSAGE,
        ),
    ],
)
def test_exit_codes_raise_owner_facing_errors(
    invoke, returncode: int, message: str
) -> None:
    with pytest.raises(speaker_resolve_transport.NativeSpeakerResolveError) as exc_info:
        invoke(
            _transport_kwargs(_runner(returncode, stderr='{"detail":"native detail"}'))
        )

    assert exc_info.value.message == message
    assert exc_info.value.detail == "native detail"
    assert exc_info.value.exit_code == returncode


def test_composed_failure_warns_that_prior_operations_may_have_run(
    capsys: pytest.CaptureFixture[str],
) -> None:
    with pytest.raises(speaker_resolve_transport.NativeSpeakerResolveError):
        speaker_resolve_transport._run_speaker_resolve(
            "write-full-labels",
            {"schema": "test"},
            operation_count=2,
            **_transport_kwargs(_runner(75)),
        )

    assert speaker_resolve_transport.COMPOSED_COMMAND_WARNING in capsys.readouterr().err


def test_handshake_skip_and_fail_have_distinct_messages() -> None:
    skipped = {
        **_transport_kwargs(_runner(0)),
        "handshake_checker": lambda: core_handshake.CoreHandshakeResult(
            "skip", "source"
        ),
    }
    with pytest.raises(speaker_resolve_transport.NativeSpeakerResolveError) as skip:
        speaker_resolve_transport._run_speaker_resolve("write-full-labels", {}, **skipped)
    assert skip.value.message == speaker_resolve_transport.HANDSHAKE_SKIP_MESSAGE

    failed = {
        **_transport_kwargs(_runner(0)),
        "handshake_checker": lambda: core_handshake.CoreHandshakeResult(
            "fail", "repair"
        ),
    }
    with pytest.raises(speaker_resolve_transport.NativeSpeakerResolveError) as fail:
        speaker_resolve_transport._run_speaker_resolve("write-full-labels", {}, **failed)
    assert fail.value.message == speaker_resolve_transport.HANDSHAKE_FAIL_MESSAGE.format(message="repair")


def test_unsupported_platform_raises_named_error_without_launch() -> None:
    def unexpected_runner(*_args, **_kwargs):
        raise AssertionError("native runner should not be called")

    with pytest.raises(speaker_resolve_transport.NativeSpeakerResolveError) as exc_info:
        speaker_resolve_transport._run_speaker_resolve(
            "write-full-labels",
            {},
            **{
                **_transport_kwargs(unexpected_runner),
                "platform_covered": lambda: False,
            },
        )

    assert exc_info.value.message == speaker_resolve_transport.UNSUPPORTED_HOST_MESSAGE
    assert exc_info.value.reason == "unsupported-host"


def test_label_wrapper_derives_segment_and_builds_exact_request() -> None:
    requests: list[dict[str, Any]] = []

    def runner(argv, *, input: str, capture_output: bool, text: bool, check: bool):
        assert argv == [
            "/tmp/bin/solstone-core",
            "speaker-resolve",
            "write-full-labels",
        ]
        assert capture_output is True
        assert text is True
        assert check is False
        requests.append(json.loads(input))
        return subprocess.CompletedProcess(argv, 0, stdout='{"status":"written"}')

    response = speaker_resolve_transport.write_full_labels(
        "/tmp/journal",
        Path("/tmp/journal/chronicle/20260101/audio/segment/talents"),
        [{"sentence_id": 1, "speaker": "person"}],
        {"source": "audio"},
        **_transport_kwargs(runner),
    )

    assert response == {"status": "written"}
    assert requests == [
        {
            "schema": "solstone-speaker-resolve-write-full-labels-request-v1",
            "journal_root": "/tmp/journal",
            "segment": {
                "day": "20260101",
                "stream": "audio",
                "segment_key": "segment",
            },
            "labels": [{"sentence_id": 1, "speaker": "person"}],
            "metadata": {"source": "audio"},
        }
    ]


def test_native_label_write_creates_fixture_segment_artifact(
    journal_copy: Path,
) -> None:
    """A real native speaker-resolve write reaches the canonical segment path."""
    segment_dir = journal_copy / "chronicle" / "20240101" / "default" / "123456_300"
    labels_path = segment_dir / "talents" / "speaker_labels.json"

    response = speaker_resolve_transport.write_full_labels(
        journal_copy,
        segment_dir,
        [
            {
                "sentence_id": 1,
                "speaker": "fixture_speaker",
                "confidence": "high",
                "method": "user_assigned",
            }
        ],
        {
            "owner_centroid_last_refreshed_at": None,
            "voiceprint_versions": {},
        },
        handshake_checker=_ok,
        helper_locator=lambda: Path("core/target/debug/solstone-core").resolve(),
        platform_covered=lambda: True,
    )

    assert response == {"status": "written"}
    assert json.loads(labels_path.read_text(encoding="utf-8")) == {
        "labels": [
            {
                "sentence_id": 1,
                "speaker": "fixture_speaker",
                "confidence": "high",
                "method": "user_assigned",
            }
        ],
        "owner_centroid_last_refreshed_at": None,
        "voiceprint_versions": {},
        "candidate_evidence": [],
    }
