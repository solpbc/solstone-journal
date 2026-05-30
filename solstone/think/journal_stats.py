# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

import argparse
import json
import logging
import os
from collections import Counter
from datetime import datetime, timezone
from pathlib import Path
from typing import Dict

from solstone.observe.sense import scan_day as sense_scan_day
from solstone.observe.utils import VIDEO_EXTENSIONS, load_analysis_frames
from solstone.think.activities import estimate_duration_minutes, load_activity_records
from solstone.think.cluster import cluster_segments
from solstone.think.facets import get_facets
from solstone.think.pipeline_health import (
    BACKLOG_DEFAULT_WINDOW,
    BacklogDay,
    BacklogError,
    BacklogUnit,
    BacklogView,
    classify_segment_completion,
    read_backlog_view,
    read_segment_progress,
)
from solstone.think.stats_schema import DAY_FIELDS, SCHEMA_VERSION, TOTAL_FIELDS
from solstone.think.stats_schema import validate as validate_stats
from solstone.think.talents import scan_day as generate_scan_day
from solstone.think.utils import day_dirs, get_journal, segment_parse, setup_cli

logger = logging.getLogger(__name__)


def _serialize_backlog_error(error: BacklogError) -> dict:
    return {
        "day": error.day,
        "stage": error.stage,
        "message": error.message,
    }


def _serialize_backlog_unit(unit: BacklogUnit) -> dict:
    return {
        "mode": unit.mode,
        "name": unit.name,
        "facet": unit.facet,
        "stream": unit.stream,
        "segment": unit.segment,
        "why": unit.why,
        "provider": unit.provider,
        "model": unit.model,
        "trailing_fail_count": unit.trailing_fail_count,
        "last_fail_ts": unit.last_fail_ts,
        "stuck": unit.stuck,
    }


def _serialize_backlog_day(day: BacklogDay) -> dict:
    return {
        "day": day.day,
        "state": day.state,
        "segments": day.segments,
        "units": day.units,
        "not_sensed": day.not_sensed,
        "why": [_serialize_backlog_unit(unit) for unit in day.why],
        "error": _serialize_backlog_error(day.error) if day.error else None,
    }


def _serialize_backlog_view(view: BacklogView) -> dict:
    return {
        "window": view.window,
        "days": [_serialize_backlog_day(day) for day in view.days],
        "pending_days": view.pending_days,
        "stuck_days": view.stuck_days,
        "oldest_pending_day": view.oldest_pending_day,
        "errors": [_serialize_backlog_error(error) for error in view.errors],
        "degraded": view.degraded,
    }


def _empty_backlog_view() -> BacklogView:
    return BacklogView(
        window=BACKLOG_DEFAULT_WINDOW,
        days=(),
        pending_days=0,
        stuck_days=0,
        oldest_pending_day=None,
        errors=(),
    )


def _degraded_backlog_view() -> BacklogView:
    return BacklogView(
        window=BACKLOG_DEFAULT_WINDOW,
        days=(),
        pending_days=0,
        stuck_days=0,
        oldest_pending_day=None,
        errors=(),
        degraded=True,
    )


