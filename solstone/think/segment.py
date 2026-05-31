# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""journal segment - segment inspection and management CLI."""

from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import sqlite3
import sys
from datetime import datetime, timedelta
from pathlib import Path

from solstone.think.streams import (
    get_stream_state,
    read_segment_stream,
    rebuild_stream_state,
    write_segment_stream,
)
from solstone.think.utils import (
    day_dirs,
    day_path,
    get_journal,
    iter_segments,
    require_solstone,
    segment_parse,
    setup_cli,
)


def _find_segment_dir_readonly(
    day: str, segment: str, stream: str | None
) -> Path | None:
    """Locate a segment directory without creating anything."""
    day_dir = day_path(day, create=False)
    if not day_dir.exists():
        return None
    if stream:
        candidate = day_dir / stream / segment
        return candidate if candidate.is_dir() else None
    for _stream_name, _seg_key, seg_path in iter_segments(day):
        if seg_path.name == segment:
            return seg_path
    return None


def _format_size(size_bytes: int) -> str:
    """Return a simple human-readable size string."""
    if size_bytes >= 1_000_000:
        return f"{size_bytes / 1_000_000:.1f}M"
    if size_bytes >= 1_000:
        return f"{size_bytes / 1_000:.1f}K"
    return f"{size_bytes}B"


def _segment_stats(seg_path: Path) -> dict[str, int]:
    """Return recursive file, talent, and byte counts for a segment."""
    files = 0
    talents = 0
    size = 0
    for path in seg_path.rglob("*"):
        if path.is_file():
            files += 1
            size += path.stat().st_size
            if "talents" in path.parts:
                talents += 1
    return {"files": files, "talents": talents, "size": size}


def _split_segment_path(path: str) -> tuple[str, str, str]:
    """Parse day/stream/segment input."""
    parts = path.split("/")
    if len(parts) != 3:
        print(
            "Segment path must be day/stream/segment (e.g. 20260304/default/090000_300)",
            file=sys.stderr,
        )
        raise SystemExit(1)
    return parts[0], parts[1], parts[2]


def _segment_duration(segment: str) -> int:
    """Return the duration seconds from HHMMSS_LEN."""
    return int(segment.split("_", 1)[1])


def _segment_time_strings(seg_path: Path) -> tuple[str | None, str | None]:
    """Return segment start/end strings if parseable."""
    start_time, end_time = segment_parse(str(seg_path))
    if start_time is None or end_time is None:
        return None, None
    return start_time.strftime("%H:%M:%S"), end_time.strftime("%H:%M:%S")


def _next_day(day: str) -> str:
    """Return the next YYYYMMDD day string."""
    return (datetime.strptime(day, "%Y%m%d") + timedelta(days=1)).strftime("%Y%m%d")


def _find_next_segment(
    day: str, stream: str, segment: str
) -> tuple[str | None, str | None]:
    """Find the next segment in a stream, checking same day then next day."""
    for scan_day in (day, _next_day(day)):
        for stream_name, seg_key, seg_path in iter_segments(scan_day):
            if stream_name != stream:
                continue
            marker = read_segment_stream(seg_path)
            if not marker:
                continue
            if marker.get("stream") != stream:
                continue
            if marker.get("prev_day") != day:
                continue
            if marker.get("prev_segment") != segment:
                continue
            return scan_day, seg_key
    return None, None


def _find_successor_segment(
    day: str, stream: str, segment: str
) -> tuple[str | None, str | None, Path | None]:
    """Find the segment whose stream.json points back to day/segment.

    Unlike _find_next_segment which only checks 2 days, this scans
    all days in the journal. The successor can be on any day because
    prior cross-day moves may have reordered the chain.

    Returns (day, segment_key, segment_path) or (None, None, None).
    """
    for scan_day in sorted(day_dirs().keys()):
        for stream_name, seg_key, seg_path in iter_segments(scan_day):
            if stream_name != stream:
                continue
            marker = read_segment_stream(seg_path)
            if not marker:
                continue
            if marker.get("stream") != stream:
                continue
            if marker.get("prev_day") == day and marker.get("prev_segment") == segment:
                return scan_day, seg_key, seg_path
    return None, None, None


