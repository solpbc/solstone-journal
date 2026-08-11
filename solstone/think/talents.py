# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Unified talent execution module for solstone.

Spawned by cortex for all talent types:
- Tool-using talents (with configured tools)
- Generators (transcript analysis, no tools)

Both paths share unified config preparation and execution flow.
Reads NDJSON config from stdin, emits JSONL events to stdout.
"""

from __future__ import annotations

import argparse
import asyncio
import errno
import json
import logging
import os
import re
import signal
import sys
import threading
import traceback
from datetime import datetime, timezone
from pathlib import Path
from string import Template
from typing import Any, Callable, Literal, Optional

from jsonschema import Draft202012Validator

from solstone.think.cluster import (
    cluster,
    cluster_period,
    cluster_span,
    read_segment_data_state,
)
from solstone.think.data_state import DataState
from solstone.think.pipeline_health import (
    TERMINAL_COMPLETE,
    TerminalUnit,
    read_terminal_states,
)
from solstone.think.providers.cli import QuotaExhaustedError
from solstone.think.providers.shared import Event, classify_provider_error, safe_raw
from solstone.think.responsiveness import (
    NON_RESPONSIVE_RAW_OUTPUT_CAP_CHARS,
    NON_RESPONSIVE_REASON_CODE,
    NonResponsiveOutputError,
)
from solstone.think.talent import (
    get_output_path,
    get_talent_configs,
    get_talent_filter,
    hydrate_runtime_enums,
    load_post_hook,
    load_pre_hook,
    load_prompt,
    source_is_enabled,
    source_is_required,
)
from solstone.think.talent_provenance import (
    UnsupportedProvenancePath,
    compute_identity_hash,
    output_digest,
    read_provenance,
    write_provenance,
)
from solstone.think.utils import (
    day_log,
    day_path,
    format_day,
    format_segment_times,
    get_journal,
    now_ms,
    require_solstone,
    segment_parse,
    setup_cli,
)

TALENT_EXECUTION_MODULE = "solstone.think.talents"

LOG = logging.getLogger("solstone.think.talents")

# Qwen3.5-4B's model card warns against greedy / near-greedy decoding. A single
# retry above this floor perturbs the sampler out of a repetition attractor.
# This is a floor, not a base: talent-owned temperature above it is preserved.
_LOCAL_LENGTH_RETRY_TEMPERATURE_FLOOR = 0.7
_GENERATE_PROGRESS_INTERVAL_S = 5.0
_GENERATE_PROGRESS_JOIN_TIMEOUT_S = 1.0

# Minimum content length for transcript-based generation
MIN_INPUT_CHARS = 50
# Minimum model output tokens before a degradation-checked talent run is flagged near-empty
MIN_OUTPUT_TOKENS = 300
_BRAIN_INGRESS_REASONS: frozenset[str] = frozenset(
    {
        "provider_key_invalid",
        "model_not_found",
        "provider_quota_exceeded",
        "provider_request_rejected",
        "provider_unavailable",
        "network_unreachable",
        "endpoint_unreachable",
        "chat_timeout",
        "provider_response_invalid",
        "cogitate_terminal_error",
    }
)


class TalentHookError(RuntimeError):
    """Raised when an invoked talent hook fails."""

    def __init__(
        self,
        phase: str,
        hook_name: str,
        talent_name: str,
        original: Exception,
    ) -> None:
        self.phase = phase
        self.hook_name = hook_name
        self.talent_name = talent_name
        super().__init__(
            f"{phase}-hook {hook_name!r} failed for talent {talent_name!r}: {original}"
        )


def setup_logging(verbose: bool = False) -> logging.Logger:
    """Configure logging for agent CLI."""
    level = logging.DEBUG if verbose else logging.INFO
    logging.basicConfig(level=level, stream=sys.stdout)
    return LOG


class JSONEventWriter:
    """Write JSONL events to stdout and optionally to a file."""

    def __init__(self, path: Optional[str] = None) -> None:
        self.path = path
        self.file = None
        self._pipe_dead = False
        self._emit_lock = threading.Lock()
        if path:
            try:
                Path(path).parent.mkdir(parents=True, exist_ok=True)
                self.file = open(path, "a", encoding="utf-8")
            except OSError as exc:
                LOG.warning("Failed to open JSON event sidecar %s: %s", path, exc)

    def emit(self, data: Event) -> None:
        with self._emit_lock:
            line = json.dumps(data, ensure_ascii=False)
            if not self._pipe_dead:
                try:
                    print(line)
                    sys.stdout.flush()  # Ensure immediate output for cortex
                except (BrokenPipeError, OSError) as exc:
                    if (
                        not isinstance(exc, BrokenPipeError)
                        and exc.errno != errno.EPIPE
                    ):
                        raise
                    self._pipe_dead = True
            if self.file:
                try:
                    self.file.write(line + "\n")
                    self.file.flush()
                except OSError as exc:
                    LOG.warning(
                        "Failed to write JSON event sidecar %s: %s", self.path, exc
                    )

    def close(self) -> None:
        if self.file:
            try:
                self.file.close()
            except OSError as exc:
                LOG.warning("Failed to close JSON event sidecar %s: %s", self.path, exc)


class _GenerateProgressHeartbeat:
    """Emit generation liveness events while synchronous provider calls block."""

    def __init__(
        self,
        emit_event: Callable[[dict], None],
        *,
        talent_name: str,
        day: object,
        schedule: object,
        interval_s: float | None = None,
        join_timeout_s: float | None = None,
    ) -> None:
        self._emit_event = emit_event
        self._talent_name = talent_name
        self._day = day
        self._schedule = schedule
        self._interval_s = (
            _GENERATE_PROGRESS_INTERVAL_S if interval_s is None else interval_s
        )
        self._join_timeout_s = (
            _GENERATE_PROGRESS_JOIN_TIMEOUT_S
            if join_timeout_s is None
            else join_timeout_s
        )
        self._stop_event = threading.Event()
        self._count_lock = threading.Lock()
        self._count = 0
        self._join_warning_logged = False
        self._thread = threading.Thread(
            target=self._run,
            name=f"generate-progress-{talent_name}",
            daemon=True,
        )

    def start(self) -> None:
        self._thread.start()

    def stop_and_join(self) -> None:
        self._stop_event.set()
        if not self._thread.is_alive():
            return
        self._thread.join(self._join_timeout_s)
        if self._thread.is_alive() and not self._join_warning_logged:
            self._join_warning_logged = True
            # Do not raise here: preserving a successful generation beats
            # protecting the diagnostics-only derived status from one late line.
            LOG.error(
                "generate progress heartbeat did not stop promptly talent=%s day=%s schedule=%s",
                self._talent_name,
                self._day,
                self._schedule,
            )

    @property
    def count(self) -> int:
        with self._count_lock:
            return self._count

    def _run(self) -> None:
        while not self._stop_event.wait(self._interval_s):
            try:
                self._emit_event({"event": "progress", "phase": "generate"})
            except Exception:
                LOG.exception(
                    "generate progress heartbeat failed talent=%s day=%s schedule=%s",
                    self._talent_name,
                    self._day,
                    self._schedule,
                )
                continue
            with self._count_lock:
                self._count += 1


# =============================================================================
# Unified Config Preparation
# =============================================================================


def _stream_content_description(stream: str | None) -> str:
    """Return a human-readable content description for a stream.

    Used in preamble templates so talents know what kind of content they're
    analyzing (live capture vs imported conversations, notes, etc.).
    """
    if not stream:
        return "audio transcription and screen recording"

    STREAM_DESCRIPTIONS = {
        "archon": "audio transcription and screen recording",
        "import.chatgpt": "an imported ChatGPT conversation",
        "import.claude": "an imported Claude conversation",
        "import.gemini": "an imported Gemini conversation",
        "import.ics": "an imported calendar event",
        "import.obsidian": "an imported note from Obsidian",
        "import.document": "an imported document (PDF)",
        "import.kindle": "imported Kindle reading highlights",
    }

    if stream in STREAM_DESCRIPTIONS:
        return STREAM_DESCRIPTIONS[stream]

    # Fallback for unknown import streams
    if stream.startswith("import."):
        source = stream.split(".", 1)[1]
        return f"imported content from {source}"

    if stream.endswith(".browser"):
        return (
            "semantic page text and change updates from browser web apps "
            "such as Gmail or Slack"
        )

    return "captured content"


def _stream_import_guidance(stream: str | None) -> str:
    """Return stream-conditional guidance for the activity agent.

    For live capture, returns guidance about frame comparison and spoken audio.
    For imports, returns content-type-specific analysis instructions.
    Returns empty string for unknown streams.
    """
    if not stream or stream == "archon":
        return (
            "## Live Capture Guidance\n\n"
            "ONLY report what CHANGED between screenshots or was SPOKEN in audio. "
            "If content looks the same across frames, skip it entirely.\n\n"
            "### Your Inputs\n\n"
            "- **Screenshots**: Sampled across this segment. Compare frames — what's different?\n"
            "- **Audio**: Transcript of speech. What was said?\n\n"
            "### SKIP Entirely\n\n"
            "- Windows that look identical in first and last frame\n"
            "- Apps open but showing same content throughout\n"
            "- Background windows never brought to focus\n"
            '- Anything you\'d describe as "had open" or "was visible"'
        )

    IMPORT_GUIDANCE = {
        "import.chatgpt": (
            "This is an AI conversation. Summarize the key topics discussed, "
            "questions asked, solutions proposed, and decisions reached. "
            "Focus on what the human was trying to accomplish and what they learned or decided."
        ),
        "import.claude": (
            "This is an AI conversation. Summarize the key topics discussed, "
            "questions asked, solutions proposed, and decisions reached. "
            "Focus on what the human was trying to accomplish and what they learned or decided."
        ),
        "import.gemini": (
            "This is an AI conversation. Summarize the key topics discussed, "
            "questions asked, solutions proposed, and decisions reached. "
            "Focus on what the human was trying to accomplish and what they learned or decided."
        ),
        "import.ics": (
            "This is a calendar event. Describe the event: its purpose, "
            "participants, and any context from the description about why it was scheduled."
        ),
        "import.obsidian": (
            "This is a note. Summarize the key ideas, references, and connections. "
            "What was the author thinking about and working through?"
        ),
        "import.document": (
            "This is an imported document (legal, financial, medical, or personal). "
            "Extract all named parties and their roles (grantor, trustee, beneficiary, "
            "attorney, witness, agent, etc.). Produce a plain-language summary that a "
            "non-expert could understand. Identify key provisions, dates, conditions, "
            "obligations, and deadlines. Note any time-sensitive requirements (renewal "
            "dates, filing deadlines, review periods)."
        ),
        "import.kindle": (
            "These are reading highlights. Describe what was being read and what "
            "the reader found noteworthy. What themes or ideas do these highlights capture?"
        ),
    }

    if stream in IMPORT_GUIDANCE:
        return f"## Content Guidance\n\n{IMPORT_GUIDANCE[stream]}"

    if stream.startswith("import."):
        return (
            "## Content Guidance\n\n"
            "This is imported content. Summarize the key topics, actions, "
            "and takeaways present in this segment."
        )

    if stream.endswith(".browser"):
        return (
            "## Content Guidance\n\n"
            "This is semantic page text and change updates from web apps the "
            "owner was reading in their browser, such as Gmail or Slack. Read it "
            "as visible page text, not audio and not screen frames. A "
            "segment_start snapshot contains the page's visible text. Delta rows "
            "describe text that was added or updated during the segment; remove "
            "deltas mean text left the page. Summarize what the owner was "
            "reading, doing, and attending to."
        )

    return ""


def _build_prompt_context(
    day: str | None,
    segment: str | None,
    span: list[str] | None,
    activity: dict | None = None,
    facet: str | None = None,
) -> dict[str, str]:
    """Build context dict for prompt template substitution.

    Args:
        day: Day in YYYYMMDD format
        segment: Segment key (HHMMSS_LEN)
        span: List of segment keys
        activity: Optional activity record dict for activity-scheduled talents
        facet: Optional facet name for daily multi-facet talents

    Returns:
        Dict with template variables:
        - day: Friendly format (e.g., "Sunday, February 2, 2025")
        - day_YYYYMMDD: Raw day string (e.g., "20250202")
        - segment_start, segment_end: Time strings if segment/span provided
        - stream, content_description: Stream name and human-readable description
        - activity_*: Activity fields if activity record provided
        - facet, activity_md_dir: Facet name and activity markdown dir for daily runs
    """
    context: dict[str, str] = {}
    if not day:
        return context

    context["day"] = format_day(day)
    context["day_YYYYMMDD"] = day

    # Stream-aware content description and import guidance
    stream = os.environ.get("SOL_STREAM")
    context["stream"] = stream or "archon"
    context["content_description"] = _stream_content_description(stream)
    context["import_guidance"] = _stream_import_guidance(stream)

    if segment:
        start_str, end_str = format_segment_times(segment)
        if start_str and end_str:
            context["segment"] = segment
            context["segment_start"] = start_str
            context["segment_end"] = end_str
    elif span:
        all_times = []
        for seg in span:
            start_time, end_time = segment_parse(seg)
            if start_time and end_time:
                all_times.append((start_time, end_time))

        if all_times:
            earliest_start = min(t[0] for t in all_times)
            latest_end = max(t[1] for t in all_times)
            context["segment_start"] = (
                datetime.combine(datetime.today(), earliest_start)
                .strftime("%I:%M %p")
                .lstrip("0")
            )
            context["segment_end"] = (
                datetime.combine(datetime.today(), latest_end)
                .strftime("%I:%M %p")
                .lstrip("0")
            )

    # Activity template variables
    if activity:
        from solstone.think.activities import estimate_duration_minutes

        context["activity_id"] = activity.get("id", "")
        context["activity_type"] = activity.get("activity", "")
        context["activity_description"] = activity.get("description", "")
        context["activity_level"] = str(activity.get("level_avg", 0.5))
        entities = activity.get("active_entities", [])
        context["activity_entities"] = ", ".join(entities) if entities else ""
        segments = activity.get("segments", [])
        context["activity_segments"] = ", ".join(segments) if segments else ""
        context["activity_duration"] = str(estimate_duration_minutes(segments))

    if facet:
        context["facet"] = facet
        try:
            context["activity_md_dir"] = (
                f"{get_journal()}/facets/{facet}/activities/{day}/"
            )
        except Exception:
            LOG.debug(
                "Failed to build activity_md_dir for facet=%s day=%s",
                facet,
                day,
                exc_info=True,
            )

    return context


def _build_activity_context(
    activity: dict,
    span: list[str],
    facet: str,
    day: str,
) -> str | None:
    """Build activity context sections for $activity_context.

    Args:
        activity: Activity record dict (from activity records JSONL)
        span: List of segment keys in the activity's span
        facet: Facet name
        day: Day in YYYYMMDD format

    Returns:
        Formatted string for the $activity_context template variable.
    """
    activity_cfg = {"context": True, "state": True, "focus": True}

    parts: list[str] = []
    activity_type = activity.get("activity", "unknown")

    # --- activity.context: Activity metadata section ---
    if activity_cfg.get("context"):
        from solstone.think.activities import estimate_duration_minutes

        level_avg = activity.get("level_avg", 0.5)
        level_label = (
            "high" if level_avg >= 0.75 else "medium" if level_avg >= 0.4 else "low"
        )
        segments = activity.get("segments", [])
        duration = estimate_duration_minutes(segments)
        entities = activity.get("active_entities", [])
        entities_str = ", ".join(entities) if entities else "none detected"

        parts.append(
            f"## Activity Context\n"
            f"- **Type:** {activity_type}\n"
            f"- **Description:** {activity.get('description', '')}\n"
            f"- **Engagement Level:** {level_avg} ({level_label})\n"
            f"- **Duration:** ~{duration} minutes ({len(segments)} segments)\n"
            f"- **Active Entities:** {entities_str}"
        )

    # --- activity.state: Per-segment activity descriptions ---
    if activity_cfg.get("state"):
        from solstone.think.activities import load_segment_activity_state

        state_lines: list[str] = []
        for seg in span:
            entry = load_segment_activity_state(day, seg, facet, activity_type)
            if entry:
                level = entry.get("level", "")
                desc = entry.get("description", "")
                # Format segment time for readability
                start_str, end_str = format_segment_times(seg)
                time_label = (
                    f" ({start_str} - {end_str})" if start_str and end_str else ""
                )
                state_lines.append(
                    f"### {seg}{time_label}\n{activity_type} [{level}]: {desc}"
                )

        if state_lines:
            parts.append("## Activity State Per Segment\n\n" + "\n\n".join(state_lines))

    # --- activity.focus: Focusing instructions ---
    if activity_cfg.get("focus"):
        parts.append(
            f"## Analysis Focus\n"
            f"You are analyzing ONLY the **{activity_type}** activity within the "
            f"**{facet}** facet. The transcript segments may contain content from "
            f"other concurrent activities (e.g., background meetings, messaging). "
            f"Use the Activity State Per Segment section above to identify which "
            f"content relates to this activity, and ignore unrelated content. "
            f"Your analysis should only cover what happened within this specific activity."
        )

    if not parts:
        return None

    return "\n\n".join(parts)


def _load_transcript(
    day: str,
    segment: str | None,
    span: list[str] | None,
    sources: dict,
    stream: str | None = None,
) -> tuple[str, dict[str, int]]:
    """Load and cluster transcript for day/segment/span.

    Args:
        day: Day in YYYYMMDD format
        segment: Optional segment key
        span: Optional list of segment keys
        sources: Source config dict from frontmatter load
        stream: Optional stream name; falls back to SOL_STREAM when omitted

    Returns:
        Tuple of (transcript text, source_counts dict)
    """
    if stream is None:
        stream = os.environ.get("SOL_STREAM")

    # Set segment key for token usage logging
    if segment:
        os.environ["SOL_SEGMENT"] = segment
    elif span:
        os.environ["SOL_SEGMENT"] = span[0]

    # Convert sources config for clustering.
    # Frontmatter now uses ``load.talents`` but cluster still consumes the
    # normalized ``agents`` source key internally.
    cluster_sources: dict = {}
    for k, v in sources.items():
        if k == "talents":
            talent_filter = get_talent_filter(v)
            if talent_filter is None:
                cluster_sources["agents"] = source_is_enabled(v)
            elif not talent_filter:
                cluster_sources["agents"] = False
            else:
                cluster_sources["agents"] = talent_filter
        else:
            cluster_sources[k] = source_is_enabled(v)

    if span:
        return cluster_span(day, span, sources=cluster_sources, stream=stream)
    elif segment:
        return cluster_period(day, segment, sources=cluster_sources, stream=stream)
    else:
        return cluster(day, sources=cluster_sources)


def _is_no_input(transcript: str, source_counts: dict[str, int]) -> bool:
    """The framework's emptiness rule: no clustered input, or below MIN_INPUT_CHARS."""
    total_count = sum(source_counts.values())
    return total_count == 0 or len(transcript.strip()) < MIN_INPUT_CHARS


