# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""CLI for inspecting talent prompt configurations.

Lists all system and app prompts with their frontmatter metadata,
supports filtering by schedule and source, and provides detail views.

Usage:
    journal talent                          List all prompts grouped by schedule
    journal talent list --schedule daily    Filter by schedule type
    journal talent list --json              Output all configs as JSONL
    journal talent show <name>              Show details for a specific prompt
    journal talent show <name> --json       Output a single prompt as JSONL
    journal talent show <name> --prompt     Show full prompt context (dry-run)
    journal talent logs                     Show recent talent runs
    journal talent logs <agent> -c 5        Show last 5 runs for a talent
    journal talent log <id>                 Show events for a talent run
    journal talent log <id> --json          Output raw JSONL events
    journal talent log <id> --full          Show expanded event details
"""

from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
import time
from collections import deque
from datetime import datetime, timedelta
from pathlib import Path
from typing import Any

import frontmatter

from solstone.think.talent import (
    TALENT_DIR,
    _load_prompt_metadata,
    get_talent_configs,
)
from solstone.think.utils import day_path, setup_cli

# Package root for computing logical package-relative paths
_PACKAGE_ROOT = Path(__file__).parent.parent

# Internal bookkeeping keys to exclude from JSONL output
_INTERNAL_KEYS = frozenset({"path", "mtime"})


def _relative_path(abs_path: str) -> str:
    """Convert absolute path to package-relative path."""
    try:
        return str(Path(abs_path).relative_to(_PACKAGE_ROOT))
    except ValueError:
        return abs_path


def _resolve_md_path(name: str) -> Path:
    """Resolve a prompt name to its .md file path."""
    if ":" in name:
        app, agent_name = name.split(":", 1)
        return _PACKAGE_ROOT / "apps" / app / "talent" / f"{agent_name}.md"
    return TALENT_DIR / f"{name}.md"


def _scan_variables(body: str) -> list[str]:
    """Scan prompt body text for $template variables."""
    # Match $word or ${word} but not $$ (escaped dollar signs)
    matches = re.findall(r"(?<!\$)\$\{?([a-zA-Z_]\w*)\}?", body)
    # Deduplicate preserving order
    seen: set[str] = set()
    result: list[str] = []
    for m in matches:
        if m not in seen:
            seen.add(m)
            result.append(m)
    return result


def _format_last_run(key: str, talents_dir: Path) -> tuple[str, bool]:
    """Format age of last run with optional runtime duration.

    Returns (display_string, failed) where failed is True if the last
    event in the log was an error.
    """
    safe_name = key.replace(":", "--")
    link_path = talents_dir / f"{safe_name}.log"
    if not link_path.exists():
        return "-", False

    try:
        with link_path.open() as f:
            first_line = f.readline()
            last_line = next(iter(deque(f, maxlen=1)), None)

        first_event = json.loads(first_line)
        first_ts = first_event["ts"]
        age_seconds = time.time() - (first_ts / 1000)

        if age_seconds < 60:
            age = f"{int(age_seconds)}s ago"
        elif age_seconds < 3600:
            age = f"{int(age_seconds / 60)}m ago"
        elif age_seconds < 86400:
            age = f"{int(age_seconds / 3600)}h ago"
        else:
            age = f"{int(age_seconds / 86400)}d ago"

        failed = False
        if last_line:
            last_event = json.loads(last_line)
            failed = last_event.get("event") == "error"
            last_ts = last_event["ts"]
            duration_seconds = (last_ts - first_ts) / 1000
            if duration_seconds < 60:
                duration = f"{int(duration_seconds)}s"
            elif duration_seconds < 3600:
                duration = f"{int(duration_seconds / 60)}m"
            else:
                duration = f"{int(duration_seconds / 3600)}h"
            age = f"{age} ({duration})"

        return age, failed
    except Exception:
        return "-", False


def _format_tags(info: dict[str, Any], *, failed: bool = False) -> str:
    """Build compact space-separated tags string."""
    tags: list[str] = []

    output = info.get("output")
    if output == "json":
        tags.append("json")
    elif output:
        tags.append("md")

    hook = info.get("hook")
    if hook:
        if isinstance(hook, dict):
            if hook.get("pre"):
                tags.append("pre")
            if hook.get("post"):
                tags.append("post")
        else:
            tags.append("hook")

    if info.get("disabled"):
        tags.append("disabled")

    if failed:
        tags.append("FAIL")

    return " ".join(tags)


def _collect_configs(
    *,
    schedule: str | None = None,
    source: str | None = None,
    include_disabled: bool = False,
) -> dict[str, dict[str, Any]]:
    """Collect all talent configs with optional filters applied."""
    configs = get_talent_configs(schedule=schedule, include_disabled=True)

    filtered: dict[str, dict[str, Any]] = {}
    for key, info in configs.items():
        if not include_disabled and info.get("disabled", False):
            continue
        if source and info.get("source") != source:
            continue
        filtered[key] = info

    return filtered


def _to_jsonl_record(key: str, info: dict[str, Any]) -> dict[str, Any]:
    """Build a clean JSONL record from a config entry."""
    record: dict[str, Any] = {"file": _relative_path(str(info["path"]))}
    for k, v in info.items():
        if k not in _INTERNAL_KEYS:
            record[k] = v
    return record


def list_prompts(
    *,
    schedule: str | None = None,
    source: str | None = None,
    include_disabled: bool = False,
) -> None:
    """Print prompts grouped by schedule."""
    configs = _collect_configs(
        schedule=schedule, source=source, include_disabled=include_disabled
    )
    from solstone.think.utils import get_journal

    talents_dir = Path(get_journal()) / "talents"

    if not configs:
        print("No prompts found matching filters.")
        return

    # Group by schedule
    groups: dict[str, list[tuple[str, dict[str, Any]]]] = {
        "segment": [],
        "daily": [],
        "weekly": [],
        "activity": [],
        "unscheduled": [],
    }

    for key, info in sorted(configs.items()):
        sched = info.get("schedule")
        if sched in ("segment", "daily", "weekly", "activity"):
            groups[sched].append((key, info))
        else:
            groups["unscheduled"].append((key, info))

    # Compute column widths
    all_names = list(configs.keys())
    name_width = max(len(n) for n in all_names) if all_names else 20
    name_width = max(name_width, 10)

    # Fixed widths for other columns
    title_width = 28
    last_run_width = 18

    # Print column header
    header = (
        f"  {'NAME':<{name_width}}  {'TITLE':<{title_width}}  "
        f"{'LAST RUN':<{last_run_width}}  TAGS"
    )
    print(header)
    print()

    # Print each non-empty group
    for group_name in ("segment", "daily", "activity", "unscheduled"):
        items = groups[group_name]
        if not items:
            continue

        # Skip group header if filtering to a single schedule
        if not schedule:
            print(f"{group_name}:")

        for key, info in items:
            title = info.get("title", "")[:title_width]
            last_run_str, failed = _format_last_run(key, talents_dir)
            last_run = last_run_str[:last_run_width]
            tags = _format_tags(info, failed=failed)
            src = ""
            if info.get("source") == "app":
                src = f" [{info.get('app', 'app')}]"

            tag_part = f"  {tags}" if tags else ""
            line = (
                f"  {key:<{name_width}}  {title:<{title_width}}  "
                f"{last_run:<{last_run_width}}{tag_part}{src}"
            )
            print(line.rstrip())

        if not schedule:
            print()

    # Show disabled count hint
    if not include_disabled:
        all_configs = _collect_configs(
            schedule=schedule, source=source, include_disabled=True
        )
        disabled_count = len(all_configs) - len(configs)
        if disabled_count:
            print(
                f"{len(configs)} prompts ({disabled_count} disabled hidden, use --disabled)"
            )


def show_prompt(name: str, *, as_json: bool = False) -> None:
    """Print detailed info for a single prompt."""
    md_path = _resolve_md_path(name)

    if not md_path.exists():
        print(f"Prompt not found: {name}", file=sys.stderr)
        print(f"  looked at: {_relative_path(str(md_path))}", file=sys.stderr)
        sys.exit(1)

    info = _load_prompt_metadata(md_path)
    rel_path = _relative_path(str(md_path))

    # Load body once for variables and line count
    try:
        post = frontmatter.load(md_path)
        body = post.content.strip()
    except Exception:
        body = None

    if as_json:
        record = _to_jsonl_record(name, info)
        print(json.dumps(record, default=str))
        return

    print(f"\n{rel_path}\n")

    # Display frontmatter fields
    # Order: title, description, key config fields, then alphabetical for the rest
    priority_keys = [
        "title",
        "description",
        "schedule",
        "priority",
        "output",
        "tools",
        "hook",
        "color",
    ]
    skip_keys = {"path", "mtime"}

    label_width = 14

    def print_field(key: str, value: Any) -> None:
        if key in skip_keys:
            return
        val_str = str(value)
        # Truncate long descriptions for readability
        if key == "description" and len(val_str) > 72:
            val_str = val_str[:72] + "..."
        # Format hook config nicely
        if key == "hook" and isinstance(value, dict):
            post_hook = value.get("post", "")
            if post_hook:
                val_str = f"post: {post_hook}"
        print(f"  {key + ':':<{label_width}} {val_str}")

    printed: set[str] = set()
    for key in priority_keys:
        if key in info and key not in skip_keys:
            print_field(key, info[key])
            printed.add(key)

    # Remaining fields alphabetically
    for key in sorted(info.keys()):
        if key not in printed and key not in skip_keys:
            print_field(key, info[key])

    # Template variables and body line count from single parse
    if body is not None:
        variables = _scan_variables(body)
        if variables:
            vars_str = ", ".join(f"${v}" for v in variables)
            print(f"  {'variables:':<{label_width}} {vars_str}")

        line_count = len(body.splitlines())
        print(f"  {'body:':<{label_width}} {line_count} lines")

    print()


def json_output(
    *,
    schedule: str | None = None,
    source: str | None = None,
    include_disabled: bool = False,
) -> None:
    """Print JSONL output with one config per line, including filename."""
    configs = _collect_configs(
        schedule=schedule, source=source, include_disabled=include_disabled
    )

    for key, info in sorted(configs.items()):
        print(json.dumps(_to_jsonl_record(key, info), default=str))


def _truncate_content(text: str, max_lines: int = 100) -> tuple[str, int]:
    """Truncate text to max_lines, returning (text, omitted_count)."""
    lines = text.splitlines()
    if len(lines) <= max_lines:
        return text, 0
    # Show first half and last half
    half = max_lines // 2
    truncated = (
        lines[:half]
        + ["", f"... ({len(lines) - max_lines} lines omitted)"]
        + lines[-half:]
    )
    return "\n".join(truncated), len(lines) - max_lines


def _format_section(title: str, content: str, full: bool = False) -> None:
    """Print a section with header and content."""
    print(f"\n{'=' * 60}")
    print(f"  {title}")
    print(f"{'=' * 60}\n")
    if not content or not content.strip():
        print("(empty)")
    elif full:
        print(content)
    else:
        truncated, omitted = _truncate_content(content)
        print(truncated)
        if omitted:
            print(f"\n(use --full to see all {omitted + 100} lines)")


def _yesterday() -> str:
    """Return yesterday's date in YYYYMMDD format."""
    return (datetime.now() - timedelta(days=1)).strftime("%Y%m%d")


