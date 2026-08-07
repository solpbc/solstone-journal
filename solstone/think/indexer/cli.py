# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Journal indexer command routing.

Write-bearing invocations and journal index reads execute through
`solstone-core indexer`; reads are routed by `native.py`. Invalid
`--rescan-file` combinations exit 64 before any side effect.
"""

import argparse
import sys
from typing import Any

from solstone.think.utils import get_journal, require_solstone, setup_cli

from .journal import (
    search_counts,
    search_journal,
)
from .native import EXIT_UNAVAILABLE, EXIT_USAGE, NativeIndexerReadError, run_native_indexer

INVALID_RESCAN_FILE_SCAN_MESSAGE = (
    "journal indexer usage error: --rescan-file cannot be combined with --rescan "
    "or --rescan-full."
)


def _format_count_column(
    items: list[tuple[str, int]], total: int, top_n: int
) -> list[str]:
    """Format a column of count items with overflow indicator."""
    lines = [f"{name} ({count})" for name, count in items[:top_n]]
    if total > top_n:
        lines.append(f"... +{total - top_n} more")
    return lines


def _display_counts(counts: dict[str, Any], top_n: int = 5) -> None:
    """Display aggregated counts in a compact table format."""
    total = counts["total"]
    facets = counts["facets"]  # Counter
    agents = counts["agents"]  # Counter
    days = counts["days"]  # Counter

    print(f"Total: {total:,} chunks\n")

    # Build columns
    facet_col = _format_count_column(facets.most_common(top_n), len(facets), top_n)
    agent_col = _format_count_column(agents.most_common(top_n), len(agents), top_n)
    day_col = _format_count_column(
        sorted(days.items(), reverse=True)[:top_n], len(days), top_n
    )

    # Header and rows
    print(f"{'Facet':<20} {'Agent':<20} {'Day':<20}")
    print("-" * 60)

    from itertools import zip_longest

    for f, a, d in zip_longest(facet_col, agent_col, day_col, fillvalue=""):
        print(f"{f:<20} {a:<20} {d:<20}")

    print()


def _display_search_results(
    results: list[dict[str, Any]], total: int, offset: int
) -> None:
    """Display search results in a consistent format."""
    if total == 0 or not results:
        print("No results found")
        return

    # Show pagination context
    start = offset + 1
    end = offset + len(results)
    print(f"Showing {start}-{end} of {total} results\n")

    for idx, r in enumerate(results, start):
        meta = r.get("metadata", {})
        text = r.get("text", "").replace("\n", " ")
        snippet = text[:100] + "..." if len(text) > 100 else text
        label = meta.get("agent") or meta.get("time") or ""
        facet = meta.get("facet")
        facet_str = f" ({facet})" if facet else ""
        print(f"{idx}. {meta.get('day')} {label}{facet_str}: {snippet}")


def _has_write_operation(args: argparse.Namespace) -> bool:
    return bool(
        args.reset
        or args.rebuild_edges
        or args.rescan
        or args.rescan_full
        or args.rescan_file
    )


def main() -> int | None:
    """Main CLI entry point for the indexer."""
    parser = argparse.ArgumentParser(
        description="Index journal content (insights, transcripts, events, entities)"
    )
    parser.add_argument(
        "--rescan",
        action="store_true",
        help="Scan and update index (light mode: today + facets/imports/apps, excludes historical days)",
    )
    parser.add_argument(
        "--rescan-full",
        action="store_true",
        help="Full rescan including all historical day directories",
    )
    parser.add_argument(
        "--rescan-file",
        metavar="PATH",
        help="Index a specific file (absolute or journal-relative path)",
    )
    parser.add_argument(
        "--rebuild-edges",
        action="store_true",
        help="Rebuild derived edge tables only",
    )
    parser.add_argument(
        "--reset",
        action="store_true",
        help="Remove the index before rescan",
    )
    parser.add_argument(
        "--day",
        help="Filter search results by exact YYYYMMDD day",
    )
    parser.add_argument(
        "--day-from",
        help="Filter search results by date range start (YYYYMMDD, inclusive)",
    )
    parser.add_argument(
        "--day-to",
        help="Filter search results by date range end (YYYYMMDD, inclusive)",
    )
    parser.add_argument(
        "--facet",
        help="Filter search results by facet name",
    )
    parser.add_argument(
        "--agent",
        "-a",
        help="Filter search results by agent (e.g., 'flow', 'event', 'news')",
    )
    parser.add_argument(
        "--stream",
        help="Filter search results by stream name (e.g., 'archon', 'import.apple')",
    )
    parser.add_argument(
        "-q",
        "--query",
        nargs="?",
        const="",
        help="Run query (interactive mode if no query provided)",
    )
    parser.add_argument(
        "--limit",
        type=int,
        default=10,
        help="Maximum number of results to return (default: 10)",
    )
    parser.add_argument(
        "--offset",
        type=int,
        default=0,
        help="Number of results to skip for pagination (default: 0)",
    )
    parser.add_argument(
        "--top",
        type=int,
        default=5,
        help="Number of items to show per count column (default: 5)",
    )

    args = setup_cli(parser)

    if (
        not args.rescan
        and not args.rescan_full
        and not args.rescan_file
        and not args.rebuild_edges
        and not args.reset
        and args.query is None
    ):
        parser.print_help()
        return

    if args.rescan_file and (args.rescan or args.rescan_full):
        print(INVALID_RESCAN_FILE_SCAN_MESSAGE, file=sys.stderr)
        return EXIT_USAGE

    require_solstone()
    journal = get_journal()

    if _has_write_operation(args):
        native_exit = run_native_indexer(args, journal)
        if native_exit != 0 or args.query is None:
            return native_exit

    if args.query is not None:
        query_kwargs: dict[str, Any] = {}
        if args.day:
            query_kwargs["day"] = args.day
        if args.day_from:
            query_kwargs["day_from"] = args.day_from
        if args.day_to:
            query_kwargs["day_to"] = args.day_to
        if args.facet:
            query_kwargs["facet"] = args.facet
        if args.agent:
            query_kwargs["agent"] = args.agent
        if args.stream:
            query_kwargs["stream"] = args.stream

        if args.query:
            # Single query mode - show counts then results
            try:
                counts = search_counts(args.query, **query_kwargs)
                total, results = search_journal(
                    args.query, args.limit, args.offset, **query_kwargs
                )
            except NativeIndexerReadError as exc:
                print(exc, file=sys.stderr)
                return EXIT_UNAVAILABLE
            _display_counts(counts, args.top)
            _display_search_results(results, total, args.offset)
        else:
            # Interactive mode
            while True:
                try:
                    query = input("search> ").strip()
                except EOFError:
                    break
                if not query:
                    break
                try:
                    counts = search_counts(query, **query_kwargs)
                    total, results = search_journal(
                        query, args.limit, args.offset, **query_kwargs
                    )
                except NativeIndexerReadError as exc:
                    print(exc, file=sys.stderr)
                    return EXIT_UNAVAILABLE
                _display_counts(counts, args.top)
                _display_search_results(results, total, args.offset)
