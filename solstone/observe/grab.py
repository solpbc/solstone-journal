# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Walk observed screen frames in the journal and optionally write frame images."""

from __future__ import annotations

import argparse
import json
import logging
from dataclasses import dataclass
from datetime import datetime, timedelta
from pathlib import Path
from typing import Any

from solstone.observe.see import decode_frames
from solstone.observe.utils import (
    VIDEO_EXTENSIONS,
    load_analysis_frames,
    parse_screen_filename,
)
from solstone.think.utils import (
    get_journal,
    journal_relative_path,
    require_solstone,
    segment_parse,
    segment_path,
    setup_cli,
)

SUPPORTED_OUTPUT_SUFFIXES = {".png", ".jpg", ".jpeg", ".webp"}
TABLE_LEVELS = {
    "0": ("days", ["day", "streams", "segments", "screens", "frames_analyzed"]),
    "1": ("streams", ["stream", "segments", "screens", "frames_analyzed"]),
    "2": ("segments", ["segment", "start", "end", "screens", "frames_analyzed"]),
    "3": ("screens", ["screen", "position", "connector", "frames_analyzed", "status"]),
}
NEXT_FOOTERS = {
    "0": "Next: journal grab <day>",
    "1": "Next: journal grab <day> <stream>",
    "2": "Next: journal grab <day> <stream> <segment>",
    "3": "Next: journal grab <day> <stream> <segment> <screen>",
}
LEVEL_4_FOOTER = (
    "Inspect:    journal grab <day> <stream> <segment> <screen> <id>\n"
    "Save one:   journal grab <day> <stream> <segment> <screen> <id> --out PATH\n"
    "Save many:  journal grab <day> <stream> <segment> <screen> "
    "<id1>,<id2>,... --out PATH\n"
    "\n"
    "How extraction works:\n"
    "  Decoding walks the video linearly from frame 0 — seeking is unsafe at the\n"
    "  1 Hz capture rate. Cost is dominated by the highest requested frame_id, not\n"
    "  the count. Asking for ids 7,12,23 costs the same as asking for 23 alone.\n"
    "  Prefer batch mode when you want more than one frame from the same screen."
)
LEVEL_4_PURGED_FOOTER = (
    "Save mode unavailable: raw video has been purged by retention.\n"
    "Frame metadata above is still readable.\n"
    "\n"
    "Inspect: journal grab <day> <stream> <segment> <screen> <id>"
)


@dataclass
class ScreenBundle:
    video_path: Path | None
    jsonl_rel: str | None
    video_rel: str | None
    frame_records: list[dict[str, Any]]
    frame_index: dict[int, dict[str, Any]]
    legacy_schema: bool
    header_only: bool
    status: str
    segment_start: datetime


def _is_screen_token(stem: str) -> bool:
    return stem == "screen" or stem.endswith("_screen")


def _normalize_screen_token(stem: str) -> str:
    return stem.removesuffix("_screen") if stem != "screen" else stem


def _screen_stem(screen_token: str) -> str:
    return screen_token if _is_screen_token(screen_token) else f"{screen_token}_screen"


def _frame_notes(frame: dict[str, Any]) -> str:
    lines = str(frame.get("error") or "").splitlines()
    error = lines[0].strip() if lines else ""
    if not error:
        return ""
    text = error if len(error) <= 60 else f"{error[:57]}..."
    return f"error: {text}"


def _frame_primary(frame: dict[str, Any]) -> str:
    analysis = frame.get("analysis")
    value = analysis.get("primary") if isinstance(analysis, dict) else None
    return value if isinstance(value, str) else ""


def _frame_abs_time(segment_start: datetime, timestamp: Any) -> str:
    return (segment_start + timedelta(seconds=float(timestamp or 0.0))).isoformat()


def _segment_bounds(day: str, segment: str) -> tuple[datetime, datetime]:
    start_time, end_time = segment_parse(segment)
    if start_time is None or end_time is None:
        raise ValueError(f"segment {segment} is not a valid HHMMSS_LEN key")
    base = datetime.strptime(day, "%Y%m%d").date()
    return (
        datetime.combine(base, start_time),
        datetime.combine(base, end_time),
    )