def check_segment_has_no_input(
    day: str,
    segment: str,
    sources: dict,
    stream: str | None = None,
) -> bool:
    """Read-only: True when the segment has no usable sense input.

    Two additive rules, either sufficient:
    - content emptiness: the talent's enabled sources cluster to nothing usable
      (``_is_no_input`` — no clustered input, or below ``MIN_INPUT_CHARS``), or
    - recorded-empty: every present (non-absent) modality derived ``DataState.EMPTY``
      (header-only describe/transcribe outputs that analyzed to nothing).

    Returns False when no source is enabled (nothing to probe), so callers passing a
    source-less config treat the segment as not-gated.
    """
    if not any(source_is_enabled(v) for v in sources.values()):
        return False
    transcript, source_counts = _load_transcript(
        day, segment, None, sources, stream=stream
    )
    if _is_no_input(transcript, source_counts):
        return True
    data_state = read_segment_data_state(day, segment, stream)
    return bool(data_state) and all(
        value == DataState.EMPTY.value for value in data_state.values()
    )


def prepare_config(request: dict) -> dict:
    """Prepare a complete talent config from a request.

    Single unified preparation path for all talent types. Takes raw request
    from cortex and returns fully prepared config ready for execution.

    Config fields produced:
    - name: Talent name
    - provider, model: Resolved from the journal's active Thinking profile
    - user_instruction: Talent instruction from .md file
    - prompt: User's runtime query/request
    - transcript: Clustered transcript (if day provided)
    - output_path: Where to write output (if output format set)
    - skip_reason: Why to skip (if applicable)

    Args:
        request: Raw request dict from cortex

    Returns:
        Fully prepared config dict
    """
    from solstone.think.models import (
        NO_BRAIN_PROVIDER,
        NoBrainConfiguredError,
        resolve_provider,
    )
    from solstone.think.talent import get_talent, key_to_context

    name = request["name"]
    facet = request.get("facet")
    day = request.get("day")
    segment = request.get("segment")
    span = request.get("span")
    activity = request.get("activity")
    output_format = request.get("output")
    output_path_override = request.get("output_path")
    user_prompt = request.get("prompt", "")

    # Load complete talent config
    config = get_talent(name, facet=facet, analysis_day=day)
    if "outbound_approval" in config:
        raise ValueError(
            f"talent {name!r} declares 'outbound_approval' in frontmatter; "
            "this field is launch-config-only and may not come from a talent definition"
        )
    for field in ("provider", "model"):
        if field in config:
            raise ValueError(
                f"talent {name!r} declares {field!r} in frontmatter; "
                "thinking provider and model are configured only in Thinking"
            )
        if request.get(field) is not None:
            raise ValueError(
                f"request overrides for {field!r} are not allowed; "
                "thinking provider and model are configured only in Thinking"
            )

    # Config now contains all frontmatter fields plus:
    # - path: Path to the .md file
    # - sources: Source config for transcript loading
    # - All frontmatter: tools, hook, disabled, thinking_budget, max_output_tokens, etc.

    # Convert path string to Path object for convenience
    talent_path = Path(config["path"]) if config.get("path") else None
    sources = config.get("sources", {})
    talent_cwd = config.get("cwd")
    # Capture the security-relevant fields from the talent definition BEFORE the
    # request merge. access_tier selects tool capability; type steers provider/
    # model resolution and the local-lane runtime promise. A request may not
    # override either (same as cwd). Pin on PRESENCE, not just value:
    # access_tier is populated only for cogitate talents (absent otherwise), so
    # a request that introduces access_tier on a talent that declares none is
    # itself the conflict to reject.
    definition_has_access_tier = "access_tier" in config
    definition_access_tier = config.get("access_tier")
    definition_type = config.get("type")

    # Merge request values (request overrides talent defaults)
    config.update({k: v for k, v in request.items() if v is not None})
    request_cwd = request.get("cwd")
    if request_cwd is not None and request_cwd != talent_cwd:
        raise ValueError(
            f"Request overrides 'cwd' for talent '{name}' are not allowed "
            f"({talent_cwd!r} != {request_cwd!r})"
        )

    request_access_tier = request.get("access_tier")
    if request_access_tier is not None and (
        not definition_has_access_tier or request_access_tier != definition_access_tier
    ):
        raise ValueError(
            f"Request overrides 'access_tier' for talent '{name}' are not allowed "
            f"({definition_access_tier!r} != {request_access_tier!r})"
        )

    request_type = request.get("type")
    if request_type is not None and request_type != definition_type:
        raise ValueError(
            f"Request overrides 'type' for talent '{name}' are not allowed "
            f"({definition_type!r} != {request_type!r})"
        )

    cwd_value = config.get("cwd")
    if cwd_value == "journal":
        try:
            journal_path = Path(get_journal())
        except Exception as exc:
            raise RuntimeError(
                f"Cannot resolve cwd for talent '{name}' — journal path unavailable"
            ) from exc
        if not journal_path.exists():
            raise RuntimeError(
                f"Cannot resolve cwd for talent '{name}' — journal path unavailable"
            )
        config["cwd"] = str(journal_path)

    # Populate stream from env if not already in config (think passes it as
    # SOL_STREAM env var but not as a top-level request key — hooks need it)
    if "stream" not in config:
        sol_stream = os.environ.get("SOL_STREAM")
        if sol_stream:
            config["stream"] = sol_stream

    # Track additional state
    config["span_mode"] = bool(span)
    config["source_counts"] = {}

    # Resolve provider and model for the talent's interface
    context = key_to_context(name)
    talent_type = config["type"]
    provider, model = resolve_provider(talent_type)

    config["provider"] = provider
    config["model"] = model
    config["context"] = context
    if provider == NO_BRAIN_PROVIDER:
        raise NoBrainConfiguredError()

    # Check if disabled
    if config.get("disabled"):
        config["skip_reason"] = "disabled"
        return config

    # Day-based processing: load transcript and apply template substitution
    if day:
        # Load transcript (only when the talent has enabled sources to consume)
        if any(source_is_enabled(v) for v in sources.values()):
            transcript, source_counts = _load_transcript(day, segment, span, sources)
            config["transcript"] = transcript
            config["source_counts"] = source_counts
            total_count = sum(source_counts.values())

            # Check required sources
            for source_type, mode in sources.items():
                if source_is_required(mode) and source_counts.get(source_type, 0) == 0:
                    config["skip_reason"] = f"missing_required_{source_type}"
                    return config

            # Skip if no content
            if _is_no_input(transcript, source_counts):
                config["skip_reason"] = "no_input"
                return config

            # Note for limited recordings
            if total_count < 3:
                config["transcript"] = (
                    "**Input Note:** Limited recordings for this day. "
                    "Scale analysis to available input.\n\n" + transcript
                )

        # Reload talent instruction with template substitution for day/segment context
        if talent_path and talent_path.exists():
            from solstone.think.prompts import _resolve_facets

            prompt_context = _build_prompt_context(
                day, segment, span, activity=activity, facet=facet
            )
            prompt_context["facets"] = _resolve_facets(facet)

            if activity and span and facet:
                activity_ctx = _build_activity_context(activity, span, facet, day)
                if activity_ctx:
                    prompt_context["activity_context"] = activity_ctx

            talent_prompt_obj = load_prompt(
                talent_path.stem, base_dir=talent_path.parent, context=prompt_context
            )
            config["user_instruction"] = talent_prompt_obj.text

    # Set prompt (user's runtime query)
    # For tool talents: prompt is the user's question
    # For generators: prompt is typically empty (instruction is in user_instruction)
    config["prompt"] = user_prompt

    # Determine output path
    if output_format:
        if output_path_override:
            config["output_path"] = Path(output_path_override)
        elif day:
            stream = os.environ.get("SOL_STREAM")
            day_dir = str(day_path(day))
            config["output_path"] = get_output_path(
                day_dir,
                name,
                segment=segment,
                output_format=output_format,
                facet=facet,
                stream=stream,
            )

    return config