def show_prompt_context(
    name: str,
    *,
    day: str | None = None,
    segment: str | None = None,
    facet: str | None = None,
    activity: str | None = None,
    query: str | None = None,
    full: bool = False,
) -> None:
    """Show full prompt context via dry-run.

    Builds config and pipes to `sol solstone.think.talents --dry-run` to show exactly
    what would be sent to the LLM provider.
    """
    # Load prompt metadata
    configs = get_talent_configs(include_disabled=True)
    if name not in configs:
        print(f"Prompt not found: {name}", file=sys.stderr)
        sys.exit(1)

    info = configs[name]
    prompt_type = info.get("type", "prompt")
    schedule = info.get("schedule")
    is_multi_facet = info.get("multi_facet", False)

    # Validate day format if provided
    if day and (len(day) != 8 or not day.isdigit()):
        print(f"Invalid --day format: {day}. Expected YYYYMMDD.", file=sys.stderr)
        sys.exit(1)

    # Validate arguments based on type and schedule
    if prompt_type == "generate":
        # Generators need day, and segment-scheduled need segment
        if schedule == "segment" and not segment:
            print(
                f"Prompt '{name}' is segment-scheduled. Use --segment HHMMSS_LEN",
                file=sys.stderr,
            )
            sys.exit(1)
        if not day:
            day = _yesterday()
            print(f"Using day: {day} (yesterday)")
    elif prompt_type == "prompt":
        print(
            f"Prompt '{name}' is a hook prompt and cannot be run directly.",
            file=sys.stderr,
        )
        sys.exit(1)

    # Activity-scheduled agents need --facet and --activity
    if schedule == "activity":
        if not facet:
            try:
                from solstone.think.facets import get_facets

                facets = get_facets()
                facet_names = [
                    k for k, v in facets.items() if not v.get("muted", False)
                ]
                print(
                    f"Prompt '{name}' is activity-scheduled. Use --facet NAME",
                    file=sys.stderr,
                )
                print(f"Available facets: {', '.join(facet_names)}", file=sys.stderr)
            except Exception:
                print(
                    f"Prompt '{name}' is activity-scheduled. Use --facet NAME",
                    file=sys.stderr,
                )
            sys.exit(1)

        if not day:
            day = _yesterday()
            print(f"Using day: {day} (yesterday)")

        if not activity:
            from solstone.think.activities import load_activity_records

            records = load_activity_records(facet, day)
            if not records:
                print(
                    f"No activity records for facet '{facet}' on {day}",
                    file=sys.stderr,
                )
                sys.exit(1)
            print(
                f"Prompt '{name}' is activity-scheduled. Use --activity ID",
                file=sys.stderr,
            )
            print(f"Activities for {facet} on {day}:", file=sys.stderr)
            for r in records:
                desc = r.get("description", "")
                if len(desc) > 50:
                    desc = desc[:50] + "..."
                print(
                    f"  {r['id']}  ({r.get('activity', '?')})  {desc}", file=sys.stderr
                )
            sys.exit(1)

    if is_multi_facet and not facet:
        # List available facets
        try:
            from solstone.think.facets import get_facets

            facets = get_facets()
            facet_names = [k for k, v in facets.items() if not v.get("muted", False)]
            print(
                f"Prompt '{name}' is multi-facet. Use --facet NAME",
                file=sys.stderr,
            )
            print(f"Available facets: {', '.join(facet_names)}", file=sys.stderr)
        except Exception:
            print(
                f"Prompt '{name}' is multi-facet. Use --facet NAME",
                file=sys.stderr,
            )
        sys.exit(1)

    # Build config for dry-run
    config: dict[str, Any] = {"name": name}

    if schedule == "activity":
        # Build activity config matching thinking.py:run_activity_prompts()
        from solstone.think.activities import (
            get_activity_output_path,
            load_activity_records,
        )

        records = load_activity_records(facet, day)
        record = None
        for r in records:
            if r.get("id") == activity:
                record = r
                break

        if not record:
            print(
                f"Activity '{activity}' not found in facet '{facet}' on {day}",
                file=sys.stderr,
            )
            sys.exit(1)

        segments = record.get("segments", [])
        if not segments:
            print(f"Activity '{activity}' has no segments", file=sys.stderr)
            sys.exit(1)

        output_format = info.get("output", "md")
        config["day"] = day
        config["facet"] = facet
        config["span"] = segments
        config["activity"] = record
        config["output"] = output_format
        config["output_path"] = str(
            get_activity_output_path(facet, day, activity, name, output_format)
        )
    elif prompt_type == "generate":
        config["day"] = day
        config["output"] = info.get("output", "md")
        if segment:
            config["segment"] = segment
        if facet:
            config["facet"] = facet
    else:
        # Cogitate prompt - use get_talent() to build full config with instructions
        from solstone.think.talent import get_talent

        try:
            agent_config = get_talent(name, facet=facet)
            config.update(agent_config)
        except Exception as e:
            print(f"Failed to load talent config: {e}", file=sys.stderr)
            sys.exit(1)

        # Override prompt with user query
        if query:
            config["prompt"] = query
        else:
            config["prompt"] = "(no --query provided)"

    # Run sol solstone.think.talents --dry-run
    config_json = json.dumps(config)
    try:
        result = subprocess.run(
            ["sol", "solstone.think.talents", "--dry-run"],
            input=config_json + "\n",
            capture_output=True,
            text=True,
            timeout=30,
        )
    except subprocess.TimeoutExpired:
        print("Dry-run timed out", file=sys.stderr)
        sys.exit(1)
    except FileNotFoundError:
        print("Could not find 'sol' command", file=sys.stderr)
        sys.exit(1)

    if result.returncode != 0:
        print(f"Dry-run failed: {result.stderr}", file=sys.stderr)
        sys.exit(1)

    # Parse JSONL output to find dry_run event
    dry_run_event = None
    for line in result.stdout.strip().splitlines():
        if not line:
            continue
        try:
            event = json.loads(line)
            if event.get("event") == "dry_run":
                dry_run_event = event
                break
            elif event.get("event") == "error":
                print(f"Error: {event.get('error')}", file=sys.stderr)
                sys.exit(1)
        except json.JSONDecodeError:
            continue

    if not dry_run_event:
        print("No dry_run event received", file=sys.stderr)
        if result.stderr:
            print(result.stderr, file=sys.stderr)
        sys.exit(1)

    # Format and display output
    print(f"\n  Dry-run for: {name} ({dry_run_event.get('type', 'unknown')})")
    print(f"  Provider: {dry_run_event.get('provider')} / {dry_run_event.get('model')}")
    if dry_run_event.get("day"):
        print(f"  Day: {dry_run_event.get('day')}")
    if dry_run_event.get("segment"):
        print(f"  Segment: {dry_run_event.get('segment')}")
    if activity:
        act_type = config.get("activity", {}).get("activity", "unknown")
        span = config.get("span", [])
        print(f"  Activity: {activity} ({act_type}, {len(span)} segments)")
        print(f"  Facet: {facet}")
    if dry_run_event.get("output_path"):
        print(f"  Output: {dry_run_event.get('output_path')}")

    # Pre-hook info
    if dry_run_event.get("pre_hook"):
        mods = dry_run_event.get("pre_hook_modifications", [])
        print(
            f"  Pre-hook: {dry_run_event.get('pre_hook')} (modified: {', '.join(mods) or 'none'})"
        )

    # System instruction (show before first if pre-hook modified it)
    if dry_run_event.get("system_instruction_before"):
        _format_section(
            "SYSTEM INSTRUCTION (before pre-hook)",
            dry_run_event.get("system_instruction_before", ""),
            full=full,
        )
    _format_section(
        f"SYSTEM INSTRUCTION (source: {dry_run_event.get('system_instruction_source', 'unknown')})",
        dry_run_event.get("system_instruction", ""),
        full=full,
    )

    # User instruction (agents only, show before first if pre-hook modified it)
    if dry_run_event.get("user_instruction"):
        if dry_run_event.get("user_instruction_before"):
            _format_section(
                "USER INSTRUCTION (before pre-hook)",
                dry_run_event.get("user_instruction_before", ""),
                full=full,
            )
        _format_section(
            "USER INSTRUCTION", dry_run_event.get("user_instruction", ""), full=full
        )

    # Extra context (agents only)
    if dry_run_event.get("extra_context"):
        _format_section(
            "EXTRA CONTEXT", dry_run_event.get("extra_context", ""), full=full
        )

    # Prompt (show before first if pre-hook modified it)
    prompt_source = dry_run_event.get("prompt_source", "")
    if prompt_source:
        prompt_source = f" (source: {_relative_path(prompt_source)})"
    if dry_run_event.get("prompt_before"):
        _format_section(
            "PROMPT (before pre-hook)",
            dry_run_event.get("prompt_before", ""),
            full=full,
        )
    _format_section(
        f"PROMPT{prompt_source}", dry_run_event.get("prompt", ""), full=full
    )

    # Transcript (generators only, show before first if pre-hook modified it)
    if "transcript" in dry_run_event:
        chars = dry_run_event.get("transcript_chars", 0)
        files = dry_run_event.get("transcript_files", 0)
        if dry_run_event.get("transcript_before"):
            before_chars = dry_run_event.get("transcript_before_chars", 0)
            _format_section(
                f"TRANSCRIPT (before pre-hook, {before_chars:,} chars)",
                dry_run_event.get("transcript_before", ""),
                full=full,
            )
        _format_section(
            f"TRANSCRIPT ({chars:,} chars from {files} files)",
            dry_run_event.get("transcript", ""),
            full=full,
        )

    # Tools (agents only)
    if dry_run_event.get("tools"):
        tools = dry_run_event.get("tools", [])
        if isinstance(tools, list):
            tools_str = ", ".join(tools)
        else:
            tools_str = str(tools)
        print(f"\n{'=' * 60}")
        print("  TOOLS")
        print(f"{'=' * 60}\n")
        print(tools_str)

    print()


