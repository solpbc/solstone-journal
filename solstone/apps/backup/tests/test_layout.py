# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Layout-source tests for the backup app."""

from __future__ import annotations

import re
from html.parser import HTMLParser
from pathlib import Path

_MEDIA_OPEN = re.compile(r"@media\s*\(\s*max-width\s*:\s*(\d+)px\s*\)\s*\{")
_CSS_RULE = re.compile(r"(?P<selector>[^{}]+)\{(?P<body>[^{}]*)\}", re.DOTALL)
_LEFT_CLEARANCE = re.compile(
    r"\b(?:padding-left|margin-left)\s*:\s*[^;]*--menu-bar-width[^;]*;",
    re.DOTALL,
)
_BOTTOM_CLEARANCE = re.compile(
    r"\b(?:padding-bottom|margin-bottom)\s*:\s*[^;]*--app-bar-height[^;]*;",
    re.DOTALL,
)


def _backup_css() -> str:
    return Path("core/crates/solstone-core-backup-web/assets/backup.css").read_text(encoding="utf-8")


def _backup_js() -> str:
    return Path("core/crates/solstone-core-backup-web/assets/backup.js").read_text(encoding="utf-8")


def _backup_workspace_html() -> str:
    return Path("core/crates/solstone-core-backup-web/assets/workspace.html").read_text(encoding="utf-8")


def _media_spans(css: str) -> list[tuple[int, int, int, str]]:
    spans: list[tuple[int, int, int, str]] = []
    for match in _MEDIA_OPEN.finditer(css):
        depth = 1
        index = match.end()
        while index < len(css) and depth > 0:
            char = css[index]
            if char == "{":
                depth += 1
            elif char == "}":
                depth -= 1
            index += 1
        if depth != 0:
            raise AssertionError("unterminated @media block in backup.css")
        spans.append(
            (match.start(), index, int(match.group(1)), css[match.end() : index - 1])
        )
    return spans


def _narrow_media_blocks(css: str) -> list[str]:
    return [body for _start, _end, width, body in _media_spans(css) if width <= 768]


def _selector_root_tokens(selector: str) -> set[str]:
    tokens: set[str] = set()
    if re.search(r"(?<![\w-])\.backup-shell(?![\w-])", selector):
        tokens.add("backup-shell")
    if re.search(r"\[data-backup-root(?:[\]\s=~|^$*])", selector):
        tokens.add("data-backup-root")
    return tokens


def _clearance_tokens(blocks: list[str], declaration: re.Pattern[str]) -> set[str]:
    tokens: set[str] = set()
    for block in blocks:
        for match in _CSS_RULE.finditer(block):
            selector_tokens = _selector_root_tokens(match.group("selector"))
            if selector_tokens and declaration.search(match.group("body")):
                tokens.update(selector_tokens)
    return tokens


def _class_token_present(html: str, class_name: str) -> bool:
    return any(
        class_name in class_attr.split()
        for class_attr in re.findall(r'class="([^"]*)"', html)
    )


def _root_token_present(html: str, token: str) -> bool:
    if token.startswith("data-"):
        return bool(re.search(rf"\s{re.escape(token)}(?:[=\s>]|$)", html))
    return _class_token_present(html, token)


def _rendered_backup_html(backup_env) -> str:
    response = backup_env().client.get("/app/backup/workspace")
    assert response.status_code == 200
    return response.get_data(as_text=True)


class _OffloadDaysTemplateParser(HTMLParser):
    def __init__(self) -> None:
        super().__init__(convert_charrefs=True)
        self._offload_days_depth = 0
        self.saw_offload_days = False
        self.saw_day_template = False
        self.template_inside_offload_days = False

    def handle_starttag(self, tag: str, attrs: list[tuple[str, str | None]]) -> None:
        attr_map = dict(attrs)
        starts_offload_days = tag == "div" and "data-offload-days" in attr_map
        in_offload_days = self._offload_days_depth > 0 or starts_offload_days

        if tag == "div" and in_offload_days:
            self._offload_days_depth += 1
        if starts_offload_days:
            self.saw_offload_days = True

        if tag == "template" and "data-offload-day-template" in attr_map:
            self.saw_day_template = True
            if in_offload_days:
                self.template_inside_offload_days = True

    def handle_startendtag(self, tag: str, attrs: list[tuple[str, str | None]]) -> None:
        self.handle_starttag(tag, attrs)
        self.handle_endtag(tag)

    def handle_endtag(self, tag: str) -> None:
        if tag == "div" and self._offload_days_depth > 0:
            self._offload_days_depth -= 1


