# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Tests for the journal talent CLI."""

import json

import pytest

from solstone.think.talent_cli import (
    _collect_configs,
    _format_bytes,
    _format_cost,
    _format_tags,
    _parse_run_stats,
    _scan_variables,
    json_output,
    list_prompts,
    log_run,
    logs_runs,
    show_prompt,
)


def test_collect_configs_returns_prompts():
    """All configs include known system prompts."""
    configs = _collect_configs(include_disabled=True)
    assert "schedule" in configs
    assert "sense" in configs
    assert "chat" in configs


def test_collect_configs_excludes_disabled_by_default():
    """Disabled prompts are excluded unless include_disabled is set."""
    without = _collect_configs(include_disabled=False)
    with_disabled = _collect_configs(include_disabled=True)
    # include_disabled should return at least as many configs
    assert len(with_disabled) >= len(without)
    assert "schedule" in without
    assert "schedule" in with_disabled


def test_collect_configs_filter_schedule():
    """Schedule filter returns only matching prompts."""
    daily = _collect_configs(schedule="daily", include_disabled=True)
    for key, info in daily.items():
        assert info.get("schedule") == "daily", f"{key} should be daily"

    segment = _collect_configs(schedule="segment", include_disabled=True)
    for key, info in segment.items():
        assert info.get("schedule") == "segment", f"{key} should be segment"

    # No overlap
    assert not set(daily.keys()) & set(segment.keys())

    activity = _collect_configs(schedule="activity", include_disabled=True)
    for key, info in activity.items():
        assert info.get("schedule") == "activity", f"{key} should be activity"

    assert "work" in activity


def test_collect_configs_filter_source():
    """Source filter returns only matching prompts."""
    system = _collect_configs(source="system", include_disabled=True)
    for key, info in system.items():
        assert info.get("source") == "system", f"{key} should be system"

    app = _collect_configs(source="app", include_disabled=True)
    for key, info in app.items():
        assert info.get("source") == "app", f"{key} should be app"


def test_format_tags_hook():
    """Format tags shows compact output, hook, disabled, and FAIL tags."""
    # Output format tags
    assert _format_tags({"output": "md"}) == "md"
    assert _format_tags({"output": "json"}) == "json"
    assert _format_tags({}) == ""

    # Hook tags (compact, no =name suffix)
    assert _format_tags({"hook": {"post": "schedule"}}) == "post"
    assert _format_tags({"hook": {"pre": "prep"}}) == "pre"
    assert _format_tags({"hook": {"pre": "prep", "post": "process"}}) == "pre post"

    # Disabled
    assert _format_tags({"disabled": True}) == "disabled"

    # FAIL tag
    assert _format_tags({}, failed=True) == "FAIL"
    assert _format_tags({"output": "md"}, failed=True) == "md FAIL"

    # Combined: output + hooks + disabled + FAIL
    tags = _format_tags(
        {"output": "md", "hook": {"post": "schedule"}, "disabled": True},
        failed=True,
    )
    assert tags == "md post disabled FAIL"


def test_scan_variables():
    """Variable scanning finds template variables in prompt body."""
    assert "name" in _scan_variables("Hello $name, welcome")
    assert "daily_preamble" in _scan_variables("$daily_preamble\n\n# Title")
    assert _scan_variables("No variables here") == []
    # Deduplicates
    result = _scan_variables("$foo and $bar and $foo again")
    assert result == ["foo", "bar"]


def test_list_prompts_output(capsys):
    """List view outputs expected groups and prompts with column layout."""
    list_prompts()
    output = capsys.readouterr().out

    # Column header
    assert "NAME" in output
    assert "TITLE" in output
    assert "LAST RUN" in output
    assert "TAGS" in output
    assert "OUTPUT" not in output

    # Group headers
    assert "segment:" in output
    assert "daily:" in output
    assert "activity:" in output

    # Prompt names
    assert "activity" in output
    assert "schedule" in output

    # Last run column is present
    assert "LAST RUN" in output