def validate_config(config: dict) -> str | None:
    """Validate prepared config.

    Args:
        config: Prepared config dict

    Returns:
        Error message string if invalid, None if valid
    """
    is_cogitate = config["type"] == "cogitate"
    has_prompt = bool(config.get("prompt"))
    has_user_instruction = bool(config.get("user_instruction"))
    has_day = bool(config.get("day"))

    if is_cogitate and not (has_prompt or has_user_instruction):
        return "Cogitate talent requires non-empty 'prompt' or 'user_instruction'"

    # Generate prompts need either day (transcript) or user_instruction
    if not is_cogitate and not has_day and not has_user_instruction and not has_prompt:
        return "Invalid config: must have 'type', 'day', or 'prompt'"

    # Segment/span requires day
    if (config.get("segment") or config.get("span")) and not has_day:
        return "Invalid config: 'segment' or 'span' requires 'day'"

    return None


# =============================================================================
# Hook Execution
# =============================================================================


def _run_pre_hooks(config: dict) -> dict:
    """Run pre-processing hooks, return dict of modifications.

    Args:
        config: Full config dict (hooks receive this directly)

    Returns:
        Dict of field modifications to apply to config
    """
    pre_hook = load_pre_hook(config)
    if not pre_hook:
        return {}

    hook_name = str(config.get("hook", {}).get("pre", "unknown"))
    talent_name = str(config.get("name", "unknown"))
    try:
        modifications = pre_hook(config)
        if modifications:
            LOG.info("Pre-hook returned modifications: %s", list(modifications.keys()))
            return modifications
    except Exception as exc:
        LOG.error("Pre-hook failed: %s", exc)
        raise TalentHookError("pre", hook_name, talent_name, exc) from exc

    return {}


