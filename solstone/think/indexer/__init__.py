# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Indexer package for journal content.

This module provides the unified journal index for all content types.
"""

# Import from cli
from .cli import main

# Import from journal (unified index)
from .journal import (
    ScanReport,
    get_journal_index,
    index_file,
    scan_journal,
    search_counts,
    search_entities,
    search_journal,
)

# All public functions and constants
__all__ = [
    # Journal (unified index)
    "ScanReport",
    "get_journal_index",
    "index_file",
    "scan_journal",
    "search_counts",
    "search_journal",
    "search_entities",
    # CLI
    "main",
]
