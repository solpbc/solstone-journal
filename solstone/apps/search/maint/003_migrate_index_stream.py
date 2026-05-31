# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Add stream column to journal search index.

Phase 2 of stream identity adds a stream column to the FTS5 index.
FTS5 virtual tables cannot be ALTERed, so if the old schema is detected
the index is dropped and recreated. A full rescan is requested via
supervisor to rebuild the index in the background.
"""

from __future__ import annotations

import argparse
import logging
import os
import sqlite3

from solstone.think.utils import get_journal, setup_cli

logger = logging.getLogger(__name__)

EXPECTED_COLUMNS = {"content", "path", "day", "facet", "agent", "stream", "idx"}


def _get_db_path(journal: str) -> str:
    return os.path.join(journal, "indexer", "journal.sqlite")


def _get_columns(db_path: str) -> set[str] | None:
    """Read column names from the chunks table. Returns None if DB or table missing."""
    if not os.path.exists(db_path):
        return None
    conn = sqlite3.connect(db_path)
    try:
        rows = conn.execute("PRAGMA table_info(chunks)").fetchall()
        if not rows:
            return None
        return {row[1] for row in rows}
    except Exception:
        return None
    finally:
        conn.close()


def _rebuild_schema(db_path: str) -> None:
    """Drop and recreate both tables with the current schema."""
    from solstone.think.indexer.journal import SCHEMA

    conn = sqlite3.connect(db_path)
    try:
        conn.execute("DROP TABLE IF EXISTS chunks")
        conn.execute("DROP TABLE IF EXISTS files")
        for statement in SCHEMA:
            conn.execute(statement)
        conn.commit()
    finally:
        conn.close()


def _request_full_rescan() -> bool:
    """Ask supervisor to queue a full index rescan via callosum."""
    from solstone.think.callosum import callosum_send

    return callosum_send(
        "supervisor",
        "request",
        cmd=["journal", "indexer", "--rescan-full"],
    )


def migrate(journal: str) -> bool:
    """Run migration. Returns True if schema was rebuilt."""
    db_path = _get_db_path(journal)
    cols = _get_columns(db_path)

    if cols is None:
        print("No existing index found, nothing to migrate")
        return False

    if EXPECTED_COLUMNS.issubset(cols):
        print("Index schema is current, no migration needed")
        return False

    missing = EXPECTED_COLUMNS - cols
    print(f"Index schema outdated (missing: {', '.join(sorted(missing))})")
    print("Dropping and recreating index tables...")
    _rebuild_schema(db_path)
    print("Schema rebuilt successfully")

    print("Requesting full rescan from supervisor...")
    if _request_full_rescan():
        print("Full rescan queued")
    else:
        print(
            "Could not reach supervisor — run 'journal indexer --rescan-full' manually"
        )

    return True


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    setup_cli(parser)

    journal = get_journal()
    migrate(journal)


if __name__ == "__main__":
    main()
