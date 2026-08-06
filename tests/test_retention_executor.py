# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""The Python-to-Rust seam for removing the owner's media.

⚠ These tests use a **stub executor** rather than the real binary, deliberately. The
Rust side already has 139 tests over the real filesystem; what is untested until here
is the *seam* — how this module reads an exit code, what it does with a refusal, and
whether a missing or broken executor can be mistaken for a successful deletion.

⛔ The property every test below exists for: **nothing may look like a completed
deletion unless the executor said it completed.**
"""

from __future__ import annotations

import json
import os
import stat

import pytest

from solstone.think import retention_executor as rx

SEGMENT = ("20260701", "field.audio", "070000_17")


def _stub(tmp_path, *, exit_code: int, stdout: str) -> str:
    """A fake executor that prints `stdout` and exits `exit_code`."""
    path = tmp_path / "stub-retention"
    path.write_text(
        "#!/bin/sh\n"
        f"cat <<'JSON'\n{stdout}\nJSON\n"
        f'printf "%s" "$@" > "{tmp_path}/argv.txt"\n'
        f"exit {exit_code}\n"
    )
    path.chmod(path.stat().st_mode | stat.S_IEXEC)
    return str(path)


def _receipt(*, removed=(), not_removed=(), halted=None) -> str:
    return json.dumps(
        {
            "ok": not not_removed and halted is None,
            "outcome": {
                "targets": [
                    {
                        "target": {"day": "20260701", "stream": "field.audio", "dir": "070000_17"},
                        "removed": list(removed),
                        "not_removed": list(not_removed),
                    }
                ],
                "halted": halted,
            },
            "index": {"ok": True, "chunks": 2, "files": 1},
            "detail": {"verb": "remove-segments"},
        }
    )


def test_a_clean_removal_returns_its_receipt(tmp_path, monkeypatch):
    monkeypatch.setenv(
        rx.BIN_ENV,
        _stub(tmp_path, exit_code=0, stdout=_receipt(removed=["chronicle/a/b/c/audio.flac"])),
    )
    receipt = rx.remove_segments(str(tmp_path), [SEGMENT])
    assert rx.removed_paths(receipt) == ["chronicle/a/b/c/audio.flac"]
    assert rx.index_pruned(receipt) == {"ok": True, "chunks": 2, "files": 1}


def test_a_refusal_raises_rather_than_returning(tmp_path, monkeypatch):
    """⛔ The whole point. A partial removal must not be readable as success."""
    monkeypatch.setenv(
        rx.BIN_ENV,
        _stub(
            tmp_path,
            exit_code=rx.EXIT_REFUSED,
            stdout=_receipt(
                removed=["chronicle/a/b/c/audio.flac"],
                not_removed=[{"entry": "chronicle/a/b/c/screen.mp4", "reason": "permission denied"}],
            ),
        ),
    )
    with pytest.raises(rx.RemovalRefused) as raised:
        rx.remove_segments(str(tmp_path), [SEGMENT])
    entries = raised.value.refused.entries()
    assert entries == [
        {"entry": "chronicle/a/b/c/screen.mp4", "reason": "permission denied"}
    ]
    assert "permission denied" in str(raised.value)


def test_a_halted_run_raises_too(tmp_path, monkeypatch):
    """A halt means targets were never reached; silence would claim they were."""
    monkeypatch.setenv(
        rx.BIN_ENV,
        _stub(
            tmp_path,
            exit_code=rx.EXIT_HALTED,
            stdout=_receipt(removed=[], halted={"reason": "the journal lock was lost"}),
        ),
    )
    with pytest.raises(rx.RemovalRefused):
        rx.remove_segments(str(tmp_path), [SEGMENT])


def test_a_missing_executor_is_loud(tmp_path, monkeypatch):
    """⛔ No executor means no deletion, and the caller must be told which."""
    monkeypatch.setenv(rx.BIN_ENV, str(tmp_path / "nope"))
    with pytest.raises(rx.ExecutorUnavailable, match="not an executable file"):
        rx.remove_segments(str(tmp_path), [SEGMENT])


def test_an_executor_absent_from_path_is_loud(tmp_path, monkeypatch):
    monkeypatch.delenv(rx.BIN_ENV, raising=False)
    monkeypatch.setenv("PATH", str(tmp_path))
    with pytest.raises(rx.ExecutorUnavailable, match="not on PATH"):
        rx.remove_segments(str(tmp_path), [SEGMENT])


def test_unparseable_output_is_never_success(tmp_path, monkeypatch):
    """⛔ Exit 0 with unreadable output is not a deletion. This is the trap."""
    monkeypatch.setenv(rx.BIN_ENV, _stub(tmp_path, exit_code=0, stdout="not json at all"))
    with pytest.raises(rx.ExecutorUnavailable, match="no readable receipt"):
        rx.remove_segments(str(tmp_path), [SEGMENT])


def test_a_non_object_receipt_is_rejected(tmp_path, monkeypatch):
    monkeypatch.setenv(rx.BIN_ENV, _stub(tmp_path, exit_code=0, stdout="[1, 2, 3]"))
    with pytest.raises(rx.ExecutorUnavailable, match="not an object"):
        rx.remove_segments(str(tmp_path), [SEGMENT])


def test_a_usage_error_is_not_a_refusal(tmp_path, monkeypatch):
    """A malformed request is the caller's bug, not the owner's failed deletion."""
    monkeypatch.setenv(
        rx.BIN_ENV,
        _stub(tmp_path, exit_code=rx.EXIT_USAGE, stdout='{"ok": false, "error": "bad flag"}'),
    )
    with pytest.raises(rx.ExecutorUnavailable, match="rejected the request"):
        rx.remove_segments(str(tmp_path), [SEGMENT])


def test_the_segment_spec_names_the_stream_explicitly(tmp_path, monkeypatch):
    """⛔ The default stream is spelled out, so two segments cannot parse the same."""
    monkeypatch.setenv(rx.BIN_ENV, _stub(tmp_path, exit_code=0, stdout=_receipt()))
    rx.remove_segments(str(tmp_path), [("20260701", "_default", "093000_300")])
    argv = (tmp_path / "argv.txt").read_text()
    assert "20260701/_default/093000_300" in argv


def test_no_segments_is_a_caller_error_not_a_no_op(tmp_path, monkeypatch):
    monkeypatch.setenv(rx.BIN_ENV, _stub(tmp_path, exit_code=0, stdout=_receipt()))
    with pytest.raises(ValueError):
        rx.remove_segments(str(tmp_path), [])


def test_the_instant_is_supplied_by_the_caller(tmp_path, monkeypatch):
    """The executor refuses the clock; the caller chooses one instant and records it."""
    monkeypatch.setenv(rx.BIN_ENV, _stub(tmp_path, exit_code=0, stdout=_receipt()))
    rx.remove_segments(str(tmp_path), [SEGMENT], at="2026-08-05T22:00:00Z")
    argv = (tmp_path / "argv.txt").read_text()
    assert "2026-08-05T22:00:00Z" in argv


def test_the_default_stamp_is_rfc3339_utc():
    stamp = rx.now_stamp()
    assert stamp.endswith("Z"), stamp
    assert "T" in stamp and "+" not in stamp, stamp


def test_the_reason_reaches_the_executor(tmp_path, monkeypatch):
    monkeypatch.setenv(rx.BIN_ENV, _stub(tmp_path, exit_code=0, stdout=_receipt()))
    rx.remove_segments(str(tmp_path), [SEGMENT], reason="policy")
    assert "policy" in (tmp_path / "argv.txt").read_text()


def test_an_unrecognised_exit_code_is_not_success(tmp_path, monkeypatch):
    """Any code the executor does not define must not read as a deletion."""
    monkeypatch.setenv(rx.BIN_ENV, _stub(tmp_path, exit_code=99, stdout=_receipt()))
    with pytest.raises(rx.ExecutorUnavailable):
        rx.remove_segments(str(tmp_path), [SEGMENT])