class JournalStats:
    def __init__(self) -> None:
        self.days: Dict[str, Dict[str, float | int]] = {}
        self.totals: Counter[str] = Counter()
        self.total_transcript_duration = 0.0
        self.total_percept_duration = 0.0
        self.agent_counts: Counter[str] = Counter()
        self.agent_minutes: Counter[str] = Counter()
        self.facet_counts: Counter[str] = Counter()
        self.facet_minutes: Counter[str] = Counter()
        self.heatmap: list[list[float]] = [[0.0 for _ in range(24)] for _ in range(7)]
        # Token usage tracking: {day: {model: {token_type: count}}}
        self.token_usage: Dict[str, Dict[str, Dict[str, int]]] = {}
        # Total token usage by model: {model: {token_type: count}}
        self.token_totals: Dict[str, Dict[str, int]] = {}
        # Per-day agent counts: {day: {agent: count}}
        self.agent_counts_by_day: Dict[str, Dict[str, int]] = {}
        # Per-day facet counts: {day: {facet: count}}
        self.facet_counts_by_day: Dict[str, Dict[str, int]] = {}
        self.backlog_view: BacklogView | None = None

    def _get_day_mtime(self, day_dir: Path) -> float:
        """Get latest modification time of files we scan."""
        files = []
        # Check segment subdirectories for processed files (day/stream/segment/)
        files.extend(day_dir.glob("*/*/*audio.jsonl"))
        files.extend(day_dir.glob("*/*/*_transcript.jsonl"))
        files.extend(day_dir.glob("*/*/*_transcript.md"))
        files.extend(day_dir.glob("*/*/*screen.jsonl"))
        # Check day root for unprocessed media files
        files.extend(day_dir.glob("*.flac"))
        files.extend(day_dir.glob("*.m4a"))
        for ext in VIDEO_EXTENSIONS:
            files.extend(day_dir.glob(f"*{ext}"))

        talents_dir = day_dir / "talents"
        if talents_dir.is_dir():
            files.extend(talents_dir.glob("*.json"))
            files.extend(talents_dir.glob("*.md"))
            files.extend(talents_dir.glob("*/*.json"))
            files.extend(talents_dir.glob("*/*.md"))

        files.extend(day_dir.glob("health/*.jsonl"))
        files.extend(day_dir.glob("health/*.updated"))

        if not files:
            return 0.0
        return max(f.stat().st_mtime for f in files)

    def _load_day_cache(self, day: str, day_dir: Path) -> dict | None:
        """Load cached day stats if fresh."""
        cache_file = day_dir / "stats.json"
        if not cache_file.exists():
            return None

        try:
            cache_mtime = cache_file.stat().st_mtime
            day_mtime = self._get_day_mtime(day_dir)

            if cache_mtime > day_mtime:
                with open(cache_file, encoding="utf-8") as f:
                    payload = json.load(f)
                if payload.get("schema_version") != SCHEMA_VERSION:
                    return None
                stats = payload.get("stats")
                if not isinstance(stats, dict):
                    return None
                if any(field not in stats for field in DAY_FIELDS):
                    return None
                return payload
        except Exception as e:
            logger.debug(f"Cache load failed for {day}: {e}")

        return None

    def _save_day_cache(self, day_dir: Path, stats: dict) -> None:
        """Save day stats to cache."""
        try:
            cache_file = day_dir / "stats.json"
            payload = dict(stats)
            day_stats = dict(payload.get("stats", {}))
            for field in DAY_FIELDS:
                day_stats.setdefault(field, 0)
            payload["schema_version"] = SCHEMA_VERSION
            payload["stats"] = day_stats
            with open(cache_file, "w", encoding="utf-8") as f:
                json.dump(payload, f, indent=2)
        except Exception as e:
            logger.debug(f"Cache save failed: {e}")

    def _parse_timestamp(self, ts: str) -> float:
        """Parse HH:MM:SS timestamp to seconds since midnight."""
        try:
            h, m, s = ts.split(":")
            return int(h) * 3600 + int(m) * 60 + int(s)
        except Exception:
            return 0.0

    def _calculate_audio_duration(self, segments: list) -> float:
        """Calculate audio duration from min/max timestamps."""
        timestamps = [seg.get("start") for seg in segments if seg.get("start")]
        if not timestamps:
            return 0.0

        times_seconds = [self._parse_timestamp(t) for t in timestamps]
        return max(times_seconds) - min(times_seconds)

    def _calculate_percept_duration(self, frames: list) -> float:
        """Calculate screen duration from min/max frame timestamps."""
        # Skip header (first element if it has no frame_id)
        frame_timestamps = [
            f["timestamp"] for f in frames if "timestamp" in f and "frame_id" in f
        ]
        if not frame_timestamps:
            return 0.0

        return max(frame_timestamps) - min(frame_timestamps)

    def _apply_day_stats(self, day: str, cached_data: dict) -> None:
        """Apply cached day stats to instance state."""
        # Extract components from cache
        stats = cached_data.get("stats", {})
        agent_data = cached_data.get("agent_data", {})
        heatmap_data = cached_data.get("heatmap_data", {})

        # Apply day stats
        self.days[day] = stats

        # Update totals (excluding per-day durations)
        counts_for_totals = {
            k: v
            for k, v in stats.items()
            if k not in ("transcript_duration", "percept_duration")
        }
        self.totals.update(counts_for_totals)

        # Accumulate durations
        self.total_transcript_duration += stats.get("transcript_duration", 0.0)
        self.total_percept_duration += stats.get("percept_duration", 0.0)

        # Apply agent data
        day_agent_counts: Dict[str, int] = {}
        for agent, data in agent_data.items():
            count = data.get("count", 0)
            self.agent_counts[agent] += count
            self.agent_minutes[agent] += data.get("minutes", 0.0)
            if count > 0:
                day_agent_counts[agent] = count
        if day_agent_counts:
            self.agent_counts_by_day[day] = day_agent_counts

        # Apply facet data
        facet_data = cached_data.get("facet_data", {})
        day_facet_counts: Dict[str, int] = {}
        for facet, data in facet_data.items():
            count = data.get("count", 0)
            self.facet_counts[facet] += count
            self.facet_minutes[facet] += data.get("minutes", 0.0)
            if count > 0:
                day_facet_counts[facet] = count
        if day_facet_counts:
            self.facet_counts_by_day[day] = day_facet_counts

        # Apply heatmap data
        weekday = heatmap_data.get("weekday")
        hours = heatmap_data.get("hours", {})
        if weekday is not None:
            for hour_str, minutes in hours.items():
                hour = int(hour_str)
                self.heatmap[weekday][hour] += minutes

    def scan_day(self, day: str, path: str) -> dict:
        """Scan a single day and return stats dict for caching."""
        stats: Counter[str] = Counter()
        transcript_duration = 0.0
        percept_duration = 0.0
        day_dir = Path(path)

        # Track agent data for cache
        agent_data = {}
        facet_data = {}
        heatmap_hours = {}

        # --- Transcript sessions ---
        # Check segment subdirectories for transcript JSONL files (day/stream/segment/)
        transcript_files = list(day_dir.glob("*/*/audio.jsonl"))
        transcript_files.extend(day_dir.glob("*/*/*_audio.jsonl"))
        transcript_files.extend(day_dir.glob("*/*/*_transcript.jsonl"))
        for jsonl_file in sorted(set(transcript_files)):
            stats["transcript_sessions"] += 1

            try:
                with open(jsonl_file, encoding="utf-8") as f:
                    lines = [line.strip() for line in f if line.strip()]

                if not lines:
                    logger.debug(f"Empty transcript file: {jsonl_file}")
                    continue

                # First line is metadata, rest are segments
                segments = []
                for i, line in enumerate(lines[1:], start=2):
                    try:
                        segments.append(json.loads(line))
                    except json.JSONDecodeError as e:
                        logger.debug(f"Invalid JSON at line {i} in {jsonl_file}: {e}")
                        continue

                stats["transcript_segments"] += len(segments)

                # Calculate duration from timestamps
                if segments:
                    duration = self._calculate_audio_duration(segments)
                    transcript_duration += duration

            except (OSError, IOError) as e:
                logger.warning(f"Error reading transcript file {jsonl_file}: {e}")
            except Exception as e:
                logger.warning(f"Unexpected error processing {jsonl_file}: {e}")

        # --- Screen sessions ---
        # Check segment subdirectories for screen files (day/stream/segment/)
        screen_files = list(day_dir.glob("*/*/screen.jsonl"))
        screen_files.extend(day_dir.glob("*/*/*_screen.jsonl"))
        for jsonl_file in sorted(screen_files):
            stats["percept_sessions"] += 1

            try:
                frames = load_analysis_frames(jsonl_file)
                if not frames:
                    logger.debug(f"No valid frames in: {jsonl_file}")
                    continue

                # Count frames (excluding header)
                frame_count = sum(1 for f in frames if "frame_id" in f)
                stats["percept_frames"] += frame_count

                # Calculate duration from timestamps
                if frame_count > 0:
                    duration = self._calculate_percept_duration(frames)
                    percept_duration += duration

            except (OSError, IOError) as e:
                logger.warning(f"Error reading screen file {jsonl_file}: {e}")
            except Exception as e:
                logger.warning(f"Unexpected error processing {jsonl_file}: {e}")

        # --- Pending segments (unprocessed media files) ---
        sense_info = sense_scan_day(day_dir)
        stats["pending_segments"] = sense_info["pending_segments"]

        # --- Insight summaries ---
        output_info = generate_scan_day(day)
        stats["outputs_processed"] = len(output_info["processed"])
        stats["outputs_pending"] = len(output_info["repairable"])

        try:
            completion = classify_segment_completion(
                cluster_segments(day),
                read_segment_progress(day),
            )
            stats["segments_pending_think"] = completion.not_thought
        except Exception:
            logger.warning(
                "journal_stats: segment completion fold failed for %s; "
                "segments_pending_think under-reported",
                day,
                exc_info=True,
            )
            stats["segments_pending_think"] = 0

        # --- Activities and heatmap from facets/*/activities/YYYYMMDD.jsonl ---
        weekday = datetime.strptime(day, "%Y%m%d").weekday()
        for facet_name, _facet_meta in get_facets().items():
            activities_file = (
                Path(get_journal())
                / "facets"
                / facet_name
                / "activities"
                / f"{day}.jsonl"
            )
            try:
                records = load_activity_records(facet_name, day)
                for record in records:
                    activity_type = record.get("activity") or "unknown"
                    segments = record.get("segments") or []
                    if not segments:
                        continue

                    if activity_type not in agent_data:
                        agent_data[activity_type] = {"count": 0, "minutes": 0.0}
                    agent_data[activity_type]["count"] += 1

                    duration_minutes = float(estimate_duration_minutes(segments))
                    agent_data[activity_type]["minutes"] += duration_minutes

                    if facet_name not in facet_data:
                        facet_data[facet_name] = {"count": 0, "minutes": 0.0}
                    facet_data[facet_name]["count"] += 1
                    facet_data[facet_name]["minutes"] += duration_minutes

                    # Build heatmap hours for this day
                    for seg in segments:
                        start, end = segment_parse(seg)
                        if start is None or end is None:
                            continue

                        start_sec = start.hour * 3600 + start.minute * 60 + start.second
                        end_sec = end.hour * 3600 + end.minute * 60 + end.second
                        cur = start_sec
                        while cur < end_sec:
                            hour = cur // 3600
                            if hour >= 24:
                                break
                            next_tick = min((hour + 1) * 3600, end_sec)
                            minutes = (next_tick - cur) / 60
                            heatmap_hours[str(hour)] = (
                                heatmap_hours.get(str(hour), 0.0) + minutes
                            )
                            cur = next_tick
            except (OSError, IOError) as e:
                logger.warning(f"Error reading {activities_file}: {e}")

        # --- Disk usage ---
        stats["day_bytes"] = sum(
            f.stat().st_size for f in day_dir.rglob("*") if f.is_file()
        )

        # --- Build return dict ---
        stats["transcript_duration"] = transcript_duration
        stats["percept_duration"] = percept_duration

        return {
            "stats": dict(stats),
            # NOTE: agent_data keys are now activity types (e.g., "meeting", "coding"), not extractor agent names. Key name retained for cache-format compatibility.
            "agent_data": agent_data,
            "facet_data": facet_data,
            "heatmap_data": {"weekday": weekday, "hours": heatmap_hours},
        }

    def scan_all_tokens(self, journal_path: Path, use_cache: bool = True) -> None:
        """Scan all token usage files in the tokens directory.

        Reads daily *.jsonl files (one JSON object per line).
        """
        tokens_dir = journal_path / "tokens"
        if not tokens_dir.is_dir():
            return

        today = datetime.now(timezone.utc).strftime("%Y%m%d")

        # Scan JSONL files only
        for token_file in tokens_dir.glob("*.jsonl"):
            day = token_file.stem
            cache_file = token_file.parent / f"{day}.tokens_cache.json"

            if use_cache and day != today and cache_file.exists():
                try:
                    if cache_file.stat().st_mtime > token_file.stat().st_mtime:
                        with open(cache_file, encoding="utf-8") as f:
                            cached = json.load(f)
                        self.token_usage[day] = cached
                        for model, counts in cached.items():
                            if model not in self.token_totals:
                                self.token_totals[model] = {}
                            for token_type, count in counts.items():
                                if token_type not in self.token_totals[model]:
                                    self.token_totals[model][token_type] = 0
                                self.token_totals[model][token_type] += count
                        continue
                except Exception as e:
                    logger.debug(f"Token cache load failed for {token_file}: {e}")

            try:
                with open(token_file, "r", encoding="utf-8") as f:
                    for line in f:
                        line = line.strip()
                        if not line:
                            continue
                        try:
                            data = json.loads(line)
                            self._process_token_entry(data)
                        except json.JSONDecodeError as e:
                            logger.debug(f"Invalid JSON in {token_file}: {e}")
                            continue

            except (OSError, IOError) as e:
                logger.warning(f"Error reading token file {token_file}: {e}")
                continue

            if use_cache and day != today:
                try:
                    with open(cache_file, "w", encoding="utf-8") as f:
                        json.dump(self.token_usage.get(day, {}), f)
                except Exception as e:
                    logger.debug(f"Token cache save failed for {token_file}: {e}")

    def _process_token_entry(self, data: dict) -> None:
        """Process a single token usage entry (expects normalized format)."""
        # Extract date from timestamp
        timestamp = data.get("timestamp")
        if not timestamp:
            return

        # Use UTC for consistent date extraction (timestamps are in UTC from time.time())
        file_date = datetime.fromtimestamp(timestamp, tz=timezone.utc).strftime(
            "%Y%m%d"
        )
        model = data.get("model", "unknown")
        usage = data.get("usage", {})

        # Initialize day's token usage if not exists
        if file_date not in self.token_usage:
            self.token_usage[file_date] = {}

        # Initialize model entry if not exists
        if model not in self.token_usage[file_date]:
            self.token_usage[file_date][model] = {}
        if model not in self.token_totals:
            self.token_totals[model] = {}

        # Add token counts (all fields are already normalized by migration)
        for token_type, count in usage.items():
            if not isinstance(count, int):
                continue

            # Add to day's model totals
            if token_type not in self.token_usage[file_date][model]:
                self.token_usage[file_date][model][token_type] = 0
            self.token_usage[file_date][model][token_type] += count

            # Add to overall model totals
            if token_type not in self.token_totals[model]:
                self.token_totals[model][token_type] = 0
            self.token_totals[model][token_type] += count

    def scan(self, journal: str, verbose: bool = False, use_cache: bool = True) -> None:
        days_map = day_dirs()
        sorted_days = sorted(days_map.items())
        cache_hits = 0
        cache_misses = 0

        for idx, (day, path) in enumerate(sorted_days, 1):
            if not os.path.isdir(path):
                continue

            day_dir = Path(path)

            # Try cache first
            cached_data = None
            if use_cache:
                cached_data = self._load_day_cache(day, day_dir)

            if cached_data:
                # Cache hit - apply cached data
                self._apply_day_stats(day, cached_data)
                cache_hits += 1
                if verbose:
                    print(
                        f"[{idx}/{len(sorted_days)}] {day} (cached)",
                        end="\r",
                        flush=True,
                    )
            else:
                # Cache miss - scan and save
                cache_misses += 1
                if verbose:
                    print(
                        f"[{idx}/{len(sorted_days)}] Scanning {day}...",
                        end="\r",
                        flush=True,
                    )
                day_data = self.scan_day(day, path)
                self._apply_day_stats(day, day_data)

                if use_cache:
                    self._save_day_cache(day_dir, day_data)

        try:
            self.backlog_view = read_backlog_view()
        except Exception:
            logger.exception(
                "backlog derivation failed; stats will be flagged degraded"
            )
            self.backlog_view = _degraded_backlog_view()

        # Scan tokens directory once after all days are processed
        self.scan_all_tokens(Path(journal), use_cache=use_cache)

        if verbose:
            cache_status = (
                f" (cache: {cache_hits} hits, {cache_misses} misses)"
                if use_cache
                else ""
            )
            logger.info(
                f"Scanned {len(self.days)} days, "
                f"{self.totals.get('transcript_sessions', 0)} transcript sessions, "
                f"{self.totals.get('percept_sessions', 0)} percept sessions"
                f"{cache_status}"
            )

    def to_dict(self) -> dict:
        """Return a dictionary with all collected statistics."""
        days = {
            day: {field: stats.get(field, 0) for field in DAY_FIELDS}
            for day, stats in self.days.items()
        }
        backlog_view = self.backlog_view or _empty_backlog_view()
        totals = dict(self.totals)
        totals["transcript_duration"] = self.total_transcript_duration
        totals["percept_duration"] = self.total_percept_duration
        totals["total_transcript_duration"] = self.total_transcript_duration
        totals["total_percept_duration"] = self.total_percept_duration
        totals["backlog_pending_days"] = backlog_view.pending_days
        totals["backlog_stuck_days"] = backlog_view.stuck_days
        for field in TOTAL_FIELDS:
            totals.setdefault(field, 0)
        return {
            "schema_version": SCHEMA_VERSION,
            "generated_at": datetime.now(timezone.utc).isoformat(),
            "day_count": len(self.days),
            "days": days,
            "totals": totals,
            "heatmap": self.heatmap,
            "tokens": {
                "by_day": self.token_usage,
                "by_model": self.token_totals,
            },
            "talents": {
                "counts": dict(self.agent_counts),
                "minutes": {k: round(v, 2) for k, v in self.agent_minutes.items()},
                "counts_by_day": self.agent_counts_by_day,
            },
            "facets": {
                "counts": dict(self.facet_counts),
                "minutes": {k: round(v, 2) for k, v in self.facet_minutes.items()},
                "counts_by_day": self.facet_counts_by_day,
            },
            "backlog": _serialize_backlog_view(backlog_view),
        }

    def save_json(self, journal: str) -> None:
        """Write full statistics to ``stats.json`` in ``journal``."""
        data = self.to_dict()
        errors = validate_stats(data)
        if errors:
            raise ValueError(f"Stats validation failed: {'; '.join(errors)}")
        path = os.path.join(journal, "stats.json")
        with open(path, "w", encoding="utf-8") as f:
            json.dump(data, f, indent=2)


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Scan a solstone journal and generate statistics"
    )
    parser.add_argument(
        "--no-cache",
        action="store_true",
        help="Disable per-day caching (force re-scan all days)",
    )
    args = setup_cli(parser)
    journal = get_journal()

    js = JournalStats()
    js.scan(journal, verbose=args.verbose, use_cache=not args.no_cache)

    try:
        js.save_json(journal)
        logger.info(f"Statistics saved to {journal}/stats.json")
    except Exception as e:
        logger.error(f"Error writing stats.json: {e}")
        raise


if __name__ == "__main__":
    main()