def _is_stream_tail(day: str, stream: str, segment: str) -> bool:
    """Return True if stream state marks this segment as the current tail."""
    state = get_stream_state(stream)
    if state is None:
        return False
    return state.get("last_day") == day and state.get("last_segment") == segment


def _segment_files(seg_dir: Path) -> list[str]:
    """Return top-level file names within a segment directory."""
    return sorted(path.name for path in seg_dir.iterdir() if path.is_file())


def _agent_files(seg_dir: Path) -> list[str]:
    """Return top-level file names from talents/ if present."""
    talents_dir = seg_dir / "talents"
    if not talents_dir.is_dir():
        return []
    return sorted(path.name for path in talents_dir.iterdir() if path.is_file())


def _events_summary(seg_dir: Path) -> dict[str, object]:
    """Return count and unique tracts for events.jsonl."""
    events_path = seg_dir / "events.jsonl"
    if not events_path.exists():
        return {"entries": 0, "tracts": []}

    entries = 0
    tracts: set[str] = set()
    with open(events_path, "r", encoding="utf-8") as handle:
        for line in handle:
            line = line.strip()
            if not line:
                continue
            entries += 1
            try:
                payload = json.loads(line)
            except json.JSONDecodeError:
                continue
            tract = payload.get("tract")
            if isinstance(tract, str):
                tracts.add(tract)

    return {"entries": entries, "tracts": sorted(tracts)}


def _segment_index_info(day: str, stream: str, segment: str) -> dict[str, int | bool]:
    """Return journal index presence for a segment."""
    db_path = Path(get_journal()) / "indexer" / "journal.sqlite"
    if not db_path.exists():
        return {"available": False, "indexed": False, "chunks": 0}

    rel_path = f"{day}/{stream}/{segment}"
    try:
        conn = sqlite3.connect(db_path)
        try:
            indexed = bool(
                conn.execute(
                    "SELECT 1 FROM chunks WHERE path = ? LIMIT 1",
                    (rel_path,),
                ).fetchone()
            )
            chunk_count = conn.execute(
                "SELECT count(*) FROM chunks WHERE path = ? OR path LIKE ?",
                (rel_path, f"{rel_path}/%"),
            ).fetchone()[0]
        finally:
            conn.close()
    except sqlite3.Error:
        return {"available": False, "indexed": False, "chunks": 0}

    return {"available": True, "indexed": indexed, "chunks": int(chunk_count)}


def _describe_prev(day: str, stream: str, marker: dict | None) -> str:
    """Return formatted previous-chain description."""
    if not marker or not marker.get("prev_segment"):
        return "(none)"

    prev_day = marker.get("prev_day") or day
    prev_segment = marker["prev_segment"]
    prev_dir = _find_segment_dir_readonly(prev_day, prev_segment, stream)
    prev_path = f"{prev_day}/{stream}/{prev_segment}"
    if prev_dir is None:
        return f"{prev_path} [MISSING]"
    return prev_path


def _describe_next(day: str, stream: str, segment: str) -> str:
    """Return formatted forward-chain description."""
    next_day, next_segment = _find_next_segment(day, stream, segment)
    if next_day and next_segment:
        return f"{next_day}/{stream}/{next_segment}"
    if _is_stream_tail(day, stream, segment):
        return "(tail)"
    return "(none)"


def _check_directory(seg_dir: Path | None) -> tuple[bool, str]:
    """Verify the segment directory exists."""
    if seg_dir is not None and seg_dir.is_dir():
        return True, "directory exists"
    return False, "directory missing"