def _frame_view(segment_start: datetime, frame: dict[str, Any]) -> dict[str, Any]:
    return {
        "frame_id": int(frame["frame_id"]),
        "timestamp": frame.get("timestamp", 0.0),
        "abs_time": _frame_abs_time(segment_start, frame.get("timestamp", 0.0)),
        "primary": _frame_primary(frame),
        "notes": _frame_notes(frame),
    }


def _scope(day: str, stream: str, segment: str, screen_token: str) -> dict[str, str]:
    return {"day": day, "stream": stream, "segment": segment, "screen": screen_token}


def _source(bundle: ScreenBundle) -> dict[str, str | None]:
    return {"jsonl": bundle.jsonl_rel, "video": bundle.video_rel}


def _print_table(columns: list[str], rows: list[dict[str, Any]]) -> None:
    if not rows:
        return
    widths = {
        col: max(len(col), *(len(str(row.get(col, ""))) for row in rows))
        for col in columns
    }
    print("  ".join(col.ljust(widths[col]) for col in columns))
    print("  ".join("-" * widths[col] for col in columns))
    for row in rows:
        print("  ".join(str(row.get(col, "")).ljust(widths[col]) for col in columns))


def _format_alternatives(header: str, items: list[str]) -> str:
    lines = ["", "", header]
    lines.extend(f"  {item}" for item in items)
    return "\n".join(lines)


def _available_days() -> list[str]:
    chronicle_dir = Path(get_journal()) / "chronicle"
    if not chronicle_dir.is_dir():
        return []
    return sorted(
        day_dir.name
        for day_dir in chronicle_dir.iterdir()
        if day_dir.is_dir() and len(day_dir.name) == 8 and day_dir.name.isdigit()
    )


def _closest_days(day: str, days: list[str]) -> list[str]:
    if not day.isdigit():
        return days[:5]
    target = int(day)
    closest = sorted(days, key=lambda value: (abs(int(value) - target), int(value)))[:5]
    return sorted(closest)


def _available_segments(stream_dir: Path) -> list[str]:
    segments = []
    for segment_dir in sorted(path for path in stream_dir.iterdir() if path.is_dir()):
        start_time, end_time = segment_parse(segment_dir.name)
        if start_time is not None and end_time is not None:
            segments.append(segment_dir.name)
    return segments


def _truncate_segments(segments: list[str]) -> list[str]:
    if len(segments) <= 20:
        return segments
    return [*segments[:10], "...", *segments[-10:]]


def _available_screen_tokens(segment_dir: Path) -> list[str]:
    tokens: set[str] = set()
    for entry in segment_dir.iterdir():
        if not entry.is_file():
            continue
        suffix = entry.suffix.lower()
        if suffix == ".jsonl" and _is_screen_token(entry.stem):
            tokens.add(entry.stem)
        if suffix in VIDEO_EXTENSIONS and _is_screen_token(entry.stem):
            tokens.add(entry.stem)
    return sorted(_normalize_screen_token(token) for token in tokens)


def _require_day(day: str) -> Path:
    day_dir = Path(get_journal()) / "chronicle" / day
    if not day_dir.is_dir():
        alternatives = _format_alternatives(
            "Available days (closest 5):",
            _closest_days(day, _available_days()),
        )
        raise FileNotFoundError(f"day {day} not found{alternatives}")
    return day_dir


def _require_stream(day: str, stream: str) -> Path:
    day_dir = _require_day(day)
    stream_dir = Path(get_journal()) / "chronicle" / day / stream
    if not stream_dir.is_dir():
        streams = sorted(
            path.name
            for path in day_dir.iterdir()
            if path.is_dir() and path.name != "health"
        )
        alternatives = _format_alternatives(
            f"Available streams in {day}:",
            streams,
        )
        raise FileNotFoundError(f"stream {stream} not found in {day}{alternatives}")
    return stream_dir


def _require_segment(day: str, stream: str, segment: str) -> Path:
    stream_dir = _require_stream(day, stream)
    seg_dir = segment_path(day, segment, stream, create=False)
    if not seg_dir.is_dir():
        alternatives = _format_alternatives(
            f"Available segments in {day}/{stream}:",
            _truncate_segments(_available_segments(stream_dir)),
        )
        raise FileNotFoundError(
            f"segment {segment} not found in {day}/{stream}{alternatives}"
        )
    return seg_dir