def test_backup_css_hidden_and_disabled_controls() -> None:
    css = _backup_css()

    for selector in (
        ".backup-offload-summary[hidden]",
        ".backup-offload-grid[hidden]",
        ".backup-offload-days[hidden]",
        ".backup-offload-day[hidden]",
        ".backup-panel-actions[hidden]",
        ".backup-offload-chip[hidden]",
        ".backup-offload-stakes[hidden]",
        ".backup-offload-tiering[hidden]",
        ".backup-offload-controls[hidden]",
        ".backup-teardown-gate[hidden]",
    ):
        assert selector in css
    assert ".backup-panel button:disabled" in css
    assert "cursor: not-allowed" in css
    assert "opacity: 0.45" in css
    assert ".backup-panel button.primary:hover:not(:disabled)" in css
    assert ".backup-panel button.danger:hover:not(:disabled)" in css
    assert ".backup-panel button.primary:hover {" not in css
    assert ".backup-panel button.danger:hover {" not in css
    assert ".backup-offload-stakes--warning" in css
    assert ".backup-offload-stakes--note" in css
    assert "background: #fbf1d6;" in css
    assert "color: #7c4a0c;" in css
    assert "background: #e7f1e9;" in css
    assert "color: #166534;" in css
    assert "background: #fbeaea;" in css
    assert "border-left: 3px solid #9b1c1c;" in css
    assert "max-width: 12rem;" in css
    assert "!important" not in css


def test_offload_day_template_survives_days_container_rerender() -> None:
    """The day template must survive replaceChildren().

    It cannot live inside the cleared container.
    """
    parser = _OffloadDaysTemplateParser()
    parser.feed(_backup_workspace_html())

    assert parser.saw_offload_days
    assert parser.saw_day_template
    assert not parser.template_inside_offload_days