def _check_stream_json(seg_dir: Path | None) -> tuple[bool, str]:
    """Verify stream.json exists."""
    if seg_dir is None:
        return False, "stream.json missing"
    if (seg_dir / "stream.json").is_file():
        return True, "stream.json exists"
    return False, "stream.json missing"


def _check_stream_json_valid(seg_dir: Path | None) -> tuple[bool, str]:
    """Verify stream.json is valid JSON with a stream field."""
    if seg_dir is None:
        return False, "stream.json missing"

    path = seg_dir / "stream.json"
    if not path.exists():
        return False, "stream.json missing"

    try:
        with open(path, "r", encoding="utf-8") as handle:
            payload = json.load(handle)
    except (json.JSONDecodeError, OSError):
        return False, "stream.json invalid JSON"

    if payload.get("stream"):
        return True, "stream.json valid"
    return False, "stream.json missing stream field"


def _check_content_files(seg_dir: Path | None) -> tuple[bool, str]:
    """Verify transcript content files exist."""
    if seg_dir is None:
        return False, "segment directory missing"

    if (seg_dir / "audio.jsonl").exists() or (seg_dir / "screen.jsonl").exists():
        return True, "content files present"
    return False, "no audio.jsonl or screen.jsonl"


def _check_backward_chain(
    day: str, stream: str, marker: dict | None
) -> tuple[bool, str]:
    """Verify backward chain integrity."""
    if not marker or not marker.get("prev_segment"):
        return True, "no previous segment"

    prev_day = marker.get("prev_day")
    prev_segment = marker.get("prev_segment")
    if not prev_day or not prev_segment:
        return False, "prev_segment set without prev_day"

    prev_dir = _find_segment_dir_readonly(prev_day, prev_segment, stream)
    if prev_dir is not None:
        return True, "previous segment found"
    return False, f"missing previous segment {prev_day}/{stream}/{prev_segment}"


def _check_forward_chain(day: str, stream: str, segment: str) -> tuple[bool, str]:
    """Verify forward chain integrity."""
    next_day, next_segment = _find_next_segment(day, stream, segment)
    if next_day and next_segment:
        return True, f"next segment {next_day}/{stream}/{next_segment}"
    if _is_stream_tail(day, stream, segment):
        return True, "stream tail"
    return False, "next segment not found, not stream tail"


def _check_index_presence(day: str, stream: str, segment: str) -> tuple[bool, str]:
    """Verify the segment has an index entry when a DB is available."""
    info = _segment_index_info(day, stream, segment)
    if not info["available"]:
        return True, "journal index not available"
    if info["indexed"]:
        return True, "segment indexed"
    return False, "segment not indexed"


def _run_checks(day: str, stream: str, segment: str) -> list[dict[str, object]]:
    """Run all segment verification checks."""
    seg_dir = _find_segment_dir_readonly(day, segment, stream)
    marker = read_segment_stream(seg_dir) if seg_dir is not None else None

    checks = [
        ("directory exists", _check_directory(seg_dir)),
        ("stream.json exists", _check_stream_json(seg_dir)),
        ("stream.json valid", _check_stream_json_valid(seg_dir)),
        ("content files present", _check_content_files(seg_dir)),
        ("backward chain", _check_backward_chain(day, stream, marker)),
        ("forward chain", _check_forward_chain(day, stream, segment)),
        ("index presence", _check_index_presence(day, stream, segment)),
    ]

    return [
        {"check": name, "passed": passed, "detail": detail}
        for name, (passed, detail) in checks
    ]


def _rewrite_events_jsonl(seg_dir: Path, new_day: str, new_segment: str) -> int:
    """Rewrite events.jsonl to update day and segment fields.

    Returns number of lines rewritten.
    """
    events_path = seg_dir / "events.jsonl"
    if not events_path.exists():
        return 0

    lines = events_path.read_text(encoding="utf-8").splitlines()
    rewritten = []
    count = 0
    for line in lines:
        stripped = line.strip()
        if not stripped:
            rewritten.append(line)
            continue
        try:
            obj = json.loads(stripped)
            obj["day"] = new_day
            obj["segment"] = new_segment
            rewritten.append(json.dumps(obj, ensure_ascii=False))
            count += 1
        except (json.JSONDecodeError, TypeError):
            rewritten.append(line)

    tmp = events_path.with_suffix(".tmp")
    tmp.write_text("\n".join(rewritten) + "\n" if rewritten else "", encoding="utf-8")
    os.rename(str(tmp), str(events_path))
    return count


