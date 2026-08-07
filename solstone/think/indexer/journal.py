# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Unified journal index for all content types.

This module provides a single FTS5 index over journal content:
- Agent outputs (markdown files)
- Events (facet event JSONL)
- Entities (facet entity JSONL)
- Action logs (facet/journal-level JSONL)

All content is converted to markdown chunks via the formatters framework,
then indexed with metadata fields for filtering (day, facet, agent).
Raw audio/screen transcripts are formattable but not indexed by default.
"""

import logging
import os
import sqlite3
import time
from collections import Counter
from dataclasses import dataclass
from datetime import date
from pathlib import Path
from typing import Any, Iterable

from solstone.think.edge_sources import get_edge_source
from solstone.think.entities.journal import load_all_journal_entities
from solstone.think.entities.relationships import (
    load_all_facet_relationships_across_facets,
)
from solstone.think.formatters import (
    extract_path_metadata,
    find_formattable_files,
    format_file,
    get_formatter,
)
from solstone.think.indexer.edges import (
    _ensure_edges_schema,
    _extract_file_edges,
    delete_edges_for_path,
    discover_edge_files,
    edge_file_mtimes,
    replace_edge_file_mtime,
)
from solstone.think.indexer.native import (
    NativeIndexerReadError,
    run_native_indexer_agents,
    run_native_indexer_coverage,
    run_native_indexer_search,
)
from solstone.think.indexer.rerank_scorer import score
from solstone.think.markdown import format_markdown
from solstone.think.utils import (
    DATE_RE,
    get_journal,
    journal_relative_path,
    resolve_journal_path,
    segment_key,
    segment_parse,
)

logger = logging.getLogger(__name__)


# Database constants
INDEX_DIR = "indexer"
DB_NAME = "journal.sqlite"
ENTITY_SEARCH_WATERMARK_MTIME_PATH = "entity_search:__mtime__"
ENTITY_SEARCH_WATERMARK_COUNT_PATH = "entity_search:__count__"


@dataclass(frozen=True)
class ScanReport:
    changed: bool
    edge_rows_inserted: int


# Schema for the unified journal index
SCHEMA = [
    "CREATE TABLE IF NOT EXISTS files(path TEXT PRIMARY KEY, mtime INTEGER)",
    """
    CREATE VIRTUAL TABLE IF NOT EXISTS chunks USING fts5(
        content,
        path UNINDEXED,
        day UNINDEXED,
        facet UNINDEXED,
        agent UNINDEXED,
        stream UNINDEXED,
        idx UNINDEXED,
        time_bucket UNINDEXED
    )
    """,
]


def _ensure_schema(conn: sqlite3.Connection) -> None:
    """Create required tables if they don't exist."""
    conn.execute("DROP TABLE IF EXISTS entity_signals")
    conn.execute("DROP TABLE IF EXISTS entities")
    for statement in SCHEMA:
        conn.execute(statement)

    # Detect old schema missing time_bucket — FTS5 cannot ALTER, must rebuild
    row = conn.execute(
        "SELECT sql FROM sqlite_master WHERE type='table' AND name='chunks'"
    ).fetchone()
    if row and "time_bucket" not in row[0]:
        logger.warning(
            "Schema migration: rebuilding chunks table to add time_bucket column"
        )
        conn.execute("DROP TABLE IF EXISTS chunks")
        conn.execute("DROP TABLE IF EXISTS files")
        for statement in SCHEMA:
            conn.execute(statement)

    _ensure_edges_schema(conn)


def _time_bucket(rel: str) -> str:
    """Derive time bucket from a journal-relative path.

    Returns 'morning' (06-11), 'afternoon' (12-16), 'evening' (17-20),
    'night' (21-05), or '' for non-segment content.
    """
    start_time, _ = segment_parse(rel)
    if start_time is None:
        return ""
    hour = start_time.hour
    if 6 <= hour <= 11:
        return "morning"
    elif 12 <= hour <= 16:
        return "afternoon"
    elif 17 <= hour <= 20:
        return "evening"
    else:
        return "night"


def get_journal_index(journal: str | None = None) -> tuple[sqlite3.Connection, str]:
    """Return SQLite connection for the journal index.

    Args:
        journal: Path to journal root. Uses SOLSTONE_JOURNAL env var if not provided.

    Returns:
        Tuple of (connection, db_path)
    """
    journal = journal or get_journal()

    db_dir = os.path.join(journal, INDEX_DIR)
    os.makedirs(db_dir, exist_ok=True)
    db_path = os.path.join(db_dir, DB_NAME)

    conn = sqlite3.connect(db_path)
    conn.execute("PRAGMA journal_mode=WAL")
    conn.execute("PRAGMA synchronous=NORMAL")
    _ensure_schema(conn)

    return conn, db_path