def test_list_prompts_schedule_filter(capsys):
    """Schedule filter shows only matching group."""
    list_prompts(schedule="segment")
    output = capsys.readouterr().out

    assert "sense" in output
    # Should not show daily-only prompts
    # (but don't assert group headers since they're suppressed with filter)


def test_list_prompts_disabled_shown(capsys):
    """--disabled includes disabled prompts (currently none after cleanup)."""
    list_prompts(include_disabled=True)
    output = capsys.readouterr().out

    # all agents should appear in the listing
    assert "schedule" in output


def test_show_prompt_known(capsys):
    """Detail view shows expected fields for a known prompt."""
    show_prompt("schedule")
    output = capsys.readouterr().out

    assert "talent/schedule.md" in output
    assert "title:" in output
    assert "schedule:" in output
    assert "daily" in output
    assert "hook:" in output
    assert "schedule" in output
    assert "variables:" in output
    assert "$daily_preamble" in output
    assert "body:" in output
    assert "lines" in output


def test_show_prompt_not_found(capsys):
    """Detail view exits with error for unknown prompt."""
    with pytest.raises(SystemExit):
        show_prompt("nonexistent_prompt_xyz")

    output = capsys.readouterr().err
    assert "not found" in output.lower()


def test_json_output_format(capsys):
    """JSON output produces valid JSONL with file field."""
    json_output()
    output = capsys.readouterr().out

    lines = [x for x in output.strip().splitlines() if x.strip()]
    assert len(lines) > 0

    for line in lines:
        record = json.loads(line)
        assert "file" in record, f"Missing 'file' key in: {line}"
        assert record["file"].endswith(".md")


def test_json_output_contains_known_prompts(capsys):
    """JSON output includes known prompts with expected fields."""
    json_output(include_disabled=True)
    output = capsys.readouterr().out

    records = [json.loads(x) for x in output.strip().splitlines() if x.strip()]
    files = {r["file"] for r in records}
    assert any("schedule.md" in f for f in files)
    assert any("sense.md" in f for f in files)

    # Check a specific record has expected fields
    schedule = next(r for r in records if "schedule.md" in r["file"])
    assert "title" in schedule
    assert "schedule" in schedule


def test_json_output_schedule_filter(capsys):
    """JSON output respects schedule filter."""
    json_output(schedule="segment")
    output = capsys.readouterr().out

    records = [json.loads(x) for x in output.strip().splitlines() if x.strip()]
    for r in records:
        assert r.get("schedule") == "segment", f"Expected segment: {r}"


def test_show_prompt_as_json(capsys):
    """Detail view with --json outputs single JSONL record."""
    show_prompt("schedule", as_json=True)
    output = capsys.readouterr().out

    lines = [x for x in output.strip().splitlines() if x.strip()]
    assert len(lines) == 1

    record = json.loads(lines[0])
    assert record["file"].endswith("schedule.md")
    assert "title" in record
    assert "schedule" in record
    # Should not contain expanded instruction text
    assert "system_instruction" not in record


def test_truncate_content():
    """Content truncation works correctly."""
    from solstone.think.talent_cli import _truncate_content

    # Short content not truncated
    short = "line1\nline2\nline3"
    result, omitted = _truncate_content(short, max_lines=10)
    assert result == short
    assert omitted == 0

    # Long content truncated
    long = "\n".join(f"line{i}" for i in range(200))
    result, omitted = _truncate_content(long, max_lines=100)
    assert omitted == 100
    assert "lines omitted" in result
    assert "line0" in result  # First lines kept
    assert "line199" in result  # Last lines kept


def test_yesterday():
    """Yesterday helper returns correct format."""
    from solstone.think.talent_cli import _yesterday

    result = _yesterday()
    assert len(result) == 8
    assert result.isdigit()


def test_show_prompt_context_segment_validation(capsys):
    """Segment-scheduled prompts require --segment."""
    from solstone.think.talent_cli import show_prompt_context

    with pytest.raises(SystemExit):
        show_prompt_context("screen", day="20260101")

    output = capsys.readouterr().err
    assert "segment-scheduled" in output.lower()


