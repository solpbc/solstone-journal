# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import html
import json
import re
from pathlib import Path

from solstone.apps.health import copy as health_copy
from solstone.convey import backlog_copy

WORKSPACE_PATH = Path(__file__).resolve().parents[1] / "workspace.html"
LOGS_COPY_KEYS = [
    "LOGS_SERVICE_FILTER_LABEL",
    "LOGS_STREAM_FILTER_LABEL",
    "LOGS_LEVEL_FILTER_LABEL",
    "LOGS_LEVEL_OPTION_ALL",
    "LOGS_LEVEL_OPTION_ERROR",
    "LOGS_LEVEL_OPTION_WARNING",
    "LOGS_LEVEL_OPTION_INFO",
    "LOGS_SERVICE_COLLAPSED",
]
HEALTH_GLANCE_COPY_KEYS = [
    "HEALTH_GLANCE_OK",
    "HEALTH_GLANCE_SERVICES_ATTENTION",
    "HEALTH_GLANCE_CATCHING_UP",
    "HEALTH_GLANCE_OBSERVER_SILENT",
    "HEALTH_GLANCE_SERVICES_UNREACHABLE",
]
HEALTH_GLANCE_LITERALS = {
    "HEALTH_GLANCE_OK": "everything's working — last observation {age} ago.",
    "HEALTH_GLANCE_SERVICES_ATTENTION": "{n} service(s) need attention — {service_names}.",
    "HEALTH_GLANCE_CATCHING_UP": (
        "I'm catching up on {n} task(s) in the background — last update {age} ago."
    ),
    "HEALTH_GLANCE_OBSERVER_SILENT": (
        "I haven't heard from your observer in {age} — it may have stopped."
    ),
    "HEALTH_GLANCE_SERVICES_UNREACHABLE": (
        "I couldn't reach my own services — check that solstone is running."
    ),
}


def _backlog(
    *,
    pending_days=0,
    stuck_days=0,
    days=None,
    errors=None,
    degraded=False,
) -> dict:
    return {
        "degraded": degraded,
        "pending_days": pending_days,
        "stuck_days": stuck_days,
        "days": days or [],
        "errors": errors or [],
    }


def _render_health_workspace(health_env) -> str:
    env = health_env()
    response = env.client.get("/app/health/")
    assert response.status_code == 200
    return response.get_data(as_text=True)


def _render_health_workspace_with_stats(health_env, stats_payload: dict) -> str:
    env = health_env()
    (env.journal / "stats.json").write_text(
        json.dumps(stats_payload),
        encoding="utf-8",
    )
    response = env.client.get("/app/health/")
    assert response.status_code == 200
    return html.unescape(response.get_data(as_text=True))


def _copy(key: str, *, pending=None, stuck=None) -> str:
    return (
        getattr(backlog_copy, key)
        .replace("{pending_n}", "" if pending is None else str(pending))
        .replace("{stuck_n}", "" if stuck is None else str(stuck))
    )


def _section_by_id(rendered: str, section_id: str) -> str:
    id_pos = rendered.index(f'id="{section_id}"')
    start = rendered.rfind("<section", 0, id_pos)
    end = rendered.index("</section>", id_pos) + len("</section>")
    return rendered[start:end]


def _optional_section_by_id(rendered: str, section_id: str) -> str:
    marker = f'id="{section_id}"'
    if marker not in rendered:
        return ""
    return _section_by_id(rendered, section_id)


def _verdict_text(rendered: str) -> str:
    section = _section_by_id(rendered, "backlogVerdict")
    match = re.search(r'<p class="backlog-verdict-line">(?P<text>.*?)</p>', section)
    assert match is not None
    return match.group("text")


def test_logs_copy_and_controls_render(health_env):
    rendered = _render_health_workspace(health_env)
    decoded = html.unescape(rendered)

    for value in (
        health_copy.LOGS_SERVICE_FILTER_LABEL,
        health_copy.LOGS_STREAM_FILTER_LABEL,
        health_copy.LOGS_LEVEL_FILTER_LABEL,
        health_copy.LOGS_LEVEL_OPTION_ALL,
        health_copy.LOGS_LEVEL_OPTION_ERROR,
        health_copy.LOGS_LEVEL_OPTION_WARNING,
        health_copy.LOGS_LEVEL_OPTION_INFO,
    ):
        assert value in decoded

    assert 'label for="logServiceFilter"' in rendered
    assert 'label for="logLevelFilter"' in rendered
    assert 'label for="logStreamFilter"' in rendered
    assert '<select id="logLevelFilter">' in rendered
    assert decoded.count("<option value=") >= 8
    assert 'id="logsAnnouncer"' in rendered
    assert 'class="logs-announcer"' in rendered
    assert 'role="status"' in rendered
    assert 'aria-live="polite"' in rendered