def prune_chunks_by_stream(stream: str, journal: str | None = None) -> dict:
    """Remove all index chunks for a stream and their files rows.

    Returns ``{"chunks": <removed chunk rows>, "files": <removed files rows>}``.
    This is index maintenance, not domain processing.
    """
    conn, _ = get_journal_index(journal)
    count = conn.execute(
        "SELECT count(*) FROM chunks WHERE stream=?",
        (stream,),
    ).fetchone()[0]
    paths = [
        row[0]
        for row in conn.execute(
            "SELECT DISTINCT path FROM chunks WHERE stream=?",
            (stream,),
        ).fetchall()
    ]
    conn.execute("DELETE FROM chunks WHERE stream=?", (stream,))
    for path in paths:
        conn.execute("DELETE FROM files WHERE path=?", (path,))
    conn.commit()
    conn.close()
    return {"chunks": count, "files": len(paths)}


def delete_segment_index_rows(
    journal: str | None, rel_path: str
) -> dict[str, int | str | None]:
    """Delete index rows that reference one chronicle segment path."""
    journal_root = Path(journal or get_journal())
    db_path = journal_root / INDEX_DIR / DB_NAME
    if not db_path.exists():
        return {"chunks": 0, "files": 0, "error": None}

    try:
        conn = sqlite3.connect(db_path)
        try:
            cur = conn.execute(
                "DELETE FROM chunks WHERE path = ? OR path LIKE ?",
                (rel_path, f"{rel_path}/%"),
            )
            chunks_deleted = cur.rowcount

            cur = conn.execute(
                "DELETE FROM files WHERE path LIKE ?",
                (f"{rel_path}/%",),
            )
            files_deleted = cur.rowcount

            conn.commit()
        finally:
            conn.close()
    except sqlite3.Error as exc:
        logger.warning("Segment index row delete failed for %s: %s", rel_path, exc)
        return {"chunks": 0, "files": 0, "error": str(exc)}

    return {"chunks": chunks_deleted, "files": files_deleted, "error": None}


def index_file(journal: str, file_path: str, verbose: bool = False) -> bool:
    """Index a single file into the journal index.

    Validates that the file exists, is under the journal directory, and has
    a registered formatter and/or edge source. Formatter matches replace chunks;
    edge source matches replace derived edge rows.

    Args:
        journal: Path to journal root directory
        file_path: Absolute or journal-relative path to file
        verbose: If True, log detailed progress

    Returns:
        True if file was indexed successfully

    Raises:
        ValueError: If file is outside journal or matches neither formatter nor edge source
        FileNotFoundError: If file doesn't exist
    """
    journal_path = Path(journal).resolve()

    # Resolve file path (handle both absolute and relative)
    if os.path.isabs(file_path):
        abs_path = Path(file_path).resolve()
    else:
        abs_path = resolve_journal_path(journal_path, file_path).resolve()

    # Validate file exists
    if not abs_path.is_file():
        raise FileNotFoundError(f"File not found: {abs_path}")

    # Validate file is under journal
    try:
        rel_path = journal_relative_path(journal_path, abs_path)
    except ValueError:
        raise ValueError(f"File is outside journal directory: {abs_path}") from None

    formatter = get_formatter(rel_path)
    edge_src = get_edge_source(rel_path)
    if formatter is None and edge_src is None:
        raise ValueError(f"No formatter found for: {rel_path}")

    # Get file mtime
    mtime = int(os.path.getmtime(abs_path))

    # Index the file
    conn, _ = get_journal_index(journal)

    if formatter is not None:
        # Delete existing chunks for this file
        conn.execute("DELETE FROM chunks WHERE path=?", (rel_path,))

        if verbose:
            logger.info("Indexing %s", rel_path)

        stream = _extract_stream(journal, rel_path)
        _index_file(conn, rel_path, str(abs_path), verbose, stream=stream)

        # Update file mtime
        conn.execute("REPLACE INTO files(path, mtime) VALUES (?, ?)", (rel_path, mtime))

        # Regenerate segment chunk if file is in a segment
        parts = rel_path.replace("\\", "/").split("/")
        if len(parts) >= 4 and segment_key(parts[2]):
            rel_segment = "/".join(parts[:3])
            seg_dir = str(resolve_journal_path(journal, rel_segment))
            conn.execute("DELETE FROM chunks WHERE path=?", (rel_segment,))
            if os.path.isdir(seg_dir):
                seg_stream = _extract_stream(journal, rel_segment + "/dummy")
                _index_segment_chunks(conn, seg_dir, rel_segment, seg_stream, verbose)

    if edge_src is not None:
        delete_edges_for_path(conn, rel_path)
        result = _extract_file_edges(conn, rel_path, str(abs_path), {})
        replace_edge_file_mtime(conn, rel_path, mtime)
        logger.info(
            "edge file indexed: %s rows=%s drops=%s failed=%s skipped_invalid=%s",
            rel_path,
            result.rows_inserted,
            result.drops,
            result.failed,
            result.invalid_segment,
        )

    conn.commit()
    conn.close()

    return True