def _apply_template_vars(config: dict, template_vars: dict) -> None:
    """Substitute template_vars into text fields of config in-place.

    Expands each key with auto-capitalize convention (matching load_prompt):
      {"foo": "bar"} -> $foo="bar", $Foo="Bar"
    """
    expanded = {}
    for key, value in template_vars.items():
        str_value = str(value)
        expanded[key] = str_value
        expanded[key.capitalize()] = str_value.capitalize()

    for field in ("user_instruction", "transcript", "prompt"):
        text = config.get(field)
        if text:
            config[field] = Template(text).safe_substitute(expanded)


def _run_post_hooks(result: str, config: dict) -> str:
    """Run post-processing hooks, return transformed result.

    Args:
        result: LLM output text
        config: Full config dict (hooks receive this directly)

    Returns:
        Transformed result (or original if no hook)
    """
    post_hook = load_post_hook(config)
    if not post_hook:
        return result

    hook_name = str(config.get("hook", {}).get("post", "unknown"))
    talent_name = str(config.get("name", "unknown"))
    try:
        hook_result = post_hook(result, config)
        if hook_result is not None:
            LOG.info("Post-hook transformed result")
            return hook_result
    except Exception as exc:
        LOG.error("Post-hook failed: %s", exc)
        raise TalentHookError("post", hook_name, talent_name, exc) from exc

    return result


def _expected_output_blank(config: dict, raw_result: str, result: str) -> bool:
    if not config.get("output_path"):
        return False
    if result and result.strip():
        return False
    if raw_result and raw_result.strip():
        return False
    return True


_NO_OUTPUT_ERROR = (
    "no_output: expects-output talent finished without producing a result"
)


# =============================================================================
# Unified Talent Execution
# =============================================================================


def _write_output(output_path: Path, result: str) -> bool:
    """Write result to output file and return whether bytes changed."""
    payload = result.encode("utf-8")
    if output_path.exists() and output_path.read_bytes() == payload:
        LOG.info("Output unchanged at %s", output_path)
        return False
    output_path.parent.mkdir(parents=True, exist_ok=True)
    with open(output_path, "wb") as f:
        f.write(payload)
    LOG.info("Wrote output to %s", output_path)
    return True


_MD_FENCE_LINE = re.compile(r"^```[A-Za-z0-9_-]*$")


def _strip_outer_markdown_fence(text: str) -> tuple[str, bool]:
    """Strip a single whole-output code fence wrapping markdown text.

    Returns (text, stripped). Only strips when the output is wrapped as a
    whole in exactly one outer fence: the opener is the first non-whitespace
    line (```  or ```lang) and the closer is the last non-whitespace line and
    is a bare ```. Requires the opener and closer to be the ONLY fence-delimiter
    lines in the output, so an interior code block never triggers a strip.
    On any non-match returns the original text unchanged with stripped=False.
    """
    lines = text.split("\n")
    fence_indices = [
        index for index, line in enumerate(lines) if _MD_FENCE_LINE.match(line.strip())
    ]
    if len(fence_indices) != 2:
        return text, False

    opener_idx, closer_idx = fence_indices
    if any(line.strip() for line in lines[:opener_idx]):
        return text, False
    if any(line.strip() for line in lines[closer_idx + 1 :]):
        return text, False
    if lines[closer_idx].strip() != "```":
        return text, False

    interior = "\n".join(lines[opener_idx + 1 : closer_idx])
    return interior, True


def _build_generation_contents(config: dict) -> list[Any]:
    messages = config.get("messages")
    if messages and isinstance(messages, list):
        return messages

    contents: list[Any] = []
    transcript = config.get("transcript", "")
    user_instruction = config.get("user_instruction", "")
    prompt = config.get("prompt", "")
    if transcript:
        contents.append(transcript)
    if user_instruction:
        contents.append(user_instruction)
    if prompt:
        contents.append(prompt)
    return contents or ["No input provided."]


def _generation_params(config: dict) -> dict[str, Any]:
    return {
        "temperature": config.get("temperature", 0.3),
        "max_output_tokens": config.get("max_output_tokens") or 8192 * 6,
        # Default only when unset — an explicit thinking_budget=0 must pass through
        # to disable thinking (see providers/google.py: budget=0 disables, omitting
        # the config lets Gemini apply its own default). `or` would coalesce 0 -> default.
        "thinking_budget": (
            8192 * 2
            if config.get("thinking_budget") is None
            else config["thinking_budget"]
        ),
    }


def _normalized_sources(config: dict) -> dict[str, Any]:
    normalized: dict[str, Any] = {}
    for key, value in (config.get("sources") or {}).items():
        if key == "talents":
            normalized[key] = get_talent_filter(value)
        else:
            normalized[key] = value
    return normalized