def test_show_prompt_context_multi_facet_validation(capsys):
    """Multi-facet prompts require --facet."""
    from solstone.think.talent_cli import show_prompt_context

    with pytest.raises(SystemExit):
        show_prompt_context("entities:entities")

    output = capsys.readouterr().err
    assert "multi-facet" in output.lower()


def test_show_prompt_context_day_format_validation(capsys):
    """Day argument must be YYYYMMDD format."""
    from solstone.think.talent_cli import show_prompt_context

    # Too short
    with pytest.raises(SystemExit):
        show_prompt_context("schedule", day="2026")

    output = capsys.readouterr().err
    assert "invalid --day format" in output.lower()

    # Non-numeric
    with pytest.raises(SystemExit):
        show_prompt_context("schedule", day="abcdefgh")

    output = capsys.readouterr().err
    assert "invalid --day format" in output.lower()


def test_logs_runs_default(capsys):
    """Logs shows recent runs from fixture day-index files."""
    logs_runs(count=50)
    output = capsys.readouterr().out

    # Should have runs from all fixture days (original + R&J)
    assert "default" in output or "chat" in output
    assert "flow" in output
    assert "activity" in output
    assert "entities" in output
    assert "meetings" in output
    assert "knowledge_graph" in output
    # Error run should show ✗
    assert "\u2717" in output
    # Completed runs should show ✓
    assert "\u2713" in output


def test_logs_runs_filter_agent(capsys):
    """Logs filters to a specific agent."""
    logs_runs(agent="default")
    output = capsys.readouterr().out

    lines = [line for line in output.strip().splitlines() if line.strip()]
    # fixture has 2 "default" runs in 20231114 + 2 from R&J (20260305, 20260310)
    assert len(lines) == 4
    for line in lines:
        assert "default" in line
    # Should NOT contain other agents
    assert "flow" not in output
    assert "activity" not in output


def test_logs_runs_count_limit(capsys):
    """Logs respects count limit."""
    logs_runs(count=2)
    output = capsys.readouterr().out

    lines = [line for line in output.strip().splitlines() if line.strip()]
    assert len(lines) == 2


def test_logs_runs_no_results(capsys):
    """Logs with unknown agent produces empty output."""
    logs_runs(agent="nonexistent_agent_xyz")
    output = capsys.readouterr().out
    assert output.strip() == ""


def test_logs_runs_new_columns(capsys):
    """Logs output includes enriched columns for runs with JSONL files."""
    logs_runs(count=50)
    output = capsys.readouterr().out
    lines = [line for line in output.strip().splitlines() if line.strip()]

    # Find the line for use_id 1700000000001 (has JSONL file)
    enriched_line = None
    for line in lines:
        if "1700000000001" in line:
            enriched_line = line
            break
    assert enriched_line is not None

    # Should have numeric event/tool counts (not "-")
    # The fixture has 7 events total, 6 non-request, 1 tool_start
    assert "  6  " in enriched_line  # events
    assert "  1  " in enriched_line  # tools

    # Lines without JSONL files should show "-" for enriched columns
    # (most lines lack JSONL files)
    dash_count = sum(1 for line in lines if "  -  " in line)
    assert dash_count > 0


def test_logs_runs_day_filter(capsys):
    """--day filters to a specific day."""
    logs_runs(day="20231114")
    output = capsys.readouterr().out
    lines = [line for line in output.strip().splitlines() if line.strip()]
    # 20231114 has 4 records
    assert len(lines) == 4
    # All should be from 20231114
    for line in lines:
        assert "1700000" in line  # all agent_ids from that day start with 1700000


def test_logs_runs_day_filter_no_match(capsys):
    """--day with nonexistent day produces empty output."""
    logs_runs(day="20990101")
    output = capsys.readouterr().out
    assert output.strip() == ""