def _extract_stream(journal: str, rel: str) -> str | None:
    """Extract stream name from a journal-relative path's segment directory.

    Reads stream.json from the segment dir if the path is inside a segment
    (e.g., "20240101/142500_300/talents/facet/flow.md").

    Returns stream name string or None for non-segment paths or pre-stream segments.
    """
    from solstone.think.streams import read_segment_stream

    parts = rel.replace("\\", "/").split("/")
    # Segment paths: parts[0]=day, parts[1]=stream, parts[2]=segment, parts[3+]=file
    if len(parts) >= 3 and segment_key(parts[2]):
        seg_dir = str(resolve_journal_path(journal, "/".join(parts[:3])))
        marker = read_segment_stream(seg_dir)
        if marker:
            return marker.get("stream")
    return None


def _index_file(
    conn: sqlite3.Connection,
    rel: str,
    path: str,
    verbose: bool,
    stream: str | None = None,
) -> None:
    """Index a single file into the chunks table.

    Uses format_file() to convert content to markdown chunks,
    then inserts each chunk with metadata.

    Metadata is sourced from two places:
    - Path-derived: day and facet from extract_path_metadata()
    - Formatter-provided: agent from meta["indexer"]["agent"]
    For markdown files, agent is also path-derived.
    """
    try:
        chunks, meta = format_file(path)
    except (ValueError, FileNotFoundError) as e:
        logger.warning("Skipping %s: %s", rel, e)
        return

    # Get path-derived metadata (day, facet, agent for .md files)
    path_meta = extract_path_metadata(rel)

    # Get formatter-provided metadata (agent for JSONL files)
    formatter_indexer = meta.get("indexer", {})

    # Merge: formatter values override path values, normalize to lowercase
    day = formatter_indexer.get("day") or path_meta["day"]
    facet = (formatter_indexer.get("facet") or path_meta["facet"]).lower()
    agent = (formatter_indexer.get("agent") or path_meta["agent"]).lower()

    if verbose:
        logger.info(
            "  %s chunks, day=%s, facet=%s, agent=%s, stream=%s",
            len(chunks),
            day,
            facet,
            agent,
            stream,
        )

    for idx, chunk in enumerate(chunks):
        content = chunk.get("markdown", "")
        if not content:
            continue

        conn.execute(
            "INSERT INTO chunks(content, path, day, facet, agent, stream, idx, time_bucket) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            (content, rel, day, facet, agent, stream, idx, _time_bucket(rel)),
        )


def _index_segment_chunks(
    conn: sqlite3.Connection,
    segment_dir: str,
    rel_segment: str,
    stream: str | None,
    verbose: bool,
) -> int:
    """Index concatenated markdown content for one segment."""
    segment_path = Path(segment_dir)
    talent_files = sorted(
        [
            *segment_path.glob("talents/*.md"),
            *segment_path.glob("talents/*/*.md"),
        ],
        key=lambda path: str(path),
    )
    if not talent_files:
        return 0

    content = "\n\n---\n\n".join(
        path.read_text(encoding="utf-8") for path in talent_files
    )
    chunks, _meta = format_markdown(content)
    day = rel_segment.replace("\\", "/").split("/")[0]

    inserted = 0
    for idx, chunk in enumerate(chunks):
        chunk_content = chunk.get("markdown", "")
        if not chunk_content:
            continue
        conn.execute(
            "INSERT INTO chunks(content, path, day, facet, agent, stream, idx, time_bucket) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            (
                chunk_content,
                rel_segment,
                day,
                "",
                "segment",
                stream,
                idx,
                _time_bucket(rel_segment),
            ),
        )
        inserted += 1

    if verbose:
        logger.info(
            "  %s segment chunks, path=%s, stream=%s", inserted, rel_segment, stream
        )

    return inserted


def _is_historical_day(rel_path: str) -> bool:
    """Check if path is in a historical YYYYMMDD directory (before today).

    Returns True for paths like "20240101/..." where the date is before today.
    Returns False for non-day paths (facets/, imports/, apps/) or today/future.
    """
    from datetime import datetime

    if not rel_path or "/" not in rel_path:
        return False

    first_part = rel_path.split("/")[0]
    if not DATE_RE.fullmatch(first_part):
        return False  # Not a day directory

    today = datetime.now().strftime("%Y%m%d")
    return first_part < today