def _find_run_file(talents_dir: Path, use_id: str) -> Path | None:
    """Locate a talent run JSONL file by ID."""
    for match in talents_dir.glob(f"*/{use_id}.jsonl"):
        return match
    for match in talents_dir.glob(f"*/{use_id}_active.jsonl"):
        return match
    return None


def _parse_run_stats(jsonl_path: Path) -> dict[str, Any]:
    """Parse an agent JSONL file for summary statistics."""
    stats: dict[str, Any] = {
        "event_count": 0,
        "tool_count": 0,
        "model": None,
        "usage": None,
        "request": None,
    }
    for line in jsonl_path.read_text().splitlines():
        if not line.strip():
            continue
        try:
            event = json.loads(line)
        except json.JSONDecodeError:
            continue
        etype = event.get("event")
        if etype == "request":
            stats["request"] = event
            continue
        stats["event_count"] += 1
        if etype == "tool_start":
            stats["tool_count"] += 1
        elif etype == "start":
            stats["model"] = event.get("model")
        elif etype == "finish":
            stats["usage"] = event.get("usage")
    return stats


def _format_bytes(n: int) -> str:
    """Format byte count as human-readable string."""
    if n < 1000:
        return str(n)
    elif n < 1_000_000:
        return f"{n / 1000:.1f}K"
    else:
        return f"{n / 1_000_000:.1f}M"


