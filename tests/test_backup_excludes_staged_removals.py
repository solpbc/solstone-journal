# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""A snapshot must never capture a segment mid-removal.

The retention executor moves a segment aside under a prefix no iterator returns,
empties it there, and moves it back holding only its tombstone. A snapshot taken during
that window would capture a **partially emptied** segment at an invisible path, and a
restore would hand the owner data they cannot see or delete.

⛔ The exclusion pattern and the executor's prefix live in different languages, and
nothing else makes them agree. This pins the pattern to the committed cross-language
fixture that records the real staged name, so changing the prefix on one side fails
here rather than silently disarming the exclusion.
"""

from __future__ import annotations

import fnmatch
import json
from pathlib import Path

from solstone.think.backup.engine import BACKUP_EXCLUDES

ORACLE = (
    Path(__file__).resolve().parents[1] / "core/fixtures/segment_name_oracle.json"
)


def _staged_names() -> list[str]:
    """Every staged directory name the oracle records."""
    rows = json.loads(ORACLE.read_text())["rows"]
    return [row["name"] for row in rows if row["name"].startswith(".removing")]


def test_the_oracle_records_a_staged_name_to_test_against():
    """Without this the assertions below would loop over nothing."""
    assert _staged_names(), (
        f"{ORACLE} records no staged directory name, so the exclusion below is "
        "asserted against an empty list"
    )


def test_every_staged_segment_name_is_excluded_from_snapshots():
    staged = _staged_names()
    for name in staged:
        assert any(
            fnmatch.fnmatch(name, pattern) for pattern in BACKUP_EXCLUDES
        ), (
            f"a segment mid-removal (`{name}`) is not excluded from snapshots, so a "
            f"restore could hand the owner a partially emptied segment at a path no "
            f"iterator returns. BACKUP_EXCLUDES = {BACKUP_EXCLUDES}"
        )


#: Segment directory names the journal's writers actually produce.
#:
#: ⚠ NOT taken from the oracle's `is_segment` rows. That fixture deliberately includes
#: hostile names -- its job is to pin what the substring-scanning key parser accepts,
#: which is broader than what any writer emits. Asserting "every name the oracle calls
#: a segment must be backed up" conflates the two, and the first version of the test
#: below did exactly that and failed on the unmodified tree.
CANONICAL_SEGMENT_NAMES = ("070000_17", "093000_300", "093000_300_summary")


def test_a_real_segment_is_not_excluded():
    """⛔ The negative twin. An over-broad pattern would drop the owner's recordings.

    Without this, adding `*` to the exclusion list would satisfy the test above.
    """
    for name in CANONICAL_SEGMENT_NAMES:
        assert not any(
            fnmatch.fnmatch(name, pattern) for pattern in BACKUP_EXCLUDES
        ), f"`{name}` is an ordinary segment and must be in every snapshot"


def test_a_key_bearing_name_with_an_excluded_suffix_is_a_known_narrow_gap():
    """📌 Recorded rather than fixed: the two classifiers can disagree.

    The journal recognises a segment by SCANNING a directory name for the key pattern,
    so a directory called `070000_17.lock` is classified as a segment holding the
    owner's data -- and `*.lock` excludes it from every snapshot. Owner data at such a
    path would be silently absent from backups.

    ⚠ Narrow, because no writer produces that name; the journal's `.lock` entries are
    files, not segment directories. It is pinned here so the interaction is a recorded
    fact rather than a surprise, and so this test fails loudly if the shape ever
    becomes reachable and someone has to decide which classifier gives way.
    """
    hostile = "070000_17.lock"
    excluded = [
        pattern for pattern in BACKUP_EXCLUDES if fnmatch.fnmatch(hostile, pattern)
    ]
    assert excluded == ["*.lock"], (
        f"the known gap changed shape: `{hostile}` is now matched by {excluded}"
    )


def test_the_tombstone_is_not_excluded():
    """The owner's evidence that a deletion happened must survive a restore.

    ⚠ The exclusion list once carried a bare `health` pattern that dropped the durable
    deletion audit from every snapshot, because restic matches a no-slash pattern by
    basename at any depth. The tombstone is the same class of record.
    """
    assert not any(
        fnmatch.fnmatch("tombstone.json", pattern) for pattern in BACKUP_EXCLUDES
    ), f"the deletion record must be backed up. BACKUP_EXCLUDES = {BACKUP_EXCLUDES}"