def test_offload_js_source_contracts() -> None:
    js = _backup_js()

    assert "const BYTES_PER_GB = 1000000000;" in js
    assert "const BYTES_PER_MB = 1000000;" in js
    assert "return Math.round(parsed * BYTES_PER_GB);" in js
    assert "budget_bytes: gbToBytes(budgetField.value)" in js
    assert "floor_bytes: gbToBytes(floorField.value)" in js
    assert "budget_bytes: budgetField.value" not in js
    assert "floor_bytes: floorField.value" not in js
    assert "await startOperation('/app/backup/offload/restore', { day });" in js
    assert re.findall(r"postJson\('(/app/backup/offload/disable)'", js) == [
        "/app/backup/offload/disable"
    ]
    assert "delete next.operation;" in js
    assert "applyPayload(await postJson('/app/backup/offload" not in js
    assert "kind === 'offload_restore'" in js
    assert "showMessage('[data-offload-restore-status]', '');" in js
    assert "applyCopy(clone, copy);" in js
    assert "offloadLabels.mb_suffix" in js
    assert "offloadLabels.under_1mb" in js
    assert "return '0.01';" in js
    assert "Number.isInteger(rounded)" not in js

    format_bytes = js[
        js.index("function formatBytes(bytes)") : js.index("function gbToBytes")
    ]
    zero_branch = "bytes === 0"
    sub_mb_branch = "bytes < BYTES_PER_MB"
    gb_mb_split = "const isGb = bytes >= BYTES_PER_GB;"
    assert zero_branch in format_bytes
    assert sub_mb_branch in format_bytes
    assert gb_mb_split in format_bytes
    assert format_bytes.index(zero_branch) < format_bytes.index(sub_mb_branch)
    assert format_bytes.index(sub_mb_branch) < format_bytes.index(gb_mb_split)
    assert "offloadLabels.under_1mb" in format_bytes
    assert "offloadLabels.mb_suffix" in format_bytes

    offload_error = js[
        js.index("function offloadActionError(err)") : js.index(
            "function maybeOpenPortal"
        )
    ]
    assert "reason === 'invalid_config_value'" in offload_error
    assert "offloadCopy.invalid_limits" in offload_error
    assert "operationLabels[reason]" in offload_error
    assert "offloadCopy.action_error" in offload_error
    assert "destinationLabels" not in offload_error
    assert "error_intro" not in offload_error
    offload_catch = js[
        js.index("if (action && action.startsWith('offload-'))") : js.index(
            "} else {\n          showError('[data-operation-error]'"
        )
    ]
    assert "offloadActionError(err)" in offload_catch
    assert "showError" not in offload_catch

    timestamp_validity = js[
        js.index("function formatTime(value)") : js.index("function validTimestamp")
    ]
    for guard in (
        "typeof value !== 'number'",
        "!Number.isFinite(value)",
        "value <= 0",
    ):
        assert guard in timestamp_validity
    assert (
        timestamp_validity.index("typeof value !== 'number'")
        < timestamp_validity.index("!Number.isFinite(value)")
        < timestamp_validity.index("value <= 0")
    )
    timestamp_relative_duration = js[
        js.index("function timestampRelativeDuration(value)") : js.index(
            "function timestampDisplay(value)"
        )
    ]
    assert timestamp_relative_duration.index("elapsed >= 0") < timestamp_relative_duration.index(
        "relativeTime(elapsed)"
    )

    working_proof = js[
        js.index("function formatWorkingProofDisplay(result)") : js.index(
            "function formatRestoreResultDisplay(result)"
        )
    ]
    assert "result.last_ok_time" in working_proof
    assert "|| result.time" not in working_proof
    assert "result.time" not in working_proof
    assert "parts.push" not in working_proof
    assert "reason" not in working_proof

    restore_result = js[
        js.index("function formatRestoreResultDisplay(result)") : js.index(
            "function offloadRestoreExpectation(bytes)"
        )
    ]
    assert "timestampDisplay(result.time)" in restore_result
    assert "offloadRestoreReasonLabel(result.reason)" in restore_result
    assert "parts.push(reason)" in restore_result

    restore_expectation = js[
        js.index("function offloadRestoreExpectation(bytes)") : js.index(
            "function teardownConfirmPhrase"
        )
    ]
    assert "offloadCopy.restore_expectation" in restore_expectation
    assert "formatBytes(bytes)" in restore_expectation

    enable_action = js[
        js.index("if (action === 'offload-enable')") : js.index(
            "if (action === 'offload-save')"
        )
    ]
    assert "limits.exactlyOnePositive" in enable_action
    assert "offloadCopy.invalid_limits" in enable_action
    config_post = "postJson('/app/backup/offload/config', offloadConfigBody())"
    enable_post = "postJson('/app/backup/offload/enable')"
    assert enable_action.index(config_post) < enable_action.index(enable_post)

    save_action = js[
        js.index("if (action === 'offload-save')") : js.index(
            "if (action === 'offload-disable')"
        )
    ]
    assert "offloadLimitState" not in save_action
    assert config_post in save_action

    catch_clear = (
        "if (action === 'offload-restore-day' || action === 'teardown-restore-first')"
    )
    assert js.index(catch_clear) < js.index(
        "if (action && action.startsWith('teardown-'))"
    )

    teardown_restore_first = js[
        js.index("if (action === 'teardown-restore-first')") : js.index(
            "if (action === 'cancel-restore')"
        )
    ]
    assert "offloadRestoreExpectation(totalBytes)" in teardown_restore_first
    assert "labelForPhase('restoring')" not in teardown_restore_first

    offload_restore_day = js[
        js.index("if (action === 'offload-restore-day')") : js.index(
            "if (action === 'offload-show-all-days')"
        )
    ]
    assert "candidate.day === day" in offload_restore_day
    assert (
        "offloadRestoreExpectation(entry && entry.backup_only_bytes)"
        in offload_restore_day
    )
    assert "labelForPhase('restoring')" not in offload_restore_day

    render_operation = js[
        js.index("function renderOperation()") : js.index("function renderStatus()")
    ]
    assert "operation.kind === 'offload_restore'" in render_operation
    assert "offloadRestoreReasonLabel(operation.reason_code)" in render_operation
    assert "reasonLabel(operation.reason_code)" in render_operation
    assert "error_intro" not in render_operation

    budget_gb = 37
    floor_gb = 23
    assert budget_gb != floor_gb


def test_offload_days_render_clears_offload_days_container() -> None:
    js = _backup_js()

    render_offload_days = js[
        js.index("function renderOffloadDays(days)") : js.index(
            "function renderOffload()"
        )
    ]
    assert "root.querySelector('[data-offload-days]')" in render_offload_days
    assert "target.replaceChildren();" in render_offload_days
    assert ".filter(offloadDayHasBackupOnly)" in render_offload_days
    assert render_offload_days.index(
        ".filter(offloadDayHasBackupOnly)"
    ) < render_offload_days.index(".sort((left, right)")
    assert render_offload_days.index(".sort((left, right)") < render_offload_days.index(
        "filtered.slice(0, MAX_OFFLOAD_DAY_ROWS)"
    )
    assert "offloadMessages.show_all_days" in render_offload_days
    assert "formatDisplayDay(day.day)" in render_offload_days
    assert "if (offloadDaysDegraded(days, payload)) return;" not in render_offload_days
    assert "degraded.className = 'backup-warning';" in render_offload_days

    day_predicate = js[
        js.index("function offloadDayHasBackupOnly(day)") : js.index(
            "function hasBackupOnly"
        )
    ]
    assert "day.backup_only_bytes > 0" in day_predicate
    assert "day.backup_only_segments > 0" in day_predicate
    assert "day.degraded === true" in day_predicate
    assert "backup_only_files" not in day_predicate