def _runtime_identity(config: dict, runtime_json_schema: Any) -> dict[str, Any]:
    activity = config.get("activity")
    activity_id = activity.get("id") if isinstance(activity, dict) else activity
    activity_type = activity.get("activity") if isinstance(activity, dict) else None
    stream = config.get("stream") or os.environ.get("SOL_STREAM")
    span = config.get("span")
    ordered_span = [str(item) for item in span] if isinstance(span, list) else []
    if not ordered_span and config.get("segment"):
        ordered_span = [str(config["segment"])]

    return {
        "contents": _build_generation_contents(config),
        "system_instruction": config.get("system_instruction") or None,
        "json_schema": runtime_json_schema,
        "talent": {
            "name": config.get("name"),
            "type": config.get("type"),
            "schedule": config.get("schedule"),
            "output": config.get("output"),
            "activities": config.get("activities"),
            "hook": config.get("hook"),
            "path": str(config.get("path") or ""),
            "priority": config.get("priority"),
            "degradation_check": config.get("degradation_check"),
        },
        "sources": _normalized_sources(config),
        "provider": config.get("provider"),
        "model": config.get("model"),
        "generation_params": _generation_params(config),
        "runtime": {
            "day": config.get("day"),
            "schedule": config.get("schedule"),
            "stream": stream,
            "segment": config.get("segment"),
            "span": ordered_span,
            "facet": config.get("facet"),
            "activity": str(activity_id) if activity_id is not None else None,
            "activity_type": activity_type,
        },
    }


def _identity_fields(config: dict) -> dict[str, Any]:
    activity = config.get("activity")
    activity_id = activity.get("id") if isinstance(activity, dict) else activity
    return {
        "name": config.get("name"),
        "type": config.get("type"),
        "day": config.get("day"),
        "schedule": config.get("schedule"),
        "stream": config.get("stream") or os.environ.get("SOL_STREAM"),
        "segment": config.get("segment"),
        "facet": config.get("facet"),
        "activity": str(activity_id) if activity_id is not None else None,
    }


def _identity_hash(config: dict, runtime_json_schema: Any) -> str:
    return compute_identity_hash(_runtime_identity(config, runtime_json_schema))


def _output_valid_for_schema(
    result: str,
    output_format: str | None,
    runtime_json_schema: Any,
) -> bool:
    if output_format != "json":
        return True
    try:
        parsed = json.loads(result)
        if runtime_json_schema is not None:
            Draft202012Validator(runtime_json_schema).validate(parsed)
    except Exception:
        LOG.warning("JSON output failed current schema validation", exc_info=True)
        return False
    return True


def _schema_validation_clean(gen_result: dict, result: str, config: dict) -> bool:
    validation = gen_result.get("schema_validation")
    if isinstance(validation, dict) and validation.get("valid") is False:
        return False
    runtime_json_schema = hydrate_runtime_enums(config.get("json_schema"))
    return _output_valid_for_schema(result, config.get("output"), runtime_json_schema)


def _schema_invalid_message(gen_result: dict) -> str:
    validation = gen_result.get("schema_validation")
    if isinstance(validation, dict) and validation.get("valid") is False:
        errors = validation.get("errors")
        if errors:
            first = errors[0]
            text = (
                f"{first['path'] or '<root>'}: "
                f"{first['constraint']}: {first['message']}"
            )
            return text if len(text) <= 200 else text[:197] + "..."
    return "talent output failed JSON schema validation"


def _terminal_unit(config: dict) -> TerminalUnit | None:
    day = config.get("day")
    mode = config.get("schedule")
    name = config.get("name")
    if not day or not isinstance(mode, str) or not isinstance(name, str):
        return None
    activity = config.get("activity")
    activity_id = activity.get("id") if isinstance(activity, dict) else activity
    return TerminalUnit(
        mode=mode,
        name=name,
        facet=config.get("facet") if isinstance(config.get("facet"), str) else None,
        stream=(
            config.get("stream")
            if isinstance(config.get("stream"), str)
            else os.environ.get("SOL_STREAM")
        ),
        segment=(
            config.get("segment") if isinstance(config.get("segment"), str) else None
        ),
        activity=str(activity_id) if activity_id is not None else None,
    )


def _latest_terminal_complete(config: dict) -> bool:
    unit = _terminal_unit(config)
    day = config.get("day")
    if unit is None or not isinstance(day, str):
        return False
    try:
        state = read_terminal_states(day).get(unit)
    except Exception:
        LOG.warning("failed to read terminal state for cache reuse", exc_info=True)
        return False
    return state is not None and state.latest_event == TERMINAL_COMPLETE


def _try_reuse_output(config: dict, emit_event: Callable[[dict], None]) -> bool:
    output_path = Path(config["output_path"]) if config.get("output_path") else None
    if output_path is None:
        return False
    try:
        runtime_json_schema = hydrate_runtime_enums(config.get("json_schema"))
        today_hash = _identity_hash(config, runtime_json_schema)
        provenance = read_provenance(output_path)
    except Exception:
        LOG.warning("failed to evaluate talent provenance", exc_info=True)
        return False

    if not provenance or provenance.get("identity_hash") != today_hash:
        return False
    if not output_path.exists() or output_path.stat().st_size == 0:
        return False

    try:
        output_sha256, output_size = output_digest(output_path)
        if (
            provenance.get("output_sha256") != output_sha256
            or provenance.get("output_size") != output_size
        ):
            return False
        result = output_path.read_text(encoding="utf-8")
        if not _output_valid_for_schema(
            result,
            config.get("output"),
            runtime_json_schema,
        ):
            return False
    except Exception:
        LOG.warning("failed to validate cached talent output", exc_info=True)
        return False

    if not _latest_terminal_complete(config):
        return False

    completed_at_ms = provenance.get("completed_at_ms")
    if not isinstance(completed_at_ms, int):
        return False

    LOG.info("Reusing cached talent output: %s", output_path)
    emit_event(
        {
            "event": "finish",
            "ts": now_ms(),
            "result": result,
            "cache_hit": True,
            "output_changed": False,
            "completed_at_ms": completed_at_ms,
        }
    )
    return True


def _write_clean_provenance(
    config: dict,
    output_path: Path | None,
    result: str,
    runtime_json_schema: Any,
    completed_at_ms: int,
) -> None:
    if not output_path or not result:
        return
    # Provenance is an observability sidecar, never the run's success
    # contract: a sidecar failure must not flip a saved output to "error".
    # Mirrors the non-fatal read path in _try_reuse_output. The unsupported-
    # path sentinel is the benign "this output shape has no day-rooted
    # provenance home" signal (logged at WARNING); any other failure is
    # unexpected and logged LOUDLY at ERROR (not a silent swallow).
    try:
        output_sha256, output_size = output_digest(output_path)
        write_provenance(
            output_path,
            identity_hash=_identity_hash(config, runtime_json_schema),
            output_sha256=output_sha256,
            output_size=output_size,
            provider=config.get("provider"),
            model=config.get("model"),
            generation_params=_generation_params(config),
            completed_at_ms=completed_at_ms,
            use_id=config.get("use_id"),
            identity_fields=_identity_fields(config),
        )
    except UnsupportedProvenancePath:
        LOG.warning(
            "skipping talent provenance for unmapped output path %s",
            output_path,
        )
    except Exception:
        LOG.error(
            "failed to write talent provenance for %s",
            output_path,
            exc_info=True,
        )


def _build_dry_run_event(config: dict, before_values: dict) -> dict:
    """Build a dry-run event with all context."""
    talent_type = config["type"]

    event: dict[str, Any] = {
        "event": "dry_run",
        "ts": now_ms(),
        "type": talent_type,
        "name": config["name"],
        "provider": config.get("provider", ""),
        "model": config.get("model") or "unknown",
        "system_instruction": config.get("system_instruction", ""),
        "user_instruction": config.get("user_instruction", ""),
        "prompt": config.get("prompt", ""),
    }

    extra_context = config.get("extra_context", "")
    if extra_context:
        event["extra_context"] = extra_context

    # Day-based fields
    if config.get("day"):
        event["day"] = config["day"]
        event["segment"] = config.get("segment")
        transcript = config.get("transcript", "")
        if transcript:
            event["transcript"] = transcript
            event["transcript_chars"] = len(transcript)
            event["transcript_files"] = sum(config.get("source_counts", {}).values())
        output_path = Path(config["output_path"]) if config.get("output_path") else None
        if output_path:
            event["output_path"] = str(output_path)

    # Show before values for comparison
    for key, before_val in before_values.items():
        current_val = config.get(key, "")
        if current_val != before_val:
            if key == "transcript":
                event["transcript_before_chars"] = len(before_val)
            else:
                event[f"{key}_before"] = before_val

    return event


def _mark_terminal_error_evented(config: dict) -> None:
    config["_terminal_error_evented"] = True