def _ts_to_day(ts_value: str | int | None) -> str:
    """Convert a millisecond timestamp to YYYYMMDD string.

    Returns empty string if the value is missing or unparseable.
    """
    if ts_value is None:
        return ""
    try:
        ms = int(ts_value)
        if ms <= 0:
            return ""
        return date.fromtimestamp(ms / 1000).strftime("%Y%m%d")
    except (ValueError, TypeError, OSError):
        return ""


def _entity_search_watermark(journal: Path) -> tuple[float, int]:
    """Return (max_mtime, file_count) for entity_search source files."""
    max_mtime = 0.0
    count = 0
    entities_dir = journal / "entities"
    if entities_dir.is_dir():
        for slug_dir in entities_dir.iterdir():
            if not slug_dir.is_dir():
                continue
            entity_file = slug_dir / "entity.json"
            if entity_file.is_file():
                mtime = entity_file.stat().st_mtime
                if mtime > max_mtime:
                    max_mtime = mtime
                count += 1

    facets_dir = journal / "facets"
    if facets_dir.is_dir():
        for facet_dir in facets_dir.iterdir():
            if not facet_dir.is_dir():
                continue
            rel_root = facet_dir / "entities"
            if not rel_root.is_dir():
                continue
            for slug_dir in rel_root.iterdir():
                if not slug_dir.is_dir():
                    continue
                relationship_file = slug_dir / "entity.json"
                if relationship_file.is_file():
                    mtime = relationship_file.stat().st_mtime
                    if mtime > max_mtime:
                        max_mtime = mtime
                    count += 1

    return max_mtime, count


def _index_entity_search_chunks(conn: sqlite3.Connection) -> int:
    """Generate FTS5 search chunks from entity domain files.

    Combines identity records (name, type, aka) with relationship records
    (description, tags, facet) to create searchable chunks for each entity.
    One chunk per entity-facet relationship, plus one for identity-only entities.

    Returns the number of entity chunks indexed.
    """
    # Clean up: remove previous entity search chunks and legacy formatter chunks
    conn.execute("DELETE FROM chunks WHERE agent='entity'")
    conn.execute("DELETE FROM chunks WHERE path LIKE 'entity_search:%'")
    conn.execute("DELETE FROM chunks WHERE path LIKE 'entities/%/entity.json'")

    identities = load_all_journal_entities()
    all_relationships = load_all_facet_relationships_across_facets()
    relationships: dict[str, list[tuple[str, dict[str, Any]]]] = {
        entity_id: [
            (facet, relationship)
            for facet, relationship in facet_relationships
            if not relationship.get("detached")
        ]
        for entity_id, facet_relationships in all_relationships.items()
    }

    count = 0
    for entity_id, identity in identities.items():
        if identity.get("blocked"):
            continue

        name = identity.get("name") or entity_id.replace("_", " ").title()
        etype = identity.get("type") or "Unknown"
        aka_list = identity.get("aka") or []

        # Build common identity lines (included in every chunk for this entity)
        identity_lines = [f"{name} ({etype})"]
        if isinstance(aka_list, list) and aka_list:
            identity_lines.append(f"Also known as: {', '.join(aka_list)}")

        path = f"entity_search:{entity_id}"
        rels = relationships.get(entity_id, [])

        if rels:
            # One chunk per facet relationship, enriched with identity data
            for idx, (facet_name, rel) in enumerate(rels):
                lines = list(identity_lines)
                if rel.get("description"):
                    lines.append(rel["description"])
                tags_list = rel.get("tags") or []
                if isinstance(tags_list, list) and tags_list:
                    lines.append(f"Tags: {', '.join(tags_list)}")

                content = "\n".join(lines)
                facet = facet_name.lower()

                # Best available day: last_seen > updated_at > attached_at
                day = ""
                last_seen = rel.get("last_seen")
                if (
                    isinstance(last_seen, str)
                    and len(last_seen) == 8
                    and last_seen.isdigit()
                ):
                    day = last_seen
                else:
                    day = _ts_to_day(rel.get("updated_at")) or _ts_to_day(
                        rel.get("attached_at")
                    )

                conn.execute(
                    "INSERT INTO chunks(content, path, day, facet, agent, stream, idx, time_bucket) "
                    "VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
                    (content, path, day, facet, "entity", "", idx, ""),
                )
                count += 1
        else:
            # Identity-only entity — one chunk with no facet
            content = "\n".join(identity_lines)
            day = _ts_to_day(identity.get("updated_at")) or _ts_to_day(
                identity.get("created_at")
            )
            conn.execute(
                "INSERT INTO chunks(content, path, day, facet, agent, stream, idx, time_bucket) "
                "VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
                (content, path, day, "", "entity", "", 0, ""),
            )
            count += 1

    conn.commit()
    logger.info("%s entity search chunks indexed", count)
    return count