def test_health_logs_copy_script_carries_all_keys(health_env):
    rendered = _render_health_workspace(health_env)

    assert "window.HEALTH_LOGS_COPY" in rendered
    for key in LOGS_COPY_KEYS:
        assert f"{key}:" in rendered

    script_values = {}
    for key in LOGS_COPY_KEYS:
        match = re.search(rf"{key}:\s*(?P<value>\"(?:\\.|[^\"])*\")", rendered)
        assert match is not None, key
        script_values[key] = json.loads(match.group("value"))

    assert script_values == {key: getattr(health_copy, key) for key in LOGS_COPY_KEYS}


def test_health_glance_copy_constants_are_literal():
    for key, value in HEALTH_GLANCE_LITERALS.items():
        assert getattr(health_copy, key) == value


def test_health_glance_copy_script_carries_all_keys(health_env):
    rendered = _render_health_workspace(health_env)

    assert "window.HEALTH_GLANCE_COPY" in rendered
    for key in HEALTH_GLANCE_COPY_KEYS:
        assert f"{key}:" in rendered

    script_values = {}
    for key in HEALTH_GLANCE_COPY_KEYS:
        match = re.search(rf"{key}:\s*(?P<value>\"(?:\\.|[^\"])*\")", rendered)
        assert match is not None, key
        script_values[key] = json.loads(match.group("value"))

    assert script_values == {
        key: getattr(health_copy, key) for key in HEALTH_GLANCE_COPY_KEYS
    }


def test_select_glance_sentence_exists(health_env):
    rendered = _render_health_workspace(health_env)

    assert "function selectGlanceSentence(state, now)" in rendered


def test_glance_precedence_order(health_env):
    rendered = _render_health_workspace(health_env)
    start = rendered.index("function selectGlanceSentence(state, now)")
    end = rendered.index("function formatGlanceSentence", start)
    selector = rendered[start:end]

    witnesses = [
        "HEALTH_GLANCE_SERVICES_UNREACHABLE",
        "HEALTH_GLANCE_SERVICES_ATTENTION",
        "HEALTH_GLANCE_OBSERVER_SILENT",
        "HEALTH_GLANCE_CATCHING_UP",
        "HEALTH_GLANCE_OK",
    ]
    positions = [selector.index(witness) for witness in witnesses]
    assert positions == sorted(positions)


def test_error_summary_dom_order(health_env):
    rendered = _render_health_workspace(health_env)

    assert rendered.index('id="healthGlance"') < rendered.index('id="backlogVerdict"')
    assert rendered.index('id="backlogVerdict"') < rendered.index('class="vitals-bar"')
    assert rendered.index('class="vitals-bar"') < rendered.index('id="errorSummary"')
    assert rendered.index('id="errorSummary"') < rendered.index(
        'class="dashboard-card observe-card"'
    )


def test_backlog_verdict_caught_up(health_env):
    rendered = _render_health_workspace_with_stats(
        health_env,
        {"backlog": _backlog()},
    )

    assert _verdict_text(rendered) == backlog_copy.BACKLOG_VERDICT_CAUGHT_UP


def test_backlog_verdict_pending_only_singular_and_plural(health_env):
    rendered = _render_health_workspace_with_stats(
        health_env,
        {"backlog": _backlog(pending_days=1)},
    )

    assert _verdict_text(rendered) == backlog_copy.BACKLOG_VERDICT_PENDING_ONLY_SINGULAR
    assert "1 day(s)" not in _section_by_id(rendered, "backlogVerdict")

    rendered = _render_health_workspace_with_stats(
        health_env,
        {"backlog": _backlog(pending_days=4)},
    )

    assert _verdict_text(rendered) == _copy(
        "BACKLOG_VERDICT_PENDING_ONLY_PLURAL",
        pending=4,
    )


def test_backlog_verdict_stuck_only_singular_and_plural(health_env):
    rendered = _render_health_workspace_with_stats(
        health_env,
        {"backlog": _backlog(stuck_days=1)},
    )

    assert _verdict_text(rendered) == backlog_copy.BACKLOG_VERDICT_STUCK_ONLY_SINGULAR

    rendered = _render_health_workspace_with_stats(
        health_env,
        {"backlog": _backlog(stuck_days=3)},
    )

    assert _verdict_text(rendered) == _copy(
        "BACKLOG_VERDICT_STUCK_ONLY_PLURAL",
        stuck=3,
    )


