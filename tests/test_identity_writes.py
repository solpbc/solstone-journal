# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

import hashlib
import json
import os
import re
import stat
import threading
from pathlib import Path

import pytest

from solstone.think.identity import (
    STEWARD_SECTION_ATTENTION,
    STEWARD_SECTION_AUTO_REPAIRS,
    STEWARD_SECTION_STATUS,
    STEWARD_SECTION_TRENDS,
    ensure_identity_directory,
    update_identity_section,
    write_identity,
)


@pytest.fixture(autouse=True)
def _temp_journal(monkeypatch, tmp_path):
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    config_dir = tmp_path / "config"
    config_dir.mkdir()
    (config_dir / "journal.json").write_text("{}", encoding="utf-8")
    return tmp_path


def _history_path(journal_path: Path) -> Path:
    return journal_path / "identity" / "history.jsonl"


def _read_history(journal_path: Path) -> list[dict]:
    return [
        json.loads(line)
        for line in _history_path(journal_path).read_text().splitlines()
    ]


def test_write_identity_first_write(tmp_path):
    write_identity(
        "pulse.md",
        actor="test writer",
        op="replace",
        section=None,
        content="first pulse\n",
        reason="test",
    )

    pulse_path = tmp_path / "identity" / "pulse.md"
    assert pulse_path.read_text(encoding="utf-8") == "first pulse\n"

    records = _read_history(tmp_path)
    assert len(records) == 1
    record = records[0]
    assert record["before_hash"] == hashlib.sha256(b"").hexdigest()
    assert record["bytes_before"] == 0
    assert record["after_hash"] == hashlib.sha256(b"first pulse\n").hexdigest()
    assert record["bytes_after"] == len("first pulse\n".encode("utf-8"))


def test_write_identity_atomic_failure(tmp_path, monkeypatch):
    identity_dir = tmp_path / "identity"
    identity_dir.mkdir()
    target = identity_dir / "self.md"
    target.write_text("original\n", encoding="utf-8")

    def fail_replace(src, dst):
        raise OSError("replace failed")

    monkeypatch.setattr("solstone.think.identity.os.replace", fail_replace)

    with pytest.raises(OSError, match="replace failed"):
        write_identity(
            "self.md",
            actor="test writer",
            op="replace",
            section=None,
            content="updated\n",
            reason="test",
        )

    assert target.read_text(encoding="utf-8") == "original\n"
    assert not _history_path(tmp_path).exists()
    assert list(identity_dir.glob(".self.md.*.tmp")) == []


def test_write_identity_lock_serializes(tmp_path):
    def writer(actor: str, content: str) -> None:
        write_identity(
            "self.md",
            actor=actor,
            op="replace",
            section=None,
            content=content,
            reason="test",
        )

    thread_one = threading.Thread(target=writer, args=("writer-1", "first\n"))
    thread_two = threading.Thread(target=writer, args=("writer-2", "second\n"))
    thread_one.start()
    thread_two.start()
    thread_one.join()
    thread_two.join()

    final_content = (tmp_path / "identity" / "self.md").read_text(encoding="utf-8")
    assert final_content in {"first\n", "second\n"}

    records = _read_history(tmp_path)
    assert len(records) == 2
    assert {records[0]["actor"], records[1]["actor"]} == {"writer-1", "writer-2"}


def test_write_identity_history_schema(tmp_path):
    write_identity(
        "awareness.md",
        actor="schema test",
        op="replace",
        section=None,
        content="awareness\n",
        reason="test",
    )

    record = _read_history(tmp_path)[0]
    assert list(record) == [
        "ts",
        "file",
        "actor",
        "op",
        "section",
        "reason",
        "before_hash",
        "after_hash",
        "bytes_before",
        "bytes_after",
    ]
    assert record["file"] == "awareness.md"
    assert record["actor"] == "schema test"
    assert record["op"] == "replace"
    assert record["section"] is None
    assert isinstance(record["bytes_before"], int)
    assert isinstance(record["bytes_after"], int)
    assert isinstance(record["before_hash"], str)
    assert isinstance(record["after_hash"], str)
    assert re.fullmatch(r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{3}Z", record["ts"])


def test_write_identity_mode_0600(tmp_path):
    write_identity(
        "partner.md",
        actor="mode test",
        op="replace",
        section=None,
        content="partner\n",
        reason="test",
    )

    mode = stat.S_IMODE(os.stat(tmp_path / "identity" / "partner.md").st_mode)
    assert mode == 0o600


def test_update_identity_section_returns_false_no_change(tmp_path):
    identity_dir = tmp_path / "identity"
    identity_dir.mkdir()
    (identity_dir / "partner.md").write_text(
        "# partner\n\n## work patterns\nPrefers mornings\n",
        encoding="utf-8",
    )

    changed = update_identity_section(
        "partner.md",
        "work patterns",
        "Prefers mornings",
        actor="section test",
        reason="test",
    )

    assert changed is False
    assert not _history_path(tmp_path).exists()


def test_health_md_bootstrap_creates_via_ensure_identity_directory(tmp_path):
    identity_dir = ensure_identity_directory()
    health = identity_dir / "health.md"

    assert health.read_text(encoding="utf-8") == "\n".join(
        [
            STEWARD_SECTION_STATUS,
            "",
            "not yet generated",
            "",
            STEWARD_SECTION_ATTENTION,
            "",
            STEWARD_SECTION_AUTO_REPAIRS,
            "",
            STEWARD_SECTION_TRENDS,
            "",
        ]
    )
    record = [row for row in _read_history(tmp_path) if row["file"] == "health.md"][0]
    assert record["actor"] == "ensure_identity_directory"
    assert record["op"] == "create"