def _touch_health_marker(day: str) -> None:
    """Touch health/stream.updated for a day."""
    health_dir = day_path(day) / "health"
    health_dir.mkdir(parents=True, exist_ok=True)
    (health_dir / "stream.updated").touch()


def _delete_index_rows(journal: str, rel_path: str) -> dict[str, int]:
    """Delete all index rows referencing a segment path.

    Returns counts of deleted rows per table.
    """
    db_path = Path(journal) / "indexer" / "journal.sqlite"
    if not db_path.exists():
        return {"chunks": 0, "files": 0}

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
    except sqlite3.Error:
        return {"chunks": 0, "files": 0}

    return {
        "chunks": chunks_deleted,
        "files": files_deleted,
    }


def _reindex_segment(journal: str, seg_dir: Path) -> int:
    """Re-index all formattable files in a segment directory.

    Returns the number of files indexed.
    """
    from solstone.think.indexer.journal import index_file

    count = 0
    for path in sorted(seg_dir.rglob("*")):
        if not path.is_file():
            continue
        try:
            if index_file(journal, str(path)):
                count += 1
        except (ValueError, FileNotFoundError):
            continue
    return count


def cmd_move(args: argparse.Namespace) -> None:
    """Move a segment to a different day/time."""
    src_day, stream, src_segment = _split_segment_path(args.path)

    src_dir = _find_segment_dir_readonly(src_day, src_segment, stream)
    if src_dir is None:
        print(f"Segment not found: {args.path}", file=sys.stderr)
        raise SystemExit(1)

    to_day = args.to_day
    if not re.fullmatch(r"\d{8}", to_day):
        print(f"Invalid --to-day format: {to_day} (expected YYYYMMDD)", file=sys.stderr)
        raise SystemExit(1)

    if args.to_time:
        if not re.fullmatch(r"\d{6}", args.to_time):
            print(
                f"Invalid --to-time format: {args.to_time} (expected HHMMSS)",
                file=sys.stderr,
            )
            raise SystemExit(1)
        duration = src_segment.split("_", 1)[1]
        new_segment = f"{args.to_time}_{duration}"
    else:
        new_segment = src_segment

    if to_day == src_day and new_segment == src_segment:
        print("Source and destination are the same", file=sys.stderr)
        raise SystemExit(1)

    dst_parent = day_path(to_day, create=False) / stream
    dst_dir = dst_parent / new_segment
    if dst_dir.exists():
        if args.to_time:
            from solstone.observe.utils import find_available_segment

            avail = find_available_segment(dst_parent, new_segment)
            if avail is None:
                print(
                    f"No available segment slot near {new_segment} on {to_day}",
                    file=sys.stderr,
                )
                raise SystemExit(1)
            new_segment = avail
            dst_dir = dst_parent / new_segment
        else:
            print(
                f"Segment {new_segment} already exists on {to_day}/{stream}. "
                f"Use --to-time to specify an alternate time.",
                file=sys.stderr,
            )
            raise SystemExit(1)

    marker = read_segment_stream(src_dir)
    if not marker:
        print("No stream.json in source segment", file=sys.stderr)
        raise SystemExit(1)
    if marker.get("stream") != stream:
        print(
            f"Stream mismatch: path says '{stream}' but stream.json says '{marker.get('stream')}'",
            file=sys.stderr,
        )
        raise SystemExit(1)

    succ_day, succ_seg, succ_path = _find_successor_segment(
        src_day, stream, src_segment
    )

    events_path = src_dir / "events.jsonl"
    events_count = 0
    if events_path.exists():
        with open(events_path, encoding="utf-8") as handle:
            events_count = sum(1 for line in handle if line.strip())

    journal = get_journal()
    old_rel = f"{src_day}/{stream}/{src_segment}"
    index_info = _segment_index_info(src_day, stream, src_segment)

    print(f"Move: {src_day}/{stream}/{src_segment} -> {to_day}/{stream}/{new_segment}")
    print(f"  events.jsonl lines: {events_count}")
    if succ_day:
        print(f"  successor to patch: {succ_day}/{stream}/{succ_seg}")
    else:
        print("  successor to patch: (none - stream tail)")
    if index_info["available"]:
        print(f"  index chunks: {index_info['chunks']}")
    print(f"  health markers: {src_day}, {to_day}")

    if args.dry_run:
        print("\n[dry run] No changes made")
        return

    verbose = getattr(args, "verbose", False)

    print("\nExecuting move...")
    dst_parent.mkdir(parents=True, exist_ok=True)
    if verbose:
        print(f"  created directory: {dst_parent}")
    shutil.move(str(src_dir), str(dst_dir))
    print(f"  moved directory: {src_dir.name} -> {dst_dir}")

    rewritten = _rewrite_events_jsonl(dst_dir, to_day, new_segment)
    if rewritten:
        print(
            f"  rewrote {rewritten} events.jsonl lines (day: {src_day}->{to_day}, segment: {src_segment}->{new_segment})"
        )
    elif verbose:
        print("  no events.jsonl to rewrite")

    if succ_path:
        succ_marker = read_segment_stream(succ_path)
        if succ_marker:
            write_segment_stream(
                succ_path,
                succ_marker["stream"],
                to_day,
                new_segment,
                succ_marker["seq"],
            )
            print(f"  patched successor {succ_day}/{stream}/{succ_seg}")
            if verbose:
                print(f"    prev_day: {succ_marker.get('prev_day')} -> {to_day}")
                print(
                    f"    prev_segment: {succ_marker.get('prev_segment')} -> {new_segment}"
                )
    elif verbose:
        print("  no successor to patch (stream tail)")

    summary = rebuild_stream_state(stream)
    print(f"  rebuilt stream state: {stream}")
    if verbose:
        print(
            f"    scanned {summary['segments_scanned']} segments, rebuilt {len(summary['rebuilt'])} stream(s)"
        )

    if index_info["available"]:
        deleted = _delete_index_rows(journal, old_rel)
        if any(deleted.values()) or verbose:
            print(
                f"  deleted index rows: chunks={deleted['chunks']}, files={deleted['files']}"
            )
        new_rel = f"{to_day}/{stream}/{new_segment}"
        indexed = _reindex_segment(journal, dst_dir)
        print(f"  re-indexed: {indexed} files at {new_rel}")
    elif verbose:
        print("  index not available, skipping reindex")

    _touch_health_marker(src_day)
    _touch_health_marker(to_day)
    print(f"  touched health markers: {src_day}, {to_day}")
    if verbose:
        print("    think will re-run daily talents on both days")

    # Post-move verify is informational — the move already completed.
    print()
    results = _run_checks(to_day, stream, new_segment)
    _print_check_results(results)
    passed = sum(1 for result in results if result["passed"])
    print(f"\n{passed}/{len(results)} checks passed")