def _load_analyzed_bundle(
    day: str, stream: str, segment: str, screen_token: str
) -> ScreenBundle:
    bundle = load_screen_bundle(day, stream, segment, screen_token, keep_errors=True)
    if bundle.status == "captured but not analyzed":
        raise ValueError(
            f"screen {screen_token} in {segment} is captured but not analyzed"
        )
    if bundle.legacy_schema:
        raise ValueError(
            "screen file uses pre-frame_id schema; frame selection is unavailable"
        )
    return bundle


def load_screen_bundle(
    day: str, stream: str, segment: str, screen_token: str, *, keep_errors: bool
) -> ScreenBundle:
    segment_dir = _require_segment(day, stream, segment)
    screen_stem = _screen_stem(screen_token)
    jsonl_path = segment_dir / f"{screen_stem}.jsonl"
    header: dict[str, Any] | None = None
    records: list[dict[str, Any]] = []

    if jsonl_path.is_file():
        records = load_analysis_frames(jsonl_path, keep_errors=keep_errors)
        if records and "raw" in records[0] and "frame_id" not in records[0]:
            header = records[0]
    else:
        jsonl_path = None

    video_path: Path | None = None
    if header:
        raw_path = header.get("raw")
        if isinstance(raw_path, str) and raw_path.endswith(VIDEO_EXTENSIONS):
            candidate = segment_dir / raw_path
            if candidate.is_file():
                video_path = candidate

    if video_path is None:
        for ext in VIDEO_EXTENSIONS:
            candidate = segment_dir / f"{screen_stem}{ext}"
            if candidate.is_file():
                video_path = candidate
                break

    frame_records = [record for record in records if "frame_id" in record]
    frame_index = {int(record["frame_id"]): record for record in frame_records}
    if header:
        non_header_records = records[1:]
    else:
        non_header_records = records
    header_only = (
        jsonl_path is not None and not frame_records and not non_header_records
    )
    legacy_schema = (
        jsonl_path is not None and not frame_records and bool(non_header_records)
    )

    if jsonl_path is None and video_path is not None:
        status = "captured but not analyzed"
    elif jsonl_path is not None and video_path is None:
        status = "analyzed; raw media purged by retention"
    elif jsonl_path is not None:
        status = "analyzed"
    else:
        alternatives = _format_alternatives(
            f"Available screens in {day}/{stream}/{segment}:",
            _available_screen_tokens(segment_dir),
        )
        raise FileNotFoundError(
            f"screen {screen_token} not found in {day}/{stream}/{segment}{alternatives}"
        )

    segment_start, _segment_end = _segment_bounds(day, segment)

    journal = Path(get_journal())
    return ScreenBundle(
        video_path=video_path,
        jsonl_rel=journal_relative_path(journal, jsonl_path) if jsonl_path else None,
        video_rel=journal_relative_path(journal, video_path) if video_path else None,
        frame_records=sorted(frame_records, key=lambda record: int(record["frame_id"])),
        frame_index=frame_index,
        legacy_schema=legacy_schema,
        header_only=header_only,
        status=status,
        segment_start=segment_start,
    )


def list_segment_screens(day: str, stream: str, segment: str) -> dict[str, Any]:
    segment_dir = _require_segment(day, stream, segment)
    tokens = [_screen_stem(token) for token in _available_screen_tokens(segment_dir)]

    screens = []
    for token in sorted(tokens):
        bundle = load_screen_bundle(day, stream, segment, token, keep_errors=True)
        position, connector = parse_screen_filename(token)
        screens.append(
            {
                "screen": _normalize_screen_token(token),
                "position": position,
                "connector": connector,
                "frames_analyzed": len(bundle.frame_records),
                "jsonl": bundle.jsonl_rel,
                "video": bundle.video_rel,
                "status": bundle.status,
            }
        )

    return {
        "level": "3",
        "scope": {"day": day, "stream": stream, "segment": segment},
        "data": {"screens": screens},
    }