def test_backlog_verdict_both_does_not_render_sum(health_env):
    rendered = _render_health_workspace_with_stats(
        health_env,
        {"backlog": _backlog(pending_days=3, stuck_days=2)},
    )

    verdict_region = _section_by_id(rendered, "backlogVerdict")
    backlog_region = verdict_region + _optional_section_by_id(
        rendered, "backlogNeedsHand"
    )
    assert "2" in verdict_region
    assert "3" in verdict_region
    assert "5" not in backlog_region


def test_backlog_missing_key_renders_cant_tell(health_env):
    rendered = _render_health_workspace_with_stats(health_env, {})

    assert _verdict_text(rendered) == backlog_copy.BACKLOG_VERDICT_CANT_TELL
    assert backlog_copy.BACKLOG_VERDICT_CAUGHT_UP not in _section_by_id(
        rendered,
        "backlogVerdict",
    )


def test_backlog_degraded_renders_cant_tell_without_bucket(health_env):
    rendered = _render_health_workspace_with_stats(
        health_env,
        {"backlog": _backlog(degraded=True)},
    )

    assert _verdict_text(rendered) == backlog_copy.BACKLOG_VERDICT_CANT_TELL
    assert backlog_copy.BACKLOG_VERDICT_CAUGHT_UP not in _section_by_id(
        rendered,
        "backlogVerdict",
    )
    assert 'id="backlogNeedsHand"' not in rendered


def test_backlog_needs_hand_bucket_rows(health_env):
    rendered = _render_health_workspace_with_stats(
        health_env,
        {
            "backlog": _backlog(
                stuck_days=1,
                days=[
                    {
                        "day": "20260320",
                        "state": "stuck",
                        "segments": 2,
                        "units": 1,
                        "reason": "corrupt_raw",
                    },
                    {
                        "day": "20260321",
                        "state": "pending",
                        "segments": 0,
                        "units": 4,
                        "reason": "failing_step",
                    },
                    {
                        "day": "20260322",
                        "state": "pending",
                        "segments": 8,
                        "units": 0,
                        "reason": "failing_step",
                    },
                ],
                errors=[
                    {
                        "day": "20260321",
                        "stage": "terminal_states",
                        "message": "boom",
                    }
                ],
            )
        },
    )

    section = _section_by_id(rendered, "backlogNeedsHand")
    assert backlog_copy.BACKLOG_BUCKET_HEADING in section
    assert backlog_copy.BACKLOG_BUCKET_DESCRIPTION in section
    assert backlog_copy.BACKLOG_DAY_BADGE in section
    assert "20260320" in section
    assert "20260321" in section
    assert "20260322" not in section
    assert backlog_copy.BACKLOG_REASON_CORRUPT_RAW in section
    assert backlog_copy.BACKLOG_REASON_FAILING_STEP in section
    assert '<span class="backlog-depth">3</span>' in section
    assert '<span class="backlog-depth">4</span>' in section
    assert "<details" not in section


def test_backlog_reprocess_buttons_render(health_env):
    day = "20260320"
    rendered = _render_health_workspace_with_stats(
        health_env,
        {
            "backlog": _backlog(
                stuck_days=1,
                days=[
                    {
                        "day": day,
                        "state": "stuck",
                        "segments": 2,
                        "units": 1,
                        "reason": "corrupt_raw",
                    },
                ],
            )
        },
    )

    section = _section_by_id(rendered, "backlogNeedsHand")
    assert f'data-day="{day}"' in section
    assert 'data-flavor="process-now"' in section
    assert 'data-flavor="from-scratch"' in section
    assert backlog_copy.BACKLOG_ACTION_PROCESS_NOW in section
    assert backlog_copy.BACKLOG_ACTION_REDO_SCRATCH in section
    assert backlog_copy.BACKLOG_CONFIRM_REDO_SCRATCH in section