def cmd_list(args: argparse.Namespace) -> None:
    """List segments for a day."""
    segments = iter_segments(args.day)
    if args.stream:
        segments = [entry for entry in segments if entry[0] == args.stream]

    if not segments:
        print(f"No segments found for {args.day}")
        return

    rows = []
    for stream_name, seg_key, seg_path in segments:
        start, end = _segment_time_strings(seg_path)
        stats = _segment_stats(seg_path)
        rows.append(
            {
                "stream": stream_name,
                "segment": seg_key,
                "start": start,
                "end": end,
                "duration": _segment_duration(seg_key),
                "files": stats["files"],
                "talents": stats["talents"],
                "size": stats["size"],
            }
        )

    if args.json_output:
        print(json.dumps(rows, indent=2))
        return

    print(
        f"{'STREAM':<20} {'SEGMENT':<14} {'TIME':<15} "
        f"{'DUR':>5} {'FILES':>5} {'TALENTS':>7} {'SIZE':>8}"
    )
    print("-" * 78)
    for row in rows:
        time_str = (
            f"{row['start']}-{row['end']}"
            if row["start"] is not None and row["end"] is not None
            else "?"
        )
        dur_str = f"{row['duration']}s"
        print(
            f"{row['stream']:<20} {row['segment']:<14} {time_str:<15} "
            f"{dur_str:>5} {row['files']:>5} {row['talents']:>7} "
            f"{_format_size(int(row['size'])):>8}"
        )