def test_logs_runs_day_invalid(capsys):
    """--day with invalid format prints error."""
    with pytest.raises(SystemExit):
        logs_runs(day="bad")
    output = capsys.readouterr().err
    assert "invalid --day format" in output.lower()


def test_logs_runs_errors_filter(capsys):
    """--errors shows only error runs."""
    logs_runs(errors=True)
    output = capsys.readouterr().out
    lines = [line for line in output.strip().splitlines() if line.strip()]
    # Only flow on 20231114 has status "error"
    assert len(lines) == 1
    assert "flow" in lines[0]
    assert "✗" in lines[0]


def test_logs_runs_daily_filter(capsys):
    """--daily shows only daily-scheduled runs."""
    logs_runs(daily=True)
    output = capsys.readouterr().out
    lines = [line for line in output.strip().splitlines() if line.strip()]
    # Daily runs: entities (20231113, schedule=daily), default x2 (20231114,
    # schedule=daily + legacy fallback)
    # Should NOT include flow (segment) or activity
    assert "flow" not in output
    assert "activity" not in output
    for line in lines:
        assert any(
            name in line
            for name in ["default", "entities", "meetings", "knowledge_graph"]
        )


def test_logs_runs_daily_bumps_count(capsys):
    """--daily bumps default count to 50."""
    # With only 6 total records in fixtures, verify explicit count still applies.
    logs_runs(daily=True, count=1)
    output = capsys.readouterr().out
    lines = [line for line in output.strip().splitlines() if line.strip()]
    assert len(lines) == 1


def test_logs_runs_filter_composition(capsys):
    """Filters compose with AND logic."""
    logs_runs(day="20231114", errors=True)
    output = capsys.readouterr().out
    lines = [line for line in output.strip().splitlines() if line.strip()]
    # Only flow on 20231114 is an error
    assert len(lines) == 1
    assert "flow" in lines[0]


def test_logs_runs_summary(capsys):
    """--summary shows grouped aggregation."""
    logs_runs(summary=True, count=50)
    output = capsys.readouterr().out
    # Should have agent names (original + R&J)
    assert "default" in output
    assert "flow" in output
    assert "entities" in output
    assert "activity" in output
    assert "meetings" in output
    assert "knowledge_graph" in output
    # Should have totals line
    assert "total" in output
    # Should show pass/fail symbols
    assert "✓" in output
    assert "✗" in output


def test_logs_runs_daily_summary(capsys):
    """--daily --summary shows only daily runs in summary."""
    logs_runs(daily=True, summary=True)
    output = capsys.readouterr().out
    # Only daily agents (entities, default, meetings, knowledge_graph)
    assert "flow" not in output
    assert "activity" not in output
    assert "default" in output
    assert "entities" in output
    assert "meetings" in output
    assert "knowledge_graph" in output
    assert "total" in output


def test_parse_run_stats():
    """Parse run stats extracts correct counts from fixture JSONL."""
    from pathlib import Path

    jsonl = Path("tests/fixtures/journal/talents/default/1700000000001.jsonl")
    stats = _parse_run_stats(jsonl)
    assert stats["event_count"] == 6  # all except request
    assert stats["tool_count"] == 1  # one tool_start
    assert stats["model"] == "gpt-4o"
    assert stats["usage"] == {"input_tokens": 150, "output_tokens": 80}
    assert stats["request"] is not None
    assert stats["request"]["prompt"] == "Search for meetings about project updates"


def test_parse_run_stats_error():
    """Parse run stats handles error run JSONL correctly."""
    from pathlib import Path

    jsonl = Path("tests/fixtures/journal/talents/flow/1700000000002.jsonl")
    stats = _parse_run_stats(jsonl)
    assert stats["event_count"] == 2  # start + error (not request)
    assert stats["tool_count"] == 0
    assert stats["model"] == "claude-3-haiku"
    assert stats["usage"] is None