def _emit_terminal_hook_error(
    config: dict,
    emit_event: Callable[[dict], None],
    exc: TalentHookError,
    *,
    generate_progress_count: int | None = None,
) -> None:
    _mark_terminal_error_evented(config)
    setattr(exc, "_evented", True)
    event: dict[str, Any] = {
        "event": "error",
        "error": str(exc),
        "reason_code": "hook_error",
        "provider": config.get("provider"),
        "terminal": True,
        "ts": now_ms(),
    }
    if generate_progress_count is not None:
        event["generate_progress_count"] = generate_progress_count
    emit_event(event)


from solstone.think.models import (
    NO_BRAIN_PROVIDER,
    _raise_if_confidential_unverified,
)


def _classify_degraded(usage: dict | None, config: dict) -> dict | None:
    """Flag an opted-in talent run whose model produced near-zero output.

    Opt-in via the talent's `degradation_check` frontmatter flag. Returns a
    marker dict for the finish event, or None when not degraded / not checked /
    output-token count unknown (never alarm without a numeric count).
    """
    if not config.get("degradation_check"):
        return None
    if not usage:
        return None
    tokens = usage.get("output_tokens")
    if isinstance(tokens, bool) or not isinstance(tokens, (int, float)):
        return None
    if tokens < MIN_OUTPUT_TOKENS:
        return {"reason": "near_empty", "output_tokens": int(tokens)}
    return None


def _read_runtime_fingerprint() -> str | None:
    from solstone.think.providers.brain_state import (
        read_active_brain_fingerprint_sha256,
    )

    try:
        return read_active_brain_fingerprint_sha256()
    except Exception as exc:
        LOG.warning("Unable to read active brain fingerprint: %s", exc)
        return None


def _record_brain_runtime_failure(
    reason_code: str,
    component: Literal["generate", "cogitate"],
    *,
    expected_fingerprint_sha256: str | None = None,
) -> None:
    if reason_code not in _BRAIN_INGRESS_REASONS:
        return
    from solstone.think.providers.brain_state import (
        BRAIN_EVIDENCE_REASON_CODES,
        record_brain_runtime_failure,
    )

    if reason_code not in BRAIN_EVIDENCE_REASON_CODES[component]:
        return
    expected_fingerprint_sha256 = (
        expected_fingerprint_sha256 or _read_runtime_fingerprint()
    )
    if not expected_fingerprint_sha256:
        LOG.warning(
            "Unable to record %s runtime evidence: active brain fingerprint unavailable",
            reason_code,
        )
        return

    result = record_brain_runtime_failure(
        reason_code,
        datetime.now(timezone.utc),
        expected_fingerprint_sha256=expected_fingerprint_sha256,
        component=component,
        diagnostic={},
    )
    if not result.get("accepted"):
        LOG.warning(
            "Unable to record %s runtime evidence: %s%s",
            reason_code,
            result.get("rejected_reason"),
            f" ({result.get('error')})" if result.get("error") else "",
        )


def _clean_stop_blank_response(exc: Exception) -> bool:
    reason = getattr(exc, "reason", None)
    finish_reason = getattr(exc, "finish_reason", None)
    return reason == "blank_visible_output" and finish_reason in {None, "stop"}


def _capture_runtime_fingerprint(config: dict) -> str | None:
    expected_fingerprint_sha256 = _read_runtime_fingerprint()
    if expected_fingerprint_sha256:
        config["_brain_runtime_fingerprint_sha256"] = expected_fingerprint_sha256
    return expected_fingerprint_sha256


def _non_responsive_raw(exc: Exception) -> list[dict[str, Any]]:
    payload: dict[str, Any] = {"reason_code": NON_RESPONSIVE_REASON_CODE}
    output = getattr(exc, "non_responsive_output", None)
    if isinstance(output, str):
        payload["non_responsive_output"] = output[:NON_RESPONSIVE_RAW_OUTPUT_CAP_CHARS]
    matched_signal = getattr(exc, "non_responsive_matched_signal", None)
    if matched_signal is not None:
        payload["non_responsive_matched_signal"] = matched_signal
    return safe_raw([payload])


def _emit_terminal_generate_error(
    config: dict,
    emit_event: Callable[[dict], None],
    exc: Exception,
    *,
    reason_code: str,
    raw: list[dict[str, Any]] | None = None,
    retries: int = 0,
    generate_progress_count: int | None = None,
) -> None:
    _mark_terminal_error_evented(config)
    event: dict[str, Any] = {
        "event": "error",
        "error": str(exc),
        "reason_code": reason_code,
        "provider": config.get("provider"),
        "terminal": True,
        "ts": now_ms(),
    }
    if raw is not None:
        event["raw"] = raw
    if retries:
        event["retries"] = retries
    if generate_progress_count is not None:
        event["generate_progress_count"] = generate_progress_count
    emit_event(event)


async def _execute_with_tools(
    config: dict,
    emit_event: Callable[[dict], None],
) -> None:
    """Execute a tool-using talent via the provider's run_cogitate.

    Args:
        config: Prepared config dict
        emit_event: Event emission callback
    """
    from . import cogitate_client

    provider = config.get("provider", "google")
    output_path = Path(config["output_path"]) if config.get("output_path") else None

    _raise_if_confidential_unverified(provider)
    expected_fingerprint_sha256 = _capture_runtime_fingerprint(config)

    # Wrapper to intercept finish event for post-processing
    def talent_emit_event(data: Event) -> None:
        if (
            data.get("event") == "error"
            and data.get("terminal") is True
            and data.get("reason_code") in {"agent_stuck", "no_output"}
        ):
            _record_brain_runtime_failure(
                "cogitate_terminal_error",
                "cogitate",
                expected_fingerprint_sha256=expected_fingerprint_sha256,
            )

        if data.get("event") == "finish":
            raw_result = data.get("result", "")
            result = _run_post_hooks(raw_result, config)
            if _expected_output_blank(config, raw_result, result):
                _record_brain_runtime_failure(
                    "cogitate_terminal_error",
                    "cogitate",
                    expected_fingerprint_sha256=expected_fingerprint_sha256,
                )
                _mark_terminal_error_evented(config)
                emit_event(
                    {
                        "event": "error",
                        "error": _NO_OUTPUT_ERROR,
                        "reason_code": "no_output",
                        "provider": config.get("provider"),
                        "terminal": True,
                        "ts": now_ms(),
                    }
                )
                return

            updates: dict[str, Any] = {}
            if result != raw_result:
                updates["result"] = result

            degraded = _classify_degraded(data.get("usage"), config)
            if degraded:
                updates["degraded"] = degraded

            output_changed = True if output_path is None else False
            if output_path and result:
                output_changed = _write_output(output_path, result)

            completed_at_ms = now_ms()
            runtime_json_schema = hydrate_runtime_enums(config.get("json_schema"))
            # No cogitate talent declares JSON output or a schema today; if one
            # does, mirror the generate-path terminal schema gate.
            schema_clean = _output_valid_for_schema(
                result,
                config.get("output"),
                runtime_json_schema,
            )
            if output_path and result and not degraded and schema_clean:
                _write_clean_provenance(
                    config,
                    output_path,
                    result,
                    runtime_json_schema,
                    completed_at_ms,
                )

            updates["cache_hit"] = False
            updates["output_changed"] = output_changed
            updates["completed_at_ms"] = completed_at_ms

            data = {**data, **updates}

        # Filter out start events from providers (we already emitted ours)
        if data.get("event") == "start":
            return

        emit_event(data)

    try:
        if provider == "local":
            # AC4: local keeps its admission lease around the native-client call.
            from .providers import local

            await local.run_cogitate(config=config, on_event=talent_emit_event)
        else:
            # AC1: cogitate dispatch is the native thin client, not a provider loop.
            await cogitate_client.run_cogitate(config=config, on_event=talent_emit_event)
    except TalentHookError as exc:
        _emit_terminal_hook_error(config, emit_event, exc)
        return
    except Exception as exc:
        if isinstance(exc, QuotaExhaustedError):
            _record_brain_runtime_failure(
                "provider_quota_exceeded",
                "cogitate",
                expected_fingerprint_sha256=expected_fingerprint_sha256,
            )
            exc._brain_runtime_recorded = True
            reset_at_ms = now_ms() + (exc.retry_delay_ms or 0)
            emit_event(
                {
                    "event": "error",
                    "reason": "quota_exhausted",
                    # reason_code is the chat-side carrier; reason is the quota label.
                    "reason_code": "provider_quota_exceeded",
                    "provider": provider,
                    "error": str(exc),
                    "reset_at_ms": reset_at_ms,
                    "terminal": False,
                }
            )
        raise