def _format_cost(cost_usd: float | None) -> str:
    """Format USD cost as rounded cents."""
    if cost_usd is None:
        return "-"
    cents = round(cost_usd * 100)
    if cents == 0 and cost_usd > 0:
        return "<1¢"
    return f"{cents}¢"


def _get_output_size(request_event: dict[str, Any], journal_root: str) -> int | None:
    """Get output file size in bytes from a request event, or None."""
    from solstone.think.talent import get_output_path

    req_output = request_event.get("output")
    if not req_output:
        return None

    # Prefer explicit output_path (set for activity agents, custom paths)
    if request_event.get("output_path"):
        out_path = Path(request_event["output_path"])
    else:
        req_day = request_event.get("day")
        if not req_day:
            return None
        req_segment = request_event.get("segment")
        req_facet = request_event.get("facet")
        req_name = request_event["name"]
        req_env = request_event.get("env") or {}
        req_stream = req_env.get("SOL_STREAM") if req_env else None
        day_dir = day_path(req_day, create=False)
        out_path = get_output_path(
            day_dir,
            req_name,
            segment=req_segment,
            output_format=req_output,
            facet=req_facet,
            stream=req_stream,
        )

    if out_path.exists():
        return out_path.stat().st_size
    return None


def _print_summary(records: list[dict[str, Any]]) -> None:
    """Print grouped summary of talent runs."""
    from collections import defaultdict

    groups: dict[str, list[dict[str, Any]]] = defaultdict(list)
    for r in records:
        groups[r.get("name", "unknown")].append(r)

    total_pass = 0
    total_fail = 0
    total_runtime = 0.0

    for name in sorted(groups):
        runs = groups[name]
        passed = sum(1 for r in runs if r.get("status") == "completed")
        failed = len(runs) - passed
        runtimes = [r.get("runtime_seconds") or 0 for r in runs]
        min_rt = min(runtimes)
        max_rt = max(runtimes)
        total_rt = sum(runtimes)

        total_pass += passed
        total_fail += failed
        total_runtime += total_rt

        if min_rt == max_rt:
            rt_str = f"{min_rt:.1f}s"
        else:
            rt_str = f"{min_rt:.1f}s–{max_rt:.1f}s"

        status_str = f"{passed}✓"
        if failed:
            status_str += f" {failed}✗"

        print(f"  {name:<20} {status_str:<10} {rt_str}")

    print(f"  {'—' * 40}")
    status_str = f"{total_pass}✓"
    if total_fail:
        status_str += f" {total_fail}✗"
    print(f"  {'total':<20} {status_str:<10} {total_runtime:.1f}s")