def scan_journal(journal: str, verbose: bool = False, full: bool = False) -> ScanReport:
    """Scan and index journal content.

    Args:
        journal: Path to journal root directory
        verbose: If True, log detailed progress
        full: If True, scan all files. If False (default), exclude historical
            YYYYMMDD directories (before today) for lighter incremental scans.

    Returns:
        Report with whether any rows changed and how many edge rows were inserted.
    """
    conn, db_path = get_journal_index(journal)
    journal_path = Path(journal)
    files = find_formattable_files(journal)

    # Light mode: exclude historical day directories
    if not full:
        files = {
            rel: path for rel, path in files.items() if not _is_historical_day(rel)
        }

    logger.info("Scanning %s files...", len(files))

    # Get current file mtimes from database
    db_mtimes = {
        path: mtime
        for path, mtime in conn.execute(
            "SELECT path, mtime FROM files "
            "WHERE path NOT LIKE 'entity:%' "
            "AND path NOT LIKE 'signal:%' "
            "AND path NOT LIKE 'entity_search:%'"
        )
    }

    to_index = []
    for rel, path in files.items():
        try:
            mtime = int(os.path.getmtime(path))
        except OSError:
            continue
        if db_mtimes.get(rel) != mtime:
            to_index.append((rel, path, mtime))

    cached = len(files) - len(to_index)
    logger.info(
        "%s total files, %s cached, %s to index", len(files), cached, len(to_index)
    )

    start = time.time()

    for i, (rel, path, mtime) in enumerate(to_index, 1):
        if verbose:
            logger.info("[%s/%s] %s", i, len(to_index), rel)

        # Delete existing chunks for this file
        conn.execute("DELETE FROM chunks WHERE path=?", (rel,))

        # Index the file
        stream = _extract_stream(journal, rel)
        _index_file(conn, rel, path, verbose, stream=stream)

        # Update file mtime
        conn.execute("REPLACE INTO files(path, mtime) VALUES (?, ?)", (rel, mtime))

    # Remove files that no longer exist
    # In full mode: remove all missing entries
    # In light mode: only remove entries that would have been scanned (non-historical)
    removed: set[str] = set()
    if full:
        removed = set(db_mtimes) - set(files)
    else:
        # Filter db entries to those in light mode's scan scope, then find missing
        in_scope_db = {rel for rel in db_mtimes if not _is_historical_day(rel)}
        removed = in_scope_db - set(files)

    for rel in removed:
        conn.execute("DELETE FROM chunks WHERE path=?", (rel,))
        conn.execute("DELETE FROM files WHERE path=?", (rel,))

    if to_index or removed:
        conn.commit()

    elapsed = time.time() - start
    logger.info(
        "%s indexed, %s removed in %.2f seconds", len(to_index), len(removed), elapsed
    )

    # Index segment-level concatenated chunks
    affected_segments: set[str] = set()
    for rel, _path, _mtime in to_index:
        parts = rel.replace("\\", "/").split("/")
        if len(parts) >= 4 and segment_key(parts[2]):
            affected_segments.add("/".join(parts[:3]))
    for rel in removed:
        parts = rel.replace("\\", "/").split("/")
        if len(parts) >= 4 and segment_key(parts[2]):
            affected_segments.add("/".join(parts[:3]))

    seg_count = 0
    for rel_segment in sorted(affected_segments):
        segment_dir = str(resolve_journal_path(journal, rel_segment))
        conn.execute("DELETE FROM chunks WHERE path=?", (rel_segment,))
        if os.path.isdir(segment_dir):
            stream = _extract_stream(journal, rel_segment + "/dummy")
            seg_count += _index_segment_chunks(
                conn, segment_dir, rel_segment, stream, verbose
            )

    if affected_segments:
        conn.commit()
        logger.info(
            "%s segment chunks indexed for %s segments",
            seg_count,
            len(affected_segments),
        )

    fresh_mtime, fresh_count = _entity_search_watermark(journal_path)
    stored_mtime_row = conn.execute(
        "SELECT mtime FROM files WHERE path=?",
        (ENTITY_SEARCH_WATERMARK_MTIME_PATH,),
    ).fetchone()
    stored_count_row = conn.execute(
        "SELECT mtime FROM files WHERE path=?",
        (ENTITY_SEARCH_WATERMARK_COUNT_PATH,),
    ).fetchone()
    stored_mtime = stored_mtime_row[0] if stored_mtime_row else 0.0
    stored_count = int(stored_count_row[0]) if stored_count_row else 0
    has_entity_chunks = (
        conn.execute("SELECT 1 FROM chunks WHERE agent='entity' LIMIT 1").fetchone()
        is not None
    )
    entity_changed = (
        fresh_mtime > stored_mtime
        or fresh_count != stored_count
        or (fresh_count > 0 and not has_entity_chunks)
    )
    if entity_changed:
        _index_entity_search_chunks(conn)
        conn.execute(
            "REPLACE INTO files(path, mtime) VALUES (?, ?)",
            (ENTITY_SEARCH_WATERMARK_MTIME_PATH, fresh_mtime),
        )
        conn.execute(
            "REPLACE INTO files(path, mtime) VALUES (?, ?)",
            (ENTITY_SEARCH_WATERMARK_COUNT_PATH, float(fresh_count)),
        )
        conn.commit()

    edge_files = discover_edge_files(journal)
    if not full:
        edge_files = {
            rel: path for rel, path in edge_files.items() if not _is_historical_day(rel)
        }

    db_edge_mtimes = edge_file_mtimes(conn)
    edge_to_index = []
    for rel, path in edge_files.items():
        try:
            mtime = int(os.path.getmtime(path))
        except OSError:
            continue
        if db_edge_mtimes.get(rel) != mtime:
            edge_to_index.append((rel, path, mtime))

    if full:
        edge_removed = set(db_edge_mtimes) - set(edge_files)
    else:
        in_scope_edge_db = {
            rel for rel in db_edge_mtimes if not _is_historical_day(rel)
        }
        edge_removed = in_scope_edge_db - set(edge_files)

    edge_cache: dict[str, list[dict[str, Any]]] = {}
    edge_rows_inserted = 0
    edge_rows_removed = 0
    edge_drops = 0
    edge_skipped_invalid = 0

    for rel, path, mtime in edge_to_index:
        edge_rows_removed += delete_edges_for_path(conn, rel)
        result = _extract_file_edges(conn, rel, path, edge_cache)
        edge_rows_inserted += result.rows_inserted
        edge_drops += result.drops
        edge_skipped_invalid += int(result.invalid_segment)
        replace_edge_file_mtime(conn, rel, mtime)

    for rel in edge_removed:
        edge_rows_removed += delete_edges_for_path(conn, rel)

    edge_changed = bool(edge_to_index or edge_removed)
    if edge_changed:
        conn.commit()

    logger.info(
        "%s edge files indexed, %s edge files removed, %s edge rows inserted, "
        "%s edge rows removed, %s edge drops, %s edge files skipped (invalid segment)",
        len(edge_to_index),
        len(edge_removed),
        edge_rows_inserted,
        edge_rows_removed,
        edge_drops,
        edge_skipped_invalid,
    )

    conn.close()
    return ScanReport(
        changed=bool(to_index or removed or entity_changed or edge_changed),
        edge_rows_inserted=edge_rows_inserted,
    )