def cmd_inspect(args: argparse.Namespace) -> None:
    """Inspect one segment."""
    day, stream, segment = _split_segment_path(args.path)
    seg_dir = _find_segment_dir_readonly(day, segment, stream)
    if seg_dir is None:
        print(f"Segment not found: {args.path}", file=sys.stderr)
        raise SystemExit(1)

    marker = read_segment_stream(seg_dir) or {}
    stream_name = marker.get("stream") or stream
    start, end = _segment_time_strings(seg_dir)
    duration = _segment_duration(segment)
    prev_desc = _describe_prev(day, stream_name, marker)
    next_desc = _describe_next(day, stream_name, segment)
    files = _segment_files(seg_dir)
    talents = _agent_files(seg_dir)
    stats = _segment_stats(seg_dir)
    events = _events_summary(seg_dir)
    index_info = _segment_index_info(day, stream_name, segment)

    payload = {
        "path": f"{day}/{stream}/{segment}",
        "stream": stream_name,
        "segment": segment,
        "seq": marker.get("seq"),
        "prev_day": marker.get("prev_day"),
        "prev_segment": marker.get("prev_segment"),
        "start": start,
        "end": end,
        "duration": duration,
        "chain": {"prev": prev_desc, "next": next_desc},
        "files": files,
        "talents": talents,
        "stats": stats,
        "events": events,
        "index": index_info,
    }

    if args.json_output:
        print(json.dumps(payload, indent=2))
        return

    time_range = "?"
    if start is not None and end is not None:
        time_range = f"{start} - {end}"

    print(f"Segment: {day}/{stream}/{segment}")
    if marker.get("seq") is not None:
        print(f"Stream:  {stream_name} (seq {marker['seq']})")
    else:
        print(f"Stream:  {stream_name}")
    print(f"Time:    {time_range} ({duration}s)")
    print()
    print("Chain:")
    print(f"  prev: {prev_desc}")
    print(f"  next: {next_desc}")
    print()
    print(f"Files ({len(files)}):")
    if files:
        print(f"  {', '.join(files)}")
    print()
    print(f"Talents ({len(talents)}):")
    if talents:
        print(f"  {', '.join(talents)}")
    print()
    print(f"Size: {_format_size(stats['size'])}")
    if index_info["available"]:
        if index_info["indexed"]:
            print(f"Index: indexed ({index_info['chunks']} chunks)")
        else:
            print("Index: not-indexed")
    else:
        print("Index: unavailable")
    tracts = ", ".join(events["tracts"])
    if tracts:
        print(f"Events: {events['entries']} entries ({tracts})")
    else:
        print(f"Events: {events['entries']} entries")


def _print_check_results(results: list[dict[str, object]]) -> None:
    """Print PASS/FAIL lines for verify output."""
    for result in results:
        status = "PASS" if result["passed"] else "FAIL"
        detail = str(result["detail"])
        if result["passed"]:
            print(f"{status:<5} {result['check']}")
        else:
            print(f"{status:<5} {result['check']}: {detail}")