def logs_runs(
    *,
    agent: str | None = None,
    count: int | None = None,
    day: str | None = None,
    daily: bool = False,
    errors: bool = False,
    summary: bool = False,
) -> None:
    """Print one-line summaries of recent talent runs from day-index files."""
    from solstone.think.models import calc_agent_cost
    from solstone.think.utils import get_journal

    journal_root = get_journal()
    talents_dir = Path(journal_root) / "talents"
    if not talents_dir.is_dir():
        return

    # Validate --day format
    if day and (len(day) != 8 or not day.isdigit()):
        print(f"Invalid --day format: {day}. Expected YYYYMMDD.", file=sys.stderr)
        sys.exit(1)

    # Resolve default count: 50 for --daily, 20 otherwise
    if count is None:
        count = 50 if daily else 20

    # Find day-index files, most recent first
    if day:
        day_file = talents_dir / f"{day}.jsonl"
        day_files = [day_file] if day_file.is_file() else []
    else:
        day_files = sorted(talents_dir.glob("????????.jsonl"), reverse=True)
    if not day_files:
        return

    # Collect records across day files
    records: list[dict[str, Any]] = []
    _schedule_lookup: dict[str, str | None] | None = None
    for day_file in day_files:
        for line in day_file.read_text().splitlines():
            if not line.strip():
                continue
            try:
                record = json.loads(line)
            except json.JSONDecodeError:
                continue
            if agent and record.get("name") != agent:
                continue
            if errors and record.get("status") != "error":
                continue
            if daily:
                rec_schedule = record.get("schedule")
                if rec_schedule is None:
                    if _schedule_lookup is None:
                        all_configs = get_talent_configs(include_disabled=True)
                        _schedule_lookup = {
                            key: info.get("schedule")
                            for key, info in all_configs.items()
                        }
                    rec_schedule = _schedule_lookup.get(record.get("name"))
                if rec_schedule != "daily":
                    continue
            records.append(record)
        if len(records) >= count:
            break

    if not records:
        return

    # Sort by timestamp descending and trim
    records.sort(key=lambda r: r.get("ts", 0), reverse=True)
    records = records[:count]

    if summary:
        _print_summary(records)
        return

    # Compute column widths
    name_width = max((len(r.get("name", "")) for r in records), default=10)
    name_width = max(name_width, 10)

    for r in records:
        use_id = r.get("use_id")
        run_file = (
            _find_run_file(talents_dir, use_id) if isinstance(use_id, str) else None
        )
        stats: dict[str, Any] = {
            "event_count": 0,
            "tool_count": 0,
            "model": None,
            "usage": None,
            "request": None,
        }
        cost_usd: float | None = None
        output_size: int | None = None
        if run_file:
            stats = _parse_run_stats(run_file)
            cost_usd = calc_agent_cost(stats["model"] or r.get("model"), stats["usage"])
            request_event = stats.get("request")
            if isinstance(request_event, dict):
                output_size = _get_output_size(request_event, journal_root)
        r["_run_file"] = run_file
        r["_stats"] = stats
        r["_cost_usd"] = cost_usd
        r["_output_size"] = output_size

    today = datetime.now().strftime("%Y%m%d")
    use_color = sys.stdout.isatty()

    for r in records:
        run_file = r.get("_run_file")
        stats = r.get("_stats") or {}
        cost_usd = r.get("_cost_usd")
        output_size = r.get("_output_size")
        use_id = r.get("use_id", "")

        ts = r.get("ts", 0)
        dt = datetime.fromtimestamp(ts / 1000)
        day = r.get("day", dt.strftime("%Y%m%d"))

        # Time column
        if day == today:
            time_str = dt.strftime("%H:%M")
        else:
            time_str = dt.strftime("%b %d %H:%M")

        name = r.get("name", "unknown")
        status = r.get("status", "")
        status_sym = "\u2713" if status == "completed" else "\u2717"
        runtime = r.get("runtime_seconds") or 0

        # Format runtime
        if runtime < 60:
            runtime_str = f"{runtime:.1f}s"
        else:
            mins = int(runtime // 60)
            secs = int(runtime % 60)
            runtime_str = f"{mins}m {secs:02d}s"

        model = r.get("model", "")
        facet = r.get("facet") or ""
        cost_str = _format_cost(cost_usd) if run_file else "-"
        events_str = str(stats["event_count"]) if run_file else "-"
        tools_str = str(stats["tool_count"]) if run_file else "-"
        output_str = _format_bytes(output_size) if output_size is not None else "-"

        facet_part = f"  {facet}" if facet else ""
        line = (
            f"{use_id:<15}{time_str:>12}  {name:<{name_width}}  {status_sym}  "
            f"{runtime_str:>7}  {cost_str:>4}  {events_str:>3}  {tools_str:>3}  "
            f"{output_str:>5}  {model}{facet_part}"
        )

        if use_color and status != "completed":
            line = f"\033[31m{line}\033[0m"

        print(line)


def _event_detail(event: dict[str, Any], etype: str) -> str:
    """Extract detail string for an event."""
    if etype == "request":
        return event.get("prompt", "") or ""
    elif etype == "start":
        model = event.get("model", "")
        prompt = event.get("prompt", "")
        return f'{model} "{prompt}"'
    elif etype == "thinking":
        return event.get("summary") or event.get("content") or ""
    elif etype == "tool_start":
        tool = event.get("tool", "")
        args = event.get("args")
        if isinstance(args, dict):
            parts = [f"{k}={json.dumps(v)}" for k, v in args.items()]
            return f"{tool}({', '.join(parts)})"
        return tool
    elif etype == "tool_end":
        tool = event.get("tool", "")
        result = event.get("result", "")
        return f"{tool} → {result}"
    elif etype == "talent_updated":
        return event.get("talent", "")
    elif etype == "finish":
        result = event.get("result", "")
        usage = event.get("usage")
        if usage:
            inp = usage.get("input_tokens", 0)
            out = usage.get("output_tokens", 0)
            return f"{result} [{inp}in/{out}out]"
        return result
    elif etype == "error":
        return event.get("error", "")
    return ""


def _format_event_line(event: dict[str, Any], *, full: bool = False) -> str:
    """Format a single JSONL event as a one-line summary."""
    ts = event.get("ts", 0)
    dt = datetime.fromtimestamp(ts / 1000)
    time_str = dt.strftime("%H:%M:%S") + f".{ts % 1000:03d}"

    etype = event.get("event", "?")
    label_map = {
        "request": "request",
        "start": "start",
        "thinking": "think",
        "tool_start": "tool",
        "tool_end": "tool_end",
        "talent_updated": "updated",
        "finish": "finish",
        "error": "error",
    }
    label = label_map.get(etype, etype)

    detail = _event_detail(event, etype)

    if full:
        detail = detail.replace("\n", "\\n")
    else:
        detail = detail.replace("\n", " ")
        max_detail = 100 - 24
        if len(detail) > max_detail:
            detail = detail[: max_detail - 1] + "…"

    return f"{time_str}  {label:<8}  {detail}"


def log_run(use_id: str, *, json_mode: bool = False, full: bool = False) -> None:
    """Show events for a single talent run."""
    from solstone.think.utils import get_journal

    talents_dir = Path(get_journal()) / "talents"
    run_file = _find_run_file(talents_dir, use_id)
    if run_file is None:
        print(f"Talent run not found: {use_id}", file=sys.stderr)
        sys.exit(1)

    if json_mode:
        print(run_file.read_text(), end="")
        return

    for line in run_file.read_text().splitlines():
        if not line.strip():
            continue
        try:
            event = json.loads(line)
        except json.JSONDecodeError:
            continue
        print(_format_event_line(event, full=full))


def main() -> None:
    """Entry point for journal talent."""
    parser = argparse.ArgumentParser(description="Inspect talent prompt configurations")
    subparsers = parser.add_subparsers(dest="subcommand")

    # --- list subcommand ---
    list_parser = subparsers.add_parser("list", help="List prompts grouped by schedule")
    list_parser.add_argument(
        "--schedule",
        choices=["daily", "segment", "activity"],
        help="Filter by schedule type",
    )
    list_parser.add_argument(
        "--source", choices=["system", "app"], help="Filter by origin"
    )
    list_parser.add_argument(
        "--disabled", action="store_true", help="Include disabled prompts"
    )
    list_parser.add_argument("--json", action="store_true", help="Output as JSONL")

    # --- show subcommand ---
    show_parser = subparsers.add_parser(
        "show", help="Show details for a specific prompt"
    )
    show_parser.add_argument("name", help="Prompt name")
    show_parser.add_argument("--json", action="store_true", help="Output as JSONL")
    show_parser.add_argument(
        "--prompt", action="store_true", help="Show full prompt context (dry-run mode)"
    )
    show_parser.add_argument("--day", metavar="YYYYMMDD", help="Day for prompt context")
    show_parser.add_argument(
        "--segment", metavar="HHMMSS_LEN", help="Segment for segment-scheduled prompts"
    )
    show_parser.add_argument(
        "--facet", metavar="NAME", help="Facet for multi-facet prompts"
    )
    show_parser.add_argument(
        "--activity", metavar="ID", help="Activity ID for activity-scheduled prompts"
    )
    show_parser.add_argument(
        "--query", metavar="TEXT", help="Sample query for tool agents"
    )
    show_parser.add_argument(
        "--full", action="store_true", help="Show full content without truncation"
    )

    # --- logs subcommand ---
    logs_parser = subparsers.add_parser("logs", help="Show recent talent run log")
    logs_parser.add_argument("agent", nargs="?", help="Filter to a specific agent")
    logs_parser.add_argument(
        "-c",
        "--count",
        type=int,
        default=None,
        help="Number of runs to show (default: 20)",
    )
    logs_parser.add_argument(
        "--day", metavar="YYYYMMDD", help="Show only runs from this day"
    )
    logs_parser.add_argument(
        "--daily", action="store_true", help="Show only daily-scheduled runs"
    )
    logs_parser.add_argument(
        "--errors", action="store_true", help="Show only error runs"
    )
    logs_parser.add_argument(
        "--summary", action="store_true", help="Show grouped summary"
    )

    # --- log subcommand ---
    log_parser = subparsers.add_parser("log", help="Show events for an agent run")
    log_parser.add_argument("id", help="Agent ID")
    log_parser.add_argument(
        "--json", action="store_true", dest="json_mode", help="Output raw JSONL"
    )
    log_parser.add_argument("--full", action="store_true", help="Expand event details")

    args = setup_cli(parser)

    if args.subcommand == "show":
        if args.prompt:
            show_prompt_context(
                args.name,
                day=args.day,
                segment=args.segment,
                facet=args.facet,
                activity=args.activity,
                query=args.query,
                full=args.full,
            )
        else:
            show_prompt(args.name, as_json=args.json)
    elif args.subcommand == "logs":
        logs_runs(
            agent=args.agent,
            count=args.count,
            day=args.day,
            daily=args.daily,
            errors=args.errors,
            summary=args.summary,
        )
    elif args.subcommand == "log":
        log_run(args.id, json_mode=args.json_mode, full=args.full)
    elif args.subcommand == "list" and args.json:
        json_output(
            schedule=args.schedule,
            source=args.source,
            include_disabled=args.disabled,
        )
    elif args.subcommand == "list":
        list_prompts(
            schedule=args.schedule,
            source=args.source,
            include_disabled=args.disabled,
        )
    else:
        # Default: no subcommand given -> list all prompts
        list_prompts()


if __name__ == "__main__":
    main()
