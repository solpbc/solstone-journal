# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Stream identity for journal segments.

A stream is a named series of segments from a single source. Every segment
belongs to exactly one stream and links to its predecessor, creating a
navigable chain with human-readable identity.

Naming convention (separator is '.'):
    Local observer:   {hostname}           e.g. "archon"  (domain stripped: archon.local -> archon)
    Local tmux:       {hostname}.tmux      e.g. "archon.tmux"
    Observer:         {observer_name}      e.g. "laptop"  (domain stripped: laptop.local -> laptop)
    Import (Apple):   import.apple
    Import (Plaud):   import.plaud
    Import (generic): import.audio
    Import (text):    import.text

Storage:
    journal/streams/{name}.json   - per-stream state (last segment, seq)
    {segment_dir}/stream.json          - per-segment marker (stream, prev, seq)
"""

from __future__ import annotations

import json
import logging
import os
import re
import threading
import time
from pathlib import Path

from solstone.think.utils import get_journal, iter_segments

logger = logging.getLogger(__name__)

# Valid stream name: lowercase, dots allowed, no path separators
_STREAM_NAME_RE = re.compile(r"^[a-z0-9][a-z0-9._-]*$")


def _strip_hostname(name: str) -> str:
    """Strip domain suffix from a hostname, keeping only the first label.

    Dots in stream names are reserved for qualifiers (e.g., '.tmux') and
    import prefixes (e.g., 'import.apple'). Hostnames like 'ja1r.local'
    or '192.168.1.1' must be reduced to a dot-free base name.

    Examples: 'ja1r.local' -> 'ja1r', '192.168.1.1' -> '192-168-1-1',
    'archon' -> 'archon', 'my.host.example.com' -> 'my'
    """
    name = name.strip()
    if not name:
        return name
    # IP addresses: all parts are digits — join with dashes
    parts = name.split(".")
    if all(p.isdigit() for p in parts if p):
        return "-".join(p for p in parts if p)
    # Domain names: keep only the first label
    return parts[0]


def stream_name(
    *,
    host: str | None = None,
    observer: str | None = None,
    import_source: str | None = None,
    qualifier: str | None = None,
) -> str:
    """Derive canonical stream name from source characteristics.

    Exactly one of host, observer, or import_source must be provided.

    Parameters
    ----------
    host : str, optional
        Local hostname (e.g., "archon").
    observer : str, optional
        Observer name (e.g., "laptop").
    import_source : str, optional
        Import source type (e.g., "apple", "plaud", "audio", "text").
    qualifier : str, optional
        Sub-stream qualifier (e.g., "tmux"). Appended with dot separator.

    Returns
    -------
    str
        Canonical stream name.

    Raises
    ------
    ValueError
        If no source is provided, or the resulting name is invalid.
    """
    if host:
        base = _strip_hostname(host)
    elif observer:
        base = _strip_hostname(observer)
    elif import_source:
        base = f"import.{import_source}"
    else:
        raise ValueError("stream_name requires host, observer, or import_source")

    # Sanitize: lowercase, replace spaces/slashes with dash, strip
    name = base.lower().strip()
    name = re.sub(r"[\s/\\]+", "-", name)

    if qualifier:
        qualifier = qualifier.lower().strip()
        qualifier = re.sub(r"[\s/\\]+", "-", qualifier)
        name = f"{name}.{qualifier}"

    # Validate
    if not name or ".." in name:
        raise ValueError(f"Invalid stream name: {name!r}")
    if not _STREAM_NAME_RE.match(name):
        raise ValueError(f"Invalid stream name: {name!r}")

    return name


def _streams_dir() -> Path:
    """Return the streams state directory, creating it if needed."""
    d = Path(get_journal()) / "streams"
    d.mkdir(parents=True, exist_ok=True)
    return d


def get_stream_state(name: str) -> dict | None:
    """Load stream state from journal/streams/{name}.json.

    Returns
    -------
    dict or None
        Stream state dict, or None if the stream doesn't exist.
    """
    path = _streams_dir() / f"{name}.json"
    if not path.exists():
        return None
    try:
        with open(path, "r", encoding="utf-8") as f:
            return json.load(f)
    except (json.JSONDecodeError, OSError) as exc:
        logger.warning("Failed to read stream state %s: %s", path, exc)
        return None


def update_stream(
    name: str,
    day: str,
    segment: str,
    *,
    type: str | None = None,
    host: str | None = None,
    platform: str | None = None,
) -> dict:
    """Atomic read-modify-write of stream state file.

    Creates the stream file on first segment. Increments seq and updates
    last_day/last_segment.

    Parameters
    ----------
    name : str
        Stream name.
    day : str
        Day string (YYYYMMDD).
    segment : str
        Segment key (HHMMSS_LEN).
    type : str, optional
        Stream type (e.g., "observer", "import").
    host : str, optional
        Hostname for the stream.
    platform : str, optional
        Platform string (e.g., "linux", "darwin").

    Returns
    -------
    dict
        ``{"prev_day": ..., "prev_segment": ..., "seq": N}`` where prev
        values are None for the first segment in a stream.
    """
    streams_dir = _streams_dir()
    state_path = streams_dir / f"{name}.json"

    # Read existing state
    state = None
    if state_path.exists():
        try:
            with open(state_path, "r", encoding="utf-8") as f:
                state = json.load(f)
        except (json.JSONDecodeError, OSError):
            state = None

    if state is None:
        # First segment in stream
        state = {
            "name": name,
            "type": type or "unknown",
            "host": host,
            "platform": platform,
            "created_at": int(time.time()),
            "last_day": day,
            "last_segment": segment,
            "seq": 1,
        }
        prev_day = None
        prev_segment = None
        seq = 1
    else:
        prev_day = state.get("last_day")
        prev_segment = state.get("last_segment")
        seq = state.get("seq", 0) + 1
        state["last_day"] = day
        state["last_segment"] = segment
        state["seq"] = seq
        # Update type/host/platform if provided (may be set on first call only)
        if type:
            state["type"] = type
        if host:
            state["host"] = host
        if platform:
            state["platform"] = platform

    # Atomic write: write to unique tmp file then rename
    tid = threading.get_ident()
    tmp_path = state_path.with_suffix(f".{os.getpid()}-{tid}.tmp")
    with open(tmp_path, "w", encoding="utf-8") as f:
        json.dump(state, f, indent=2)
        f.write("\n")
    os.rename(str(tmp_path), str(state_path))

    return {"prev_day": prev_day, "prev_segment": prev_segment, "seq": seq}


def write_segment_stream(
    segment_dir: str | Path,
    stream: str,
    prev_day: str | None,
    prev_segment: str | None,
    seq: int,
) -> None:
    """Write stream.json marker into a segment directory.

    Parameters
    ----------
    segment_dir : str or Path
        Path to the segment directory.
    stream : str
        Stream name.
    prev_day : str or None
        Previous segment's day (None for first segment).
    prev_segment : str or None
        Previous segment's key (None for first segment).
    seq : int
        Sequence number in stream.
    """
    marker = {
        "stream": stream,
        "prev_day": prev_day,
        "prev_segment": prev_segment,
        "seq": seq,
    }
    marker_path = Path(segment_dir) / "stream.json"
    with open(marker_path, "w", encoding="utf-8") as f:
        json.dump(marker, f)
        f.write("\n")


def read_segment_stream(segment_dir: str | Path) -> dict | None:
    """Read stream.json from a segment directory.

    Returns
    -------
    dict or None
        Stream marker dict, or None if the file doesn't exist (pre-stream segments).
    """
    marker_path = Path(segment_dir) / "stream.json"
    if not marker_path.exists():
        return None
    try:
        with open(marker_path, "r", encoding="utf-8") as f:
            return json.load(f)
    except (json.JSONDecodeError, OSError) as exc:
        logger.warning("Failed to read stream marker %s: %s", marker_path, exc)
        return None


def list_streams() -> list[dict]:
    """List all stream state files from journal/streams/.

    Returns
    -------
    list[dict]
        List of stream state dicts, sorted by name.
    """
    streams_dir = _streams_dir()
    result = []
    for path in sorted(streams_dir.glob("*.json")):
        try:
            with open(path, "r", encoding="utf-8") as f:
                state = json.load(f)
                result.append(state)
        except (json.JSONDecodeError, OSError) as exc:
            logger.warning("Failed to read stream %s: %s", path, exc)
    return result


def rebuild_stream_state(name: str | None = None) -> dict:
    """Reconstruct stream state from per-segment markers.

    Walks all day directories and reads stream.json from each segment.
    Rebuilds the stream state file(s) from this data.

    Parameters
    ----------
    name : str, optional
        If given, rebuild only this stream. Otherwise rebuild all.

    Returns
    -------
    dict
        Summary: ``{"rebuilt": ["stream1", ...], "segments_scanned": N}``
    """
    from solstone.think.utils import day_dirs

    streams: dict[str, dict] = {}  # name -> {last_day, last_segment, seq, ...}
    segments_scanned = 0

    for day in sorted(day_dirs().keys()):
        for _stream_name, seg_key, seg_dir in iter_segments(day):
            marker = read_segment_stream(seg_dir)
            if marker is None:
                continue

            stream_name_val = marker.get("stream")
            if not stream_name_val:
                continue

            # Skip if filtering to specific stream
            if name and stream_name_val != name:
                continue

            segments_scanned += 1
            seq = marker.get("seq", 0)

            if stream_name_val not in streams:
                streams[stream_name_val] = {
                    "name": stream_name_val,
                    "type": "unknown",
                    "host": None,
                    "platform": None,
                    "created_at": int(time.time()),
                    "last_day": day,
                    "last_segment": seg_key,
                    "seq": seq,
                }
            else:
                existing = streams[stream_name_val]
                # Update if this segment has a higher seq
                if seq > existing.get("seq", 0):
                    existing["last_day"] = day
                    existing["last_segment"] = seg_key
                    existing["seq"] = seq

    # Write rebuilt state files
    streams_dir = _streams_dir()
    rebuilt = []
    for sname, state in streams.items():
        state_path = streams_dir / f"{sname}.json"
        tmp_path = state_path.with_suffix(f".{os.getpid()}.tmp")
        with open(tmp_path, "w", encoding="utf-8") as f:
            json.dump(state, f, indent=2)
            f.write("\n")
        os.rename(str(tmp_path), str(state_path))
        rebuilt.append(sname)

    return {"rebuilt": rebuilt, "segments_scanned": segments_scanned}


def main() -> None:
    """CLI entry point for journal streams."""
    import argparse

    from solstone.think.utils import require_solstone, setup_cli

    parser = argparse.ArgumentParser(description="Inspect and manage stream identity")
    parser.add_argument(
        "name",
        nargs="?",
        help="Stream name to inspect (omit to list all streams)",
    )
    parser.add_argument(
        "--rebuild",
        action="store_true",
        help="Reconstruct stream state from per-segment markers",
    )

    args = setup_cli(parser)
    require_solstone()

    if args.rebuild:
        summary = rebuild_stream_state(name=args.name)
        rebuilt = summary["rebuilt"]
        scanned = summary["segments_scanned"]
        if rebuilt:
            print(f"Rebuilt {len(rebuilt)} stream(s) from {scanned} segments:")
            for name in rebuilt:
                print(f"  {name}")
        else:
            print(f"No streams found ({scanned} segments scanned)")
        return

    if args.name:
        # Inspect single stream
        state = get_stream_state(args.name)
        if state is None:
            print(f"Stream not found: {args.name}")
            raise SystemExit(1)
        print(json.dumps(state, indent=2))
        return

    # List all streams
    streams = list_streams()
    if not streams:
        print("No streams found")
        return

    # Table header
    print(f"{'Name':<24} {'Type':<12} {'Last Day':<10} {'Last Segment':<16} {'Seq':>5}")
    print("-" * 71)
    for s in streams:
        print(
            f"{s.get('name', '?'):<24} "
            f"{s.get('type', '?'):<12} "
            f"{s.get('last_day', '?'):<10} "
            f"{s.get('last_segment', '?'):<16} "
            f"{s.get('seq', 0):>5}"
        )
