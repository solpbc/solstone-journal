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
    conn = sqlite3.connect(f"file:{db_path}?mode=ro", uri=True)
    try:
        rows = conn.execute("PRAGMA table_info(chunks)").fetchall()
        if not rows:
            return None
        return {row[1] for row in rows}
    except Exception:
        return None
    finally:
        conn.close()


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
    print("Rebuilding the index with the native writer...")
    from solstone.think.indexer.native import run_native_indexer_reset_and_full_scan

    run_native_indexer_reset_and_full_scan(journal)
    print("Native index rebuild complete")

    return True


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    setup_cli(parser)

    journal = get_journal()
    migrate(journal)


if __name__ == "__main__":
    main()