def search_journal(
    query: str,
    limit: int = 10,
    offset: int = 0,
    *,
    day: str | None = None,
    day_from: str | None = None,
    day_to: str | None = None,
    facet: str | None = None,
    agent: str | None = None,
    stream: str | None = None,
    time_bucket: str | None = None,
    relax: bool = False,
    rerank: bool = False,
    include_total: bool = True,
    degraded_out: dict[str, Any] | None = None,
) -> tuple[int, list[dict[str, Any]]]:
    """Search the journal index.

    Args:
        query: FTS5 search query. Words are AND'd by default; use OR to match any,
            quotes for exact phrases, * for prefix match. Empty string returns all.
        limit: Maximum results to return
        offset: Number of results to skip for pagination
        day: Filter by exact day (YYYYMMDD) - mutually exclusive with day_from/day_to
        day_from: Filter by date range start (YYYYMMDD, inclusive)
        day_to: Filter by date range end (YYYYMMDD, inclusive)
        facet: Filter by facet name
        agent: Filter by agent (e.g., "flow", "event", "news")
        stream: Filter by stream name
        time_bucket: Filter by time of day (morning, afternoon, evening, night)
        relax: If True, try a recall-oriented relaxation ladder when tier-0 FTS
            returns zero matches and the query has no power-user syntax.
        rerank: If True, reorder the top FTS pool with the local rerank scorer
            when available.

    Returns:
        Tuple of (total_count, results) where each result has:
            - id: "{path}:{idx}"
            - text: The matched markdown chunk
            - metadata: {day, facet, agent, stream, path, idx}
            - score: BM25 relevance score
            - rerank_score: Optional rerank score on reranked result pages
    """
    if degraded_out is not None:
        degraded_out.clear()
    used_pool_fetch = rerank and offset + limit <= 50
    native_limit, native_offset = (50, 0) if used_pool_fetch else (limit, offset)
    try:
        response = run_native_indexer_search(
            query,
            str(get_journal()),
            limit=native_limit,
            offset=native_offset,
            day=day,
            day_from=day_from,
            day_to=day_to,
            facet=facet,
            agent=agent,
            stream=stream,
            time_bucket=time_bucket,
            relax=relax,
            include_counts=include_total,
        )
    except NativeIndexerReadError as exc:
        if exc.reason == "empty_index":
            return 0, []
        raise

    if degraded_out is not None:
        degraded = response.get("degraded")
        if degraded is not None:
            degraded_out.update(degraded)
    if response.get("reason") == "not_tokenizable":
        return 0, []
    results = _decode_search_results(response["results"])
    if used_pool_fetch:
        if response["order"] == "relevance" and len(results) > 1:
            scores = score(
                response["cleaned_query"],
                [result["text"] for result in results],
            )
            if scores is not None:
                order = sorted(range(len(results)), key=lambda index: -scores[index])
                results = [
                    {**results[index], "rerank_score": scores[index]} for index in order
                ]
        results = results[offset : offset + limit]
    total = int(response["total"]) if include_total else 0
    return total, results