def test_format_bytes():
    """Byte formatting produces human-readable strings."""
    assert _format_bytes(0) == "0"
    assert _format_bytes(500) == "500"
    assert _format_bytes(999) == "999"
    assert _format_bytes(1000) == "1.0K"
    assert _format_bytes(1200) == "1.2K"
    assert _format_bytes(34000) == "34.0K"
    assert _format_bytes(1500000) == "1.5M"


def test_format_cost():
    """Cost formatting shows rounded cents."""
    assert _format_cost(None) == "-"
    assert _format_cost(0.0) == "0¢"
    assert _format_cost(0.001) == "<1¢"
    assert _format_cost(0.02) == "2¢"
    assert _format_cost(0.10) == "10¢"
    assert _format_cost(1.50) == "150¢"


def test_log_run_default(capsys):
    """Log run shows one-line-per-event output."""
    log_run("1700000000001")
    output = capsys.readouterr().out
    lines = output.strip().splitlines()

    # Fixture has 7 events
    assert len(lines) == 7

    # Each line should be ≤100 chars
    for line in lines:
        assert len(line) <= 100, f"Line too long ({len(line)}): {line}"

    # Check event type labels appear
    full_output = output
    assert "request" in full_output
    assert "start" in full_output
    assert "think" in full_output
    assert "tool" in full_output
    assert "tool_end" in full_output
    assert "updated" in full_output
    assert "finish" in full_output


def test_log_run_json(capsys):
    """Log run --json outputs raw JSONL."""
    log_run("1700000000001", json_mode=True)
    output = capsys.readouterr().out
    lines = [line for line in output.strip().splitlines() if line.strip()]

    assert len(lines) == 7
    # Each line should be valid JSON
    for line in lines:
        parsed = json.loads(line)
        assert "event" in parsed


def test_log_run_full(capsys):
    """Log run --full shows expanded content with escaped newlines."""
    log_run("1700000000001", full=True)
    output = capsys.readouterr().out

    # The thinking event in the fixture has actual newlines in "content"
    # In --full mode, these should appear as literal \n
    assert "\\n" in output

    # Lines can exceed 100 chars in full mode
    lines = output.strip().splitlines()
    assert len(lines) == 7


def test_log_run_missing():
    """Log run with unknown ID exits with error."""
    with pytest.raises(SystemExit):
        log_run("nonexistent_id_12345")


def test_log_run_error_run(capsys):
    """Log run displays error events correctly."""
    log_run("1700000000002")
    output = capsys.readouterr().out
    lines = output.strip().splitlines()
    assert len(lines) == 3  # request, start, error
    assert "error" in output
    assert "Rate limit" in output


def test_show_prompt_context_activity_requires_facet(capsys):
    """Activity-scheduled prompts require --facet."""
    from solstone.think.talent_cli import show_prompt_context

    with pytest.raises(SystemExit):
        show_prompt_context("work", day="20260214")

    output = capsys.readouterr().err
    assert "activity-scheduled" in output.lower()
    assert "--facet" in output


def test_show_prompt_context_activity_requires_activity_id(capsys):
    """Activity-scheduled prompts require --activity and list available IDs."""
    from solstone.think.talent_cli import show_prompt_context

    with pytest.raises(SystemExit):
        show_prompt_context("work", day="20260214", facet="full-featured")

    output = capsys.readouterr().err
    assert "--activity" in output
    assert "coding_093000_300" in output
    assert "meeting_140000_300" in output


def test_show_prompt_context_activity_not_found(capsys):
    """Activity-scheduled prompt with unknown activity ID errors."""
    from solstone.think.talent_cli import show_prompt_context

    with pytest.raises(SystemExit):
        show_prompt_context(
            "work",
            day="20260214",
            facet="full-featured",
            activity="nonexistent_999",
        )

    output = capsys.readouterr().err
    assert "not found" in output.lower()


def test_list_prompts_activity_group(capsys):
    """List view includes activity group with storytelling talents."""
    list_prompts()
    output = capsys.readouterr().out

    assert "activity:" in output
    assert "conversation" in output
    assert "work" in output
    assert "event" in output