async def _execute_generate(
    config: dict,
    emit_event: Callable[[dict], None],
) -> None:
    """Execute single-shot generation (no tools).

    Args:
        config: Prepared config dict
        emit_event: Event emission callback
    """
    from solstone.think.models import (
        IncompleteJSONError,
        ProviderResponseInvalidError,
        generate_with_result,
    )
    from solstone.think.talent import key_to_context

    name = config["name"]
    system_instruction = config.get("system_instruction") or None
    output_path = Path(config["output_path"]) if config.get("output_path") else None
    output_format = config.get("output")

    # Get generation parameters from config (set in frontmatter)
    generation_params = _generation_params(config)
    thinking_budget = generation_params["thinking_budget"]
    max_output_tokens = generation_params["max_output_tokens"]
    temperature = generation_params["temperature"]
    is_json_output = output_format == "json"

    # Derive LLM request timeout from token budget: scale with output size,
    # floor at 120s, cap at 480s (well under cortex's 600s subprocess kill).
    timeout_s = config.get("timeout_s") or min(
        480, max(120, (max_output_tokens + thinking_budget) // 100)
    )

    contents = _build_generation_contents(config)
    context = key_to_context(name)
    runtime_json_schema = hydrate_runtime_enums(config.get("json_schema"))
    retries = 0
    expected_fingerprint_sha256 = _capture_runtime_fingerprint(config)
    heartbeat = _GenerateProgressHeartbeat(
        emit_event,
        talent_name=str(name),
        day=config.get("day"),
        schedule=config.get("schedule"),
    )
    heartbeat.start()

    def _stop_generate_progress() -> int:
        heartbeat.stop_and_join()
        return heartbeat.count

    try:
        try:
            gen_result = generate_with_result(
                contents=contents,
                context=context,
                temperature=temperature,
                max_output_tokens=max_output_tokens,
                thinking_budget=thinking_budget,
                system_instruction=system_instruction,
                json_output=is_json_output,
                json_schema=runtime_json_schema,
                timeout_s=timeout_s,
            )
        except ProviderResponseInvalidError as exc:
            if _clean_stop_blank_response(exc):
                _record_brain_runtime_failure(
                    "provider_response_invalid",
                    "generate",
                    expected_fingerprint_sha256=expected_fingerprint_sha256,
                )
            generate_progress_count = _stop_generate_progress()
            _emit_terminal_generate_error(
                config,
                emit_event,
                exc,
                reason_code="provider_response_invalid",
                generate_progress_count=generate_progress_count,
            )
            return
        except NonResponsiveOutputError as exc:
            generate_progress_count = _stop_generate_progress()
            _emit_terminal_generate_error(
                config,
                emit_event,
                exc,
                reason_code=NON_RESPONSIVE_REASON_CODE,
                raw=_non_responsive_raw(exc),
                generate_progress_count=generate_progress_count,
            )
            return
        except Exception as exc:
            provider = config.get("provider", "google")
            if isinstance(exc, QuotaExhaustedError):
                _record_brain_runtime_failure(
                    "provider_quota_exceeded",
                    "generate",
                    expected_fingerprint_sha256=expected_fingerprint_sha256,
                )
                exc._brain_runtime_recorded = True
            if provider == NO_BRAIN_PROVIDER:
                raise
            if provider == "local":
                reason_code = getattr(exc, "reason_code", None)
                length_retry = (
                    isinstance(exc, IncompleteJSONError)
                    and reason_code == "incomplete_json_length"
                )
                capacity_retry = reason_code == "local_capacity_exhausted"
                if not length_retry and not capacity_retry:
                    raise
                retries = 1
                retry_temperature = temperature
                retry_kwargs: dict[str, Any] = {"inference_retry_index": 1}
                if length_retry:
                    LOG.warning(
                        "Retrying local talent %s after incomplete JSON length limit "
                        "(reason=%s)",
                        name,
                        exc.reason,
                    )
                    retry_temperature = max(
                        temperature, _LOCAL_LENGTH_RETRY_TEMPERATURE_FLOOR
                    )
                else:
                    LOG.warning(
                        "Retrying local talent %s after local capacity exhaustion",
                        name,
                    )
                    retry_kwargs["local_exclusive_admission"] = True
                try:
                    gen_result = generate_with_result(
                        contents=contents,
                        context=context,
                        temperature=retry_temperature,
                        max_output_tokens=max_output_tokens,
                        thinking_budget=thinking_budget,
                        system_instruction=system_instruction,
                        json_output=is_json_output,
                        json_schema=runtime_json_schema,
                        timeout_s=timeout_s,
                        **retry_kwargs,
                    )
                except ProviderResponseInvalidError as retry_exc:
                    if _clean_stop_blank_response(retry_exc):
                        _record_brain_runtime_failure(
                            "provider_response_invalid",
                            "generate",
                            expected_fingerprint_sha256=expected_fingerprint_sha256,
                        )
                    generate_progress_count = _stop_generate_progress()
                    _emit_terminal_generate_error(
                        config,
                        emit_event,
                        retry_exc,
                        reason_code="provider_response_invalid",
                        retries=retries,
                        generate_progress_count=generate_progress_count,
                    )
                    return
                except NonResponsiveOutputError as retry_exc:
                    generate_progress_count = _stop_generate_progress()
                    _emit_terminal_generate_error(
                        config,
                        emit_event,
                        retry_exc,
                        reason_code=NON_RESPONSIVE_REASON_CODE,
                        raw=_non_responsive_raw(retry_exc),
                        retries=retries,
                        generate_progress_count=generate_progress_count,
                    )
                    return
                except Exception as retry_exc:
                    retry_exc.retries = retries
                    raise
            else:
                raise
    finally:
        generate_progress_count = _stop_generate_progress()

    raw_result = gen_result["text"]
    if output_format == "md":
        stripped_text, fence_stripped = _strip_outer_markdown_fence(raw_result)
        if fence_stripped:
            LOG.info(
                "Stripped whole-output markdown fence from talent %s (day=%s, schedule=%s)",
                name,
                config.get("day"),
                config.get("schedule"),
            )
            raw_result = stripped_text
    usage_data = gen_result.get("usage")

    # Run post-hooks
    try:
        result = _run_post_hooks(raw_result, config)
    except TalentHookError as exc:
        _emit_terminal_hook_error(
            config,
            emit_event,
            exc,
            generate_progress_count=generate_progress_count,
        )
        return
    if _expected_output_blank(config, raw_result, result):
        _mark_terminal_error_evented(config)
        emit_event(
            {
                "event": "error",
                "error": _NO_OUTPUT_ERROR,
                "reason_code": "no_output",
                "provider": config.get("provider"),
                "terminal": True,
                "ts": now_ms(),
                "generate_progress_count": generate_progress_count,
            }
        )
        return

    if (
        output_path
        and result
        and not _output_valid_for_schema(
            result,
            config.get("output"),
            runtime_json_schema,
        )
    ):
        _mark_terminal_error_evented(config)
        error_event: dict[str, Any] = {
            "event": "error",
            "error": _schema_invalid_message(gen_result),
            "reason_code": "schema_invalid",
            "provider": config.get("provider"),
            "terminal": True,
            "ts": now_ms(),
            "generate_progress_count": generate_progress_count,
        }
        # Describes the raw provider text, not the post-hook candidate the gate
        # rejected: it can read valid=True when a hook produced invalid output.
        if "schema_validation" in gen_result:
            error_event["schema_validation"] = gen_result["schema_validation"]
        if retries:
            error_event["retries"] = retries
        emit_event(error_event)
        return

    # Write output
    output_changed = False
    if output_path and result:
        output_changed = _write_output(output_path, result)

    # Emit finish event
    completed_at_ms = now_ms()
    degraded = _classify_degraded(usage_data, config)
    if (
        output_path
        and result
        and not degraded
        and _schema_validation_clean(gen_result, result, config)
    ):
        _write_clean_provenance(
            config,
            output_path,
            result,
            runtime_json_schema,
            completed_at_ms,
        )

    finish_event: dict[str, Any] = {
        "event": "finish",
        "ts": completed_at_ms,
        "result": result,
        "cache_hit": False,
        "output_changed": output_changed,
        "completed_at_ms": completed_at_ms,
        "generate_progress_count": generate_progress_count,
    }
    if usage_data:
        finish_event["usage"] = usage_data
    if "schema_validation" in gen_result:
        finish_event["schema_validation"] = gen_result["schema_validation"]
    if "input_budget" in gen_result:
        finish_event["input_budget"] = gen_result["input_budget"]
    if "request_budget" in gen_result:
        finish_event["request_budget"] = gen_result["request_budget"]
    if degraded:
        finish_event["degraded"] = degraded
    if retries:
        finish_event["retries"] = retries
    emit_event(finish_event)


async def _run_talent(
    config: dict,
    emit_event: Callable[[dict], None],
    dry_run: bool = False,
) -> None:
    """Execute a talent based on config.

    Unified execution path for all talent types. Handles:
    - Skip conditions (disabled, no input, etc.)
    - Output existence checking (skip if exists unless refresh)
    - Pre/post hooks
    - Dry-run mode
    - Routing to tool or generate execution

    Args:
        config: Fully prepared config dict
        emit_event: Callback to emit JSONL events
        dry_run: If True, emit dry_run event instead of calling LLM
    """
    name = config["name"]
    provider = config.get("provider", "google")
    model = config.get("model")
    is_cogitate = config["type"] == "cogitate"
    refresh = config.get("refresh", False)
    output_path = Path(config["output_path"]) if config.get("output_path") else None

    # Expose dry-run to hooks so a pre-hook can skip side effects (e.g. steward's
    # deterministic health.md write) during model-free dry runs.
    config["dry_run"] = dry_run

    # Emit start event
    start_event: dict[str, Any] = {
        "event": "start",
        "ts": now_ms(),
        "prompt": config.get("prompt", ""),
        "name": name,
        "model": model or "unknown",
        "provider": provider,
    }
    if config.get("session_id"):
        start_event["session_id"] = config["session_id"]
    if config.get("chat_id"):
        start_event["chat_id"] = config["chat_id"]
    emit_event(start_event)

    # Handle skip conditions
    skip_reason = config.get("skip_reason")
    if skip_reason:
        LOG.info("Config %s skipped: %s", name, skip_reason)
        emit_event(
            {
                "event": "finish",
                "ts": now_ms(),
                "result": "",
                "skipped": skip_reason,
            }
        )
        if config.get("day"):
            day_log(config["day"], f"talent {name} skipped ({skip_reason})")
        return

    # Capture state before pre-hooks
    before_values = {
        "prompt": config.get("prompt", ""),
        "system_instruction": config.get("system_instruction", ""),
        "user_instruction": config.get("user_instruction", ""),
        "transcript": config.get("transcript", ""),
    }
    before_values["extra_context"] = config.get("extra_context", "")

    # Run pre-hooks
    try:
        modifications = _run_pre_hooks(config)
    except TalentHookError as exc:
        _emit_terminal_hook_error(config, emit_event, exc)
        return
    template_vars = modifications.pop("template_vars", None)
    for key, value in modifications.items():
        config[key] = value
    if template_vars:
        LOG.info("Pre-hook template_vars: %s", list(template_vars.keys()))
        _apply_template_vars(config, template_vars)

    # Handle skip conditions set by pre-hooks
    skip_reason = config.get("skip_reason")
    if skip_reason:
        LOG.info("Config %s skipped by pre-hook: %s", name, skip_reason)
        emit_event(
            {
                "event": "finish",
                "ts": now_ms(),
                "result": "",
                "skipped": skip_reason,
            }
        )
        if config.get("day"):
            day_log(config["day"], f"talent {name} skipped ({skip_reason})")
        return

    # Dry-run mode
    if dry_run:
        emit_event(_build_dry_run_event(config, before_values))
        return

    if output_path and not refresh and _try_reuse_output(config, emit_event):
        return

    # Execute based on talent type
    if is_cogitate:
        await _execute_with_tools(config, emit_event)
    else:
        await _execute_generate(config, emit_event)

    # Log completion
    if config.get("day") and not config.get("_terminal_error_evented"):
        day_log(config["day"], f"talent {name} ok")


# =============================================================================
# Utility Functions
# =============================================================================


def scan_day(day: str) -> dict[str, list[str]]:
    """Return lists of processed and pending daily generator output files.

    Only scans daily generators (schedule='daily'). Segment generators are
    stored within segment directories and are not included here.
    """
    day_dir = day_path(day)
    daily_generators = get_talent_configs(
        type="generate", schedule="daily", include_disabled=True
    )
    processed: list[str] = []
    pending: list[str] = []
    for key, meta in sorted(daily_generators.items()):
        output_format = meta.get("output")
        output_file = get_output_path(day_dir, key, output_format=output_format)
        if output_file.exists():
            processed.append(os.path.join("talents", output_file.name))
        else:
            pending.append(os.path.join("talents", output_file.name))
    return {"processed": sorted(processed), "repairable": sorted(pending)}


# =============================================================================
# Main Entry Point
# =============================================================================


async def main_async() -> None:
    """NDJSON-based CLI for talents."""

    parser = argparse.ArgumentParser(
        description="solstone Talent CLI - Accepts NDJSON input via stdin"
    )
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="Show what would be sent to the provider without calling the LLM",
    )
    args = setup_cli(parser)
    require_solstone()
    dry_run = args.dry_run

    app_logger = setup_logging(args.verbose)
    event_writer = JSONEventWriter(None)
    loop = asyncio.get_running_loop()
    main_task = asyncio.current_task()
    registered_signals: list[signal.Signals] = []
    if main_task:
        for sig in (signal.SIGTERM, signal.SIGINT):
            try:
                loop.add_signal_handler(sig, main_task.cancel)
                registered_signals.append(sig)
            except (NotImplementedError, RuntimeError):
                LOG.debug("Signal handler registration unavailable for %s", sig)

    def emit_event(data: Event) -> None:
        if "ts" not in data:
            data["ts"] = now_ms()
        event_writer.emit(data)

    try:
        app_logger.info("Processing NDJSON input from stdin")
        config: dict[str, Any] | None = None
        for line in sys.stdin:
            line = line.strip()
            if not line:
                continue

            try:
                config = None
                request = json.loads(line)
                config = prepare_config(request)

                error = validate_config(config)
                if error:
                    emit_event({"event": "error", "error": error, "ts": now_ms()})
                    continue

                await _run_talent(config, emit_event, dry_run=dry_run)

            except json.JSONDecodeError as e:
                emit_event(
                    {
                        "event": "error",
                        "error": f"Invalid JSON: {str(e)}",
                        "ts": now_ms(),
                    }
                )
            except Exception as e:
                if getattr(e, "_evented", False):
                    continue
                from solstone.think.models import IncompleteJSONError

                provider = str(config.get("provider") or "") if config else ""
                reason_code = classify_provider_error(e, provider)
                if config and not getattr(e, "_brain_runtime_recorded", False):
                    component: Literal["generate", "cogitate"] = (
                        "cogitate" if config.get("type") == "cogitate" else "generate"
                    )
                    expected_fingerprint_sha256 = config.get(
                        "_brain_runtime_fingerprint_sha256"
                    )
                    _record_brain_runtime_failure(
                        reason_code,
                        component,
                        expected_fingerprint_sha256=(
                            expected_fingerprint_sha256
                            if isinstance(expected_fingerprint_sha256, str)
                            else None
                        ),
                    )
                event = {
                    "event": "error",
                    "error": str(e),
                    "reason_code": reason_code,
                    "provider": provider,
                    "trace": traceback.format_exc(),
                    "ts": now_ms(),
                }
                if isinstance(e, IncompleteJSONError):
                    from solstone.think._extraction_utils import log_extraction_failure

                    event["partial_text_length"] = len(e.partial_text)
                    event["partial_text_tail"] = e.partial_text[-500:]
                    name = config.get("name", "unknown") if config else "unknown"
                    log_extraction_failure(e, name)
                retries = getattr(e, "retries", None)
                if retries:
                    event["retries"] = retries
                emit_event(event)

    except Exception as exc:
        err = {
            "event": "error",
            "error": str(exc),
            "trace": traceback.format_exc(),
        }
        if not getattr(exc, "_evented", False):
            emit_event(err)
        raise
    finally:
        for sig in registered_signals:
            loop.remove_signal_handler(sig)
        event_writer.close()


def main() -> None:
    """Entry point wrapper."""
    try:
        asyncio.run(main_async())
    except asyncio.CancelledError:
        sys.exit(0)


__all__ = [
    "prepare_config",
    "validate_config",
    "scan_day",
]

if __name__ == "__main__":
    main()