def search_counts(
    query: str,
    *,
    day: str | None = None,
    day_from: str | None = None,
    day_to: str | None = None,
    facet: str | None = None,
    agent: str | None = None,
    stream: str | None = None,
    time_bucket: str | None = None,
    relax: bool = False,
) -> dict[str, Any]:
    """Get aggregated counts for a search query.

    Routes a limit-zero native search through ``run_native_indexer_search`` and
    decodes its Rust ``counts`` aggregation.

    Args:
        query: FTS5 search query (empty string for all)
        day: Filter by exact day (YYYYMMDD) - mutually exclusive with day_from/day_to
        day_from: Filter by date range start (YYYYMMDD, inclusive)
        day_to: Filter by date range end (YYYYMMDD, inclusive)
        facet: Filter by facet name
        agent: Filter by agent
        stream: Filter by stream name
        time_bucket: Filter by time of day (morning, afternoon, evening, night)
        relax: If True, use the same recall-oriented relaxation ladder as
            search_journal when tier-0 FTS returns zero matches.

    Returns:
        Dict with:
            - total: Total matching chunks
            - facets: Counter of facet_name -> count
            - agents: Counter of agent_name -> count
            - days: Counter of day -> count
            - streams: Counter of stream_name -> count
            - relaxed: True iff tier-0 FTS returned no matches and the relaxation
              ladder rescued the query (only possible with relax=True); False
              otherwise.
    """
    # Search exposes not_tokenizable while the counts verb intentionally does not.
    try:
        response = run_native_indexer_search(
            query,
            str(get_journal()),
            limit=0,
            offset=0,
            day=day,
            day_from=day_from,
            day_to=day_to,
            facet=facet,
            agent=agent,
            stream=stream,
            time_bucket=time_bucket,
            relax=relax,
        )
    except NativeIndexerReadError as exc:
        if exc.reason == "empty_index":
            return _empty_search_counts()
        raise
    if response.get("reason") == "not_tokenizable":
        return _empty_search_counts()
    counts = response["counts"]
    return {
        "total": int(counts["total"]),
        "facets": Counter(counts["facets"]),
        "agents": Counter(counts["agents"]),
        "days": Counter(counts["days"]),
        "streams": Counter(counts["streams"]),
        "relaxed": bool(counts["relaxed"]),
        "degraded": response.get("degraded"),
    }


def get_corpus_day_coverage() -> dict[str, str] | None:
    """Return the indexed corpus day span, or None when no dated chunks exist."""
    try:
        coverage = run_native_indexer_coverage(str(get_journal()))
    except NativeIndexerReadError as exc:
        if exc.reason == "empty_index":
            return None
        raise
    if coverage["state"] == "no_dated_chunks":
        return None
    return {"start": coverage["start"], "end": coverage["end"]}


def known_agents() -> set[str]:
    """Return the distinct, non-empty agent names present in the index."""
    try:
        return set(run_native_indexer_agents(str(get_journal())))
    except NativeIndexerReadError as exc:
        if exc.reason == "empty_index":
            return set()
        raise


def _decode_search_results(hits: list[dict[str, Any]]) -> list[dict[str, Any]]:
    return [
        {
            "id": hit["id"],
            "text": hit["text"],
            "metadata": {
                key: hit["metadata"][key]
                for key in ("day", "facet", "agent", "stream", "path", "idx")
            },
            "score": hit["score"],
        }
        for hit in hits
    ]


def _empty_search_counts() -> dict[str, Any]:
    return {
        "total": 0,
        "facets": Counter(),
        "agents": Counter(),
        "days": Counter(),
        "streams": Counter(),
        "relaxed": False,
        "degraded": None,
    }