def test_backlog_copy_constants_render_from_shared_source(health_env):
    rendered = _render_health_workspace_with_stats(
        health_env,
        {
            "backlog": _backlog(
                pending_days=3,
                stuck_days=2,
                days=[
                    {
                        "day": "20260320",
                        "state": "stuck",
                        "segments": 1,
                        "units": 0,
                        "reason": "corrupt_raw",
                    },
                    {
                        "day": "20260321",
                        "state": "pending",
                        "segments": 0,
                        "units": 1,
                        "reason": "failing_step",
                    },
                ],
                errors=[
                    {
                        "day": "20260321",
                        "stage": "segment_completion",
                        "message": "boom",
                    }
                ],
            )
        },
    )

    assert _verdict_text(rendered) == _copy(
        "BACKLOG_VERDICT_BOTH_PLURAL",
        pending=3,
        stuck=2,
    )
    section = _section_by_id(rendered, "backlogNeedsHand")
    assert re.search(r"<h2>(?P<text>.*?)</h2>", section).group("text") == getattr(
        backlog_copy, "BACKLOG_BUCKET_HEADING"
    )
    assert re.search(
        r'<p class="backlog-needs-hand-desc">(?P<text>.*?)</p>',
        section,
    ).group("text") == getattr(backlog_copy, "BACKLOG_BUCKET_DESCRIPTION")
    reasons = re.findall(
        r'<span class="backlog-row-reason">(?P<text>.*?)</span>',
        section,
    )
    assert reasons == [
        getattr(backlog_copy, "BACKLOG_REASON_CORRUPT_RAW"),
        getattr(backlog_copy, "BACKLOG_REASON_FAILING_STEP"),
    ]


def test_status_summary_text_removed(health_env):
    source = WORKSPACE_PATH.read_text(encoding="utf-8")
    rendered = _render_health_workspace(health_env)

    assert "statusSummaryText" not in source
    assert 'id="statusSummaryText"' not in rendered


def test_vitals_sections_have_role_group(health_env):
    rendered = _render_health_workspace(health_env)

    sections = re.findall(r'<div class="vitals-section"[^>]*role="group"', rendered)
    assert len(sections) == 6
    assert rendered.count('class="vitals-label" aria-hidden="true"') == 6
    values = re.findall(r'<div class="vitals-value"[^>]*aria-hidden="true"', rendered)
    assert len(values) == 6


def test_cost_fetch_uses_em_dash_on_failure():
    source = WORKSPACE_PATH.read_text(encoding="utf-8")
    start = source.index("fetch('/app/tokens/api/usage?day='")
    end = source.index("// State management", start)
    cost_fetch = source[start:end]

    assert ".catch(() =>" in cost_fetch
    assert "textContent = '—';" in cost_fetch


def _health_info_catch_block(source: str) -> str:
    fetch_start = source.index("fetch('/app/health/api/info')")
    catch_start = source.index("    .catch(() => {", fetch_start)
    catch_end = source.index("    });", catch_start) + len("    });")
    return source[catch_start:catch_end]


def test_connection_catch_has_no_dom_writes():
    source = WORKSPACE_PATH.read_text(encoding="utf-8")
    catch_block = _health_info_catch_block(source)

    assert "document.createElement" not in catch_block
    assert "appendChild" not in catch_block
    assert ".textContent =" not in catch_block
    assert ".innerHTML =" not in catch_block


def test_connect_error_indicator_handled_in_renderer():
    source = WORKSPACE_PATH.read_text(encoding="utf-8")
    catch_block = _health_info_catch_block(source)
    update_start = source.index("function updateVitals()")
    branch_end = source.index(
        "    // Combine running and crashed services", update_start
    )
    update_vitals_branch = source[update_start:branch_end]

    assert "' Connection error'" not in catch_block
    assert "' Connection error'" in update_vitals_branch
    assert "indicator.className = 'status-indicator crashed';" in update_vitals_branch


def test_no_legacy_stream_classes_in_render_paths(health_env):
    rendered = _render_health_workspace(health_env)

    assert 'class="logs-line stderr"' not in rendered
    assert 'class="logs-line log"' not in rendered
    assert re.search(r"\.logs-line\.stderr\s*\{", rendered) is None
    assert re.search(r"\.logs-line\.log\s*\{", rendered) is None


def test_deep_link_branch_uses_classifier():
    source = WORKSPACE_PATH.read_text(encoding="utf-8")
    start = source.index(
        "// Deep-link: display log file content if ?log= param is present"
    )
    end = source.index("function focusRecentErrors", start)
    branch = source[start:end]

    assert "classifyLogLevel(" in branch
    assert 'className = "logs-line stderr"' not in branch
    assert "className = 'logs-line stderr'" not in branch
    assert 'className = "logs-line log"' not in branch
    assert "className = 'logs-line log'" not in branch
    assert "data-hhmmss" not in branch
    assert "dataset.hhmmss" not in branch