def list_stream_segments(day: str, stream: str) -> dict[str, Any]:
    stream_dir = _require_stream(day, stream)
    rows = []
    for segment_dir in sorted(p for p in stream_dir.iterdir() if p.is_dir()):
        try:
            start_dt, end_dt = _segment_bounds(day, segment_dir.name)
        except ValueError:
            continue
        screen_payload = list_segment_screens(day, stream, segment_dir.name)
        screens = screen_payload["data"]["screens"]
        if not screens:
            continue
        rows.append(
            {
                "segment": segment_dir.name,
                "start": start_dt.time().isoformat(),
                "end": end_dt.time().isoformat(),
                "screens": len(screens),
                "frames_analyzed": sum(
                    int(screen["frames_analyzed"]) for screen in screens
                ),
            }
        )

    return {
        "level": "2",
        "scope": {"day": day, "stream": stream},
        "data": {"segments": rows},
    }


def list_day_streams(day: str) -> dict[str, Any]:
    day_dir = _require_day(day)
    streams = [
        {
            "stream": stream_dir.name,
            "segments": len(segments),
            "screens": sum(int(segment["screens"]) for segment in segments),
            "frames_analyzed": sum(
                int(segment["frames_analyzed"]) for segment in segments
            ),
        }
        for stream_dir in sorted(p for p in day_dir.iterdir() if p.is_dir())
        if stream_dir.name != "health"
        if (segments := list_stream_segments(day, stream_dir.name)["data"]["segments"])
    ]
    return {
        "level": "1",
        "scope": {"day": day},
        "data": {"streams": sorted(streams, key=lambda stream: str(stream["stream"]))},
    }


def list_available_days() -> dict[str, Any]:
    chronicle_dir = Path(get_journal()) / "chronicle"
    days = (
        [
            {
                "day": day_dir.name,
                "streams": len(streams),
                "segments": sum(int(stream["segments"]) for stream in streams),
                "screens": sum(int(stream["screens"]) for stream in streams),
                "frames_analyzed": sum(
                    int(stream["frames_analyzed"]) for stream in streams
                ),
            }
            for day_dir in sorted(p for p in chronicle_dir.iterdir() if p.is_dir())
            if len(day_dir.name) == 8 and day_dir.name.isdigit()
            if (streams := list_day_streams(day_dir.name)["data"]["streams"])
        ]
        if chronicle_dir.is_dir()
        else []
    )
    return {"level": "0", "scope": {}, "data": {"days": days}}


def list_screen_frames(
    day: str, stream: str, segment: str, screen_token: str
) -> dict[str, Any]:
    bundle = load_screen_bundle(day, stream, segment, screen_token, keep_errors=True)
    if bundle.status == "captured but not analyzed":
        raise ValueError(
            f"screen {screen_token} in {segment} is captured but not analyzed"
        )
    frames = (
        []
        if bundle.legacy_schema or bundle.header_only
        else [
            _frame_view(bundle.segment_start, frame) for frame in bundle.frame_records
        ]
    )
    error_frames = (
        0
        if bundle.legacy_schema or bundle.header_only
        else sum(1 for frame in bundle.frame_records if "error" in frame)
    )

    return {
        "level": "4",
        "scope": _scope(day, stream, segment, screen_token),
        "data": {
            "summary": {
                "frames_analyzed": len(bundle.frame_records),
                "error_frames": error_frames,
                "legacy_schema": bundle.legacy_schema,
                "video_present": bundle.video_path is not None,
            },
            "frames": frames,
        },
    }


def show_frame_metadata(
    day: str, stream: str, segment: str, screen_token: str, frame_id: int
) -> dict[str, Any]:
    bundle = _load_analyzed_bundle(day, stream, segment, screen_token)
    if frame_id not in bundle.frame_index:
        raise FileNotFoundError(
            f"frame id {frame_id} not found in {screen_token} for {segment}"
        )

    frame = bundle.frame_index[frame_id]
    return {
        "level": "5a",
        "scope": _scope(day, stream, segment, screen_token) | {"frame_id": frame_id},
        "data": {
            "source": _source(bundle),
            "frame": frame,
            "computed": {
                "abs_time": _frame_abs_time(
                    bundle.segment_start, frame.get("timestamp", 0.0)
                ),
                "notes": _frame_notes(frame),
            },
        },
    }