def cmd_verify(args: argparse.Namespace) -> None:
    """Verify one segment or all segments for a day."""
    if args.path:
        day, stream, segment = _split_segment_path(args.path)
        results = _run_checks(day, stream, segment)
        if args.json_output:
            print(json.dumps(results, indent=2))
        else:
            _print_check_results(results)
            passed = sum(1 for result in results if result["passed"])
            print()
            print(f"{passed}/{len(results)} checks passed")
        raise SystemExit(0 if all(result["passed"] for result in results) else 1)

    if not args.day:
        print("verify requires a segment path or --day", file=sys.stderr)
        raise SystemExit(1)

    segments = iter_segments(args.day)
    if not segments:
        print(f"No segments found for {args.day}", file=sys.stderr)
        raise SystemExit(1)

    all_results: dict[str, list[dict[str, object]]] = {}
    total_passed = 0
    total_failed = 0

    for stream_name, seg_key, _seg_path in segments:
        seg_id = f"{args.day}/{stream_name}/{seg_key}"
        results = _run_checks(args.day, stream_name, seg_key)
        all_results[seg_id] = results
        total_passed += sum(1 for result in results if result["passed"])
        total_failed += sum(1 for result in results if not result["passed"])

    if args.json_output:
        print(
            json.dumps(
                {
                    "segments": all_results,
                    "summary": {"passed": total_passed, "failed": total_failed},
                },
                indent=2,
            )
        )
    else:
        for seg_id, results in all_results.items():
            print(f"--- {seg_id} ---")
            _print_check_results(results)
            print()
        print(f"Summary: {total_passed}/{total_passed + total_failed} checks passed")

    raise SystemExit(0 if total_failed == 0 else 1)


def main() -> None:
    """CLI entry point for journal segment."""
    parser = argparse.ArgumentParser(
        description="Inspect and manage journal segments",
        usage="journal segment <command> [options]",
    )
    sub = parser.add_subparsers(dest="subcommand")

    p_list = sub.add_parser("list", help="List segments for a day")
    p_list.add_argument("day", help="Day in YYYYMMDD format")
    p_list.add_argument("--stream", help="Filter to a specific stream")
    p_list.add_argument(
        "--json", dest="json_output", action="store_true", help="Output as JSON"
    )

    p_inspect = sub.add_parser("inspect", help="Show segment metadata")
    p_inspect.add_argument(
        "path",
        help="Segment path: day/stream/segment (e.g. 20260304/default/090000_300)",
    )
    p_inspect.add_argument(
        "--json", dest="json_output", action="store_true", help="Output as JSON"
    )

    p_verify = sub.add_parser("verify", help="Verify segment integrity")
    p_verify.add_argument("path", nargs="?", help="Segment path: day/stream/segment")
    p_verify.add_argument("--day", help="Verify all segments for a day")
    p_verify.add_argument(
        "--json", dest="json_output", action="store_true", help="Output as JSON"
    )

    p_move = sub.add_parser("move", help="Move segment to a different day/time")
    p_move.add_argument(
        "path",
        help="Segment path: day/stream/segment (e.g. 20260304/default/090000_300)",
    )
    p_move.add_argument("--to-day", required=True, help="Destination day (YYYYMMDD)")
    p_move.add_argument(
        "--to-time", help="New time (HHMMSS), preserving original duration"
    )
    p_move.add_argument(
        "--dry-run", action="store_true", help="Show plan without making changes"
    )
    p_move.add_argument(
        "-v", "--verbose", action="store_true", help="Show detailed progress"
    )

    args = setup_cli(parser)
    require_solstone()

    if args.subcommand is None:
        parser.print_help()
        raise SystemExit(1)

    if args.subcommand == "list":
        cmd_list(args)
    elif args.subcommand == "inspect":
        cmd_inspect(args)
    elif args.subcommand == "verify":
        cmd_verify(args)
    elif args.subcommand == "move":
        cmd_move(args)
