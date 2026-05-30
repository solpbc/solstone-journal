# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Shared isolation harness for API baseline tests and baseline regeneration.

Used by:
- tests/test_api_baselines.py - module-scoped fixtures
- tests/verify_api.py - verify/update CLI mode

Keeps both paths on identical isolation so generated baselines match the test
oracle.
"""

from __future__ import annotations

import atexit
import os
import shutil
import subprocess
import tempfile
import threading
from contextlib import contextmanager
from pathlib import Path
from typing import Iterator

FROZEN_DATE = "2026-04-15"
FROZEN_TZ_OFFSET = -7

# The fixture journal is built once per process from an immutable snapshot of
# HEAD rather than copied file-by-file from the live working tree. make dev /
# make sandbox write runtime artifacts into tests/fixtures/journal (AGENTS.md
# §6), so a concurrent writer can delete or replace a tracked file between
# `git ls-files` and the copy — raising FileNotFoundError at fixture *setup*
# (a spurious pytest ERROR, not a real failure). `git archive HEAD` reads the
# committed tree from the object store, never the working tree, so it is immune
# to concurrent working-tree mutation.
_REPO_ROOT = Path(__file__).resolve().parent.parent
_FIXTURE_JOURNAL_REL = "tests/fixtures/journal"
_LIVE_FIXTURE_JOURNAL = (_REPO_ROOT / _FIXTURE_JOURNAL_REL).resolve()

_snapshot_lock = threading.Lock()
_snapshot_root: Path | None = None


def _fixture_journal_snapshot() -> Path:
    """Return an immutable, process-scoped snapshot of the tracked fixture journal.

    Built once per process via `git archive HEAD` → tar, which reads HEAD from
    the git object store and is therefore unaffected by concurrent writes to the
    live tests/fixtures/journal tree. The same set of git-tracked files that
    `git ls-files` selects (working tree clean ⇒ HEAD ≡ index), with symlinks
    preserved as symlinks. The temp dir is removed at interpreter exit.
    """
    global _snapshot_root
    with _snapshot_lock:
        if _snapshot_root is not None and _snapshot_root.exists():
            return _snapshot_root
        tmp = Path(tempfile.mkdtemp(prefix="solstone-journal-snapshot-"))
        archive = subprocess.run(
            ["git", "archive", "HEAD", "--", _FIXTURE_JOURNAL_REL],
            cwd=str(_REPO_ROOT),
            capture_output=True,
            check=True,
        )
        subprocess.run(
            ["tar", "-x", "-C", str(tmp)],
            input=archive.stdout,
            check=True,
        )
        # `git archive` preserves the tests/fixtures/journal/ prefix in the tar.
        _snapshot_root = (tmp / _FIXTURE_JOURNAL_REL).resolve()
        atexit.register(shutil.rmtree, tmp, ignore_errors=True)
        return _snapshot_root


def copytree_tracked(src: Path, dst: Path) -> None:
    """Copy only git-tracked files from src to dst.

    For paths within tests/fixtures/journal (the journal fixture and any subtree
    of it), copy from an immutable, process-scoped snapshot of HEAD instead of
    enumerating and copying from the live working tree — see
    `_fixture_journal_snapshot` for why this defeats the concurrent-write race.
    For any other src, fall back to enumerating tracked files from the live tree.
    """
    src = Path(src).resolve()
    dst = Path(dst)
    try:
        rel = src.relative_to(_LIVE_FIXTURE_JOURNAL)
    except ValueError:
        rel = None
    if rel is not None:
        snap_src = _fixture_journal_snapshot() / rel
        shutil.copytree(snap_src, dst, symlinks=True, dirs_exist_ok=True)
        return
    result = subprocess.run(
        ["git", "ls-files", "."],
        cwd=str(src),
        capture_output=True,
        text=True,
        check=True,
    )
    for rel_path in result.stdout.splitlines():
        if not rel_path:
            continue
        src_file = src / rel_path
        dst_file = dst / rel_path
        dst_file.parent.mkdir(parents=True, exist_ok=True)
        if src_file.is_symlink():
            os.symlink(os.readlink(src_file), dst_file)
        else:
            shutil.copy2(src_file, dst_file)


def prepare_isolated_journal(dst: Path) -> Path:
    """Copy the git-tracked fixture journal into dst and return the absolute path."""
    src = Path("tests/fixtures/journal").resolve()
    dst = dst.resolve()
    copytree_tracked(src, dst)
    return dst


@contextmanager
def isolated_app_env(journal: Path) -> Iterator[Path]:
    """Patch env so create_app(journal) is fully isolated."""

    journal = Path(journal).resolve()
    prev_override = os.environ.get("SOLSTONE_JOURNAL")

    os.environ["SOLSTONE_JOURNAL"] = str(journal)
    try:
        yield journal
    finally:
        if prev_override is None:
            os.environ.pop("SOLSTONE_JOURNAL", None)
        else:
            os.environ["SOLSTONE_JOURNAL"] = prev_override


def make_logged_in_test_client(journal: Path):
    """Create a Flask test client with an authenticated session."""
    from solstone.convey import create_app

    app = create_app(journal=str(Path(journal).resolve()))
    app.config["TESTING"] = True
    client = app.test_client()
    with client.session_transaction() as session:
        session["logged_in"] = True
        session.permanent = True
    return client