def parse_frame_id_token(token: str) -> list[int]:
    parts = [part.strip() for part in token.split(",")]
    if any(not part for part in parts):
        raise ValueError(f"frame ids must be positive integers: got '{token}'")

    frame_ids: list[int] = []
    seen: set[int] = set()
    for part in parts:
        try:
            frame_id = int(part)
        except ValueError as exc:
            raise ValueError(
                f"frame ids must be positive integers: got '{token}'"
            ) from exc
        if frame_id < 1:
            raise ValueError(f"frame ids must be positive integers: got '{token}'")
        if frame_id in seen:
            raise ValueError(f"frame ids must be unique: {frame_id}")
        seen.add(frame_id)
        frame_ids.append(frame_id)
    return sorted(frame_ids)


def resolve_output_paths(out_path: str, frame_ids: list[int]) -> list[Path]:
    target = Path(out_path)
    if target.suffix.lower() not in SUPPORTED_OUTPUT_SUFFIXES:
        raise ValueError("--out must end in .png, .jpg, .jpeg, or .webp")
    if len(frame_ids) == 1:
        return [target]
    return [
        target.with_name(f"{target.stem}_{frame_id}{target.suffix}")
        for frame_id in frame_ids
    ]


def save_frame_images(
    day: str,
    stream: str,
    segment: str,
    screen_token: str,
    frame_ids: list[int],
    out_path: str,
    force: bool,
) -> dict[str, Any]:
    bundle = _load_analyzed_bundle(day, stream, segment, screen_token)
    if bundle.video_path is None:
        if bundle.jsonl_rel is not None:
            frame_id_token = ",".join(str(frame_id) for frame_id in frame_ids)
            command = (
                f"journal grab {day} {stream} {segment} {screen_token} {frame_id_token}"
            )
            raise FileNotFoundError(
                "raw video has been purged by retention; metadata-only access "
                f"remains via: {command}"
            )
        raise FileNotFoundError(
            f"raw video not found for screen {screen_token} in {segment}"
        )

    selected = []
    for frame_id in frame_ids:
        if frame_id not in bundle.frame_index:
            raise FileNotFoundError(
                f"frame id {frame_id} not found in {screen_token} for {segment}"
            )
        selected.append(bundle.frame_index[frame_id])

    output_paths = resolve_output_paths(out_path, frame_ids)
    conflicts = [str(path) for path in output_paths if path.exists() and not force]
    if conflicts:
        joined = ", ".join(conflicts)
        raise FileExistsError(f"output path exists (use --force): {joined}")

    images = decode_frames(bundle.video_path, selected, annotate_boxes=False)
    missing = [
        frame_id
        for frame_id, image in zip(frame_ids, images, strict=True)
        if image is None
    ]
    if missing:
        raise RuntimeError(
            f"failed to decode frame ids: {', '.join(str(i) for i in missing)}"
        )

    saved = []
    try:
        for frame, image, target in zip(selected, images, output_paths, strict=True):
            suffix = target.suffix.lower()
            save_kwargs = (
                {"quality": 95}
                if suffix in {".jpg", ".jpeg"}
                else {"quality": 90}
                if suffix == ".webp"
                else {}
            )
            assert image is not None
            image.save(target, **save_kwargs)
            saved.append(
                {"path": str(target), **_frame_view(bundle.segment_start, frame)}
            )
    finally:
        for image in images:
            if image is not None:
                image.close()

    return {
        "level": "5b" if len(frame_ids) == 1 else "5c",
        "scope": _scope(day, stream, segment, screen_token) | {"frame_ids": frame_ids},
        "data": {
            "source": _source(bundle),
            "saved": saved,
        },
    }