def test_offload_js_validates_payload_shape_before_ready_state() -> None:
    js = _backup_js()

    assert "function validOffloadPayload(payload)" in js
    assert "payload.offload &&" in js
    assert "typeof payload.offload === 'object'" in js
    assert "!Array.isArray(payload.offload)" in js
    assert "Array.isArray(payload.days)" in js
    assert "malformed backup offload status payload" in js
    guard_call = "if (!validOffloadPayload(payload))"
    assert guard_call in js
    assert js.index(guard_call) < js.index("offloadState = { status: 'ready'")


def test_teardown_js_source_contracts() -> None:
    js = _backup_js()

    assert "function backupOnlyTotalsForTeardown()" in js
    assert "if (offloadState.status !== 'ready') return null;" in js
    assert "const backupOnly = payload.backup_only;" in js
    assert "typeof backupOnly !== 'object'" in js
    assert "Array.isArray(backupOnly)" in js
    backup_only_totals = js[
        js.index("function backupOnlyTotalsForTeardown()") : js.index(
            "function renderTeardownGate"
        )
    ]
    assert "backupOnly.degraded !== false" in backup_only_totals
    assert "const days = backupOnly.total_days;" in js
    assert "const bytes = backupOnly.total_bytes;" in js
    assert "typeof days !== 'number'" in js
    assert "typeof bytes !== 'number'" in js
    assert "!Number.isFinite(days)" in js
    assert "!Number.isFinite(bytes)" in js
    assert "return { days, size: formatBytes(bytes), bytes };" in js
    assert "if (totals.days > 0)" not in js
    assert re.findall(r"startOperation\('(/app/backup/teardown)'", js) == [
        "/app/backup/teardown"
    ]
    assert "await startOperation('/app/backup/offload/restore', { all: true });" in js
    assert (
        "button.disabled = teardownInputValue() !== teardownConfirmPhrase();" not in js
    )

    confirm_satisfied = js[
        js.index("function teardownConfirmSatisfied()") : js.index(
            "function updateTeardownConfirmState"
        )
    ]
    assert "const phrase = teardownConfirmPhrase();" in confirm_satisfied
    assert "phrase !== ''" in confirm_satisfied
    assert "teardownInputValue() === phrase" in confirm_satisfied

    render_gate = js[
        js.index("function renderTeardownGate(totals)") : js.index(
            "function showTeardownGate"
        )
    ]
    assert "if (totals === null)" in render_gate
    assert "managementCopy.teardown_gate_unavailable_lead" in render_gate
    assert "managementCopy.teardown_gate_zero_lead" in render_gate
    assert "managementCopy.teardown_gate_lead" in render_gate
    assert "restoreFirst.disabled = true" in render_gate
    assert "restoreFirst.disabled = false" in render_gate

    show_gate = js[
        js.index("function showTeardownGate") : js.index(
            "function disarmTeardownConfirm"
        )
    ]
    assert "setElementHidden('[data-action=\"teardown-open\"]', true);" in show_gate

    reset_gate = js[
        js.index("function resetTeardownGate") : js.index(
            "// /app/backup/teardown remains unguarded"
        )
    ]
    assert "setElementHidden('[data-action=\"teardown-open\"]', false);" in reset_gate

    open_gate = js[
        js.index("async function openTeardownGate()") : js.index(
            "function offloadConfigBody"
        )
    ]
    assert "await refreshOffloadStatus();" in open_gate
    assert "const totals = backupOnlyTotalsForTeardown();" in open_gate
    assert "renderTeardownGate(null);" in open_gate
    assert "renderTeardownGate(totals);" in open_gate
    assert "renderOffloadUnavailable();" in open_gate

    confirm_action = js[
        js.index("if (action === 'teardown-confirm')") : js.index(
            "if (action === 'teardown-restore-first')"
        )
    ]
    guard = "if (!teardownConfirmSatisfied()) return;"
    disarm = "disarmTeardownConfirm();"
    target = "await startOperation('/app/backup/teardown');"
    assert guard in confirm_action
    assert disarm in confirm_action
    assert target in confirm_action
    guard_position = js.index(guard)
    disarm_position = js.index(disarm, guard_position)
    target_position = js.index(target)
    assert guard_position < disarm_position < target_position