def _load_index_entity_dicts() -> list[dict[str, Any]]:
    """Load identity entities as entity dicts for name resolution.

    Returns dicts with "id", "name", and "aka" suitable for
    build_name_resolution_map().
    """
    entity_dicts: list[dict[str, Any]] = []
    for entity_id, entity in load_all_journal_entities().items():
        entity_dicts.append(
            {
                "id": entity_id,
                "name": entity.get("name") or "",
                "aka": entity.get("aka") or [],
            }
        )
    return entity_dicts


def _build_entity_name_map(
    names: Iterable[str],
) -> dict[str, str]:
    """Map entity names to entity_ids via shared name resolution.

    Returns dict mapping entity_name -> entity_id. Uses the same tiered
    matching as all other name resolution call sites.
    """
    from solstone.think.entities.matching import build_name_resolution_map

    entity_dicts = _load_index_entity_dicts()
    return build_name_resolution_map(
        sorted({name for name in names if name}), entity_dicts
    )


def _extract_match_candidates(fts_results: list[dict[str, Any]]) -> set[str]:
    """Extract candidate entity names from FTS result text."""
    names: set[str] = set()
    for result in fts_results:
        text = result.get("text", "")
        for line in text.splitlines():
            stripped = line.strip()
            if stripped.startswith("### "):
                name = stripped[4:].strip()
                if name.startswith("Project: "):
                    names.add(name[len("Project: ") :].strip())
                elif name.startswith("Person: "):
                    names.add(name[len("Person: ") :].strip())
                elif name:
                    names.add(name)
    return names


def search_entities(
    query: str | None = None,
    entity_type: str | None = None,
    facet: str | None = None,
    since: str | None = None,
    limit: int = 20,
) -> list[dict[str, Any]]:
    """Search entities by text query, type, facet, and/or detected activity."""
    entities_by_id = load_all_journal_entities()
    relationships_by_id = load_all_facet_relationships_across_facets()

    active_ids: set[str] | None = None
    if since:
        from solstone.think.entities.activity import iter_detected_entity_names_since

        detected_names = [
            name for name, _facet, _day in iter_detected_entity_names_since(since)
        ]
        name_map = _build_entity_name_map(detected_names)
        active_ids = set(name_map.values())

    if query:
        candidate_ids: list[str] = []
        seen_ids: set[str] = set()

        _, entity_results = search_journal(query, limit=100, agent="entity")
        for result in entity_results:
            path = result.get("metadata", {}).get("path", "")
            if not path.startswith("entity_search:"):
                continue
            entity_id = path.removeprefix("entity_search:")
            if entity_id and entity_id not in seen_ids:
                candidate_ids.append(entity_id)
                seen_ids.add(entity_id)

        _, detected_results = search_journal(query, limit=100, agent="entity:detected")
        for result in detected_results:
            path = result.get("metadata", {}).get("path", "")
            parts = path.split("/")
            if "entities" not in parts:
                continue
            idx = parts.index("entities")
            if idx + 1 >= len(parts):
                continue
            entity_id = parts[idx + 1]
            if entity_id and "." not in entity_id and entity_id not in seen_ids:
                candidate_ids.append(entity_id)
                seen_ids.add(entity_id)

        match_names = _extract_match_candidates(detected_results)
        for entity_id in _build_entity_name_map(match_names).values():
            if entity_id not in seen_ids:
                candidate_ids.append(entity_id)
                seen_ids.add(entity_id)
    else:
        candidate_ids = list(entities_by_id)

    if active_ids is not None:
        candidate_ids = [
            entity_id for entity_id in candidate_ids if entity_id in active_ids
        ]

    facet_filter = facet.lower() if facet else None
    type_filter = entity_type.lower() if entity_type else None
    result_list = []
    for entity_id in candidate_ids:
        entity = entities_by_id.get(entity_id)
        if not entity:
            continue

        entity_type_value = entity.get("type") or ""
        if type_filter and entity_type_value.lower() != type_filter:
            continue

        facet_relationships = relationships_by_id.get(entity_id, [])
        facets: list[str] = []
        description = ""
        for relationship_facet, relationship in facet_relationships:
            if relationship_facet and relationship_facet not in facets:
                facets.append(relationship_facet)
            if not description and relationship.get("description"):
                description = str(relationship["description"])

        if facet_filter and not any(
            relationship_facet.lower() == facet_filter for relationship_facet in facets
        ):
            continue

        result_list.append(
            {
                "entity_id": entity_id,
                "name": entity.get("name") or "",
                "type": entity_type_value,
                "description": description,
                "facets": facets,
            }
        )

    if not query:
        result_list.sort(key=lambda x: (str(x["name"]).lower(), str(x["name"])))
    return result_list[:limit]