def emit_output(payload: dict[str, Any], *, as_json: bool) -> None:
    if as_json:
        print(json.dumps(payload, indent=2))
        return

    level = payload["level"]
    data = payload["data"]
    if level in TABLE_LEVELS:
        key, columns = TABLE_LEVELS[level]
        _print_table(columns, data[key])
        print()
        print(NEXT_FOOTERS[level])
        return
    if level == "4":
        summary = data["summary"]
        if summary["legacy_schema"]:
            print("0 frames analyzed: file uses pre-frame_id schema")
        elif summary["frames_analyzed"] == 0 and not data["frames"]:
            print("No qualified frames in this screen's analysis.")
        else:
            _print_table(
                ["frame_id", "timestamp", "abs_time", "primary", "notes"],
                data["frames"],
            )
            print()
            if summary["video_present"]:
                print(LEVEL_4_FOOTER)
            else:
                print(LEVEL_4_PURGED_FOOTER)
        return
    if level == "5a":
        scope = payload["scope"]
        computed = data["computed"]
        for label, value in (
            ("Screen", scope["screen"]),
            ("JSONL", data["source"]["jsonl"]),
            ("Video", data["source"]["video"]),
            ("Frame", scope["frame_id"]),
            ("Time", computed["abs_time"]),
        ):
            print(f"{label}: {value}")
        if computed["notes"]:
            print(f"Notes: {computed['notes']}")
        print()
        print(json.dumps(data["frame"], indent=2))
        print()
        print("Save: journal grab <day> <stream> <segment> <screen> <id> --out PATH")
        print(
            "Batch: journal grab <day> <stream> <segment> <screen> "
            "<id1>,<id2>,... --out PATH"
        )
        return
    if level in {"5b", "5c"}:
        for item in data["saved"]:
            print(f"saved {item['path']}")
        return

    raise ValueError(f"unsupported output level: {level}")


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Walk observed screen frames and optionally write frame images."
    )
    parser.add_argument(
        "args",
        nargs="*",
        help="Path tokens: [day] [stream] [segment] [screen] [frame-id[,frame-id...]]",
    )
    parser.add_argument(
        "--out",
        type=str,
        help="Write the selected frame image here (.png, .jpg, .jpeg, or .webp).",
    )
    parser.add_argument(
        "--force",
        action="store_true",
        help="Replace an existing output path.",
    )
    parser.add_argument(
        "--json",
        action="store_true",
        help="Emit JSON instead of table or plain output.",
    )
    args = setup_cli(parser)
    observe_utils_logger = logging.getLogger("solstone.observe.utils")
    previous_level = observe_utils_logger.level
    should_quiet = (
        observe_utils_logger.getEffectiveLevel() == logging.WARNING
        and not args.verbose
        and not args.debug
    )
    if should_quiet:
        observe_utils_logger.setLevel(logging.ERROR)

    try:
        require_solstone()

        tokens = list(args.args)
        if len(tokens) > 5:
            parser.error(
                "grab accepts at most 5 positional tokens: day stream segment screen frame-id"
            )
        if args.force and not args.out:
            parser.error("--force requires --out")
        if args.out and len(tokens) != 5:
            parser.error("--out requires day stream segment screen and frame-id")
        if args.out:
            try:
                resolve_output_paths(args.out, [1])
            except ValueError as exc:
                parser.error(str(exc))

        list_handlers = {
            0: lambda: list_available_days(),
            1: lambda: list_day_streams(tokens[0]),
            2: lambda: list_stream_segments(tokens[0], tokens[1]),
            3: lambda: list_segment_screens(tokens[0], tokens[1], tokens[2]),
            4: lambda: list_screen_frames(tokens[0], tokens[1], tokens[2], tokens[3]),
        }
        try:
            if len(tokens) < 5:
                payload = list_handlers[len(tokens)]()
            else:
                frame_ids = parse_frame_id_token(tokens[4])
                if len(frame_ids) > 1 and not args.out:
                    parser.error("multiple frame ids require --out")
                if args.out:
                    payload = save_frame_images(
                        tokens[0],
                        tokens[1],
                        tokens[2],
                        tokens[3],
                        frame_ids,
                        args.out,
                        args.force,
                    )
                else:
                    payload = show_frame_metadata(
                        tokens[0], tokens[1], tokens[2], tokens[3], frame_ids[0]
                    )
        except (FileNotFoundError, FileExistsError, RuntimeError, ValueError) as exc:
            raise SystemExit(str(exc)) from exc

        emit_output(payload, as_json=bool(args.json))
    finally:
        if should_quiet:
            observe_utils_logger.setLevel(previous_level)


if __name__ == "__main__":
    main()
