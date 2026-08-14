# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Tests for backup app copy discipline."""

from __future__ import annotations

import json
import re
from pathlib import Path

from solstone.apps.backup.copy import (
    TEARDOWN_CONFIRM_PHRASE,
    TEARDOWN_CONFIRM_PROMPT,
    backup_copy_payload,
    backup_copy_values,
)
from solstone.think.offload import OFFLOAD_STALL_REASONS
from solstone.think.offload_restore import OFFLOAD_RESTORE_REASONS


def _backup_js_text() -> str:
    return Path("core/crates/solstone-core-backup-web/assets/backup.js").read_text(encoding="utf-8")


def _backup_copy_literal() -> dict:
    text = _backup_js_text()
    prefix = "  const BACKUP_COPY = "
    start = text.index(prefix) + len(prefix)
    payload, end = json.JSONDecoder().raw_decode(text[start:])
    assert text[start + end :].lstrip().startswith(";")
    return payload


def test_backup_copy_verbatim_strings() -> None:
    payload = backup_copy_payload()
    static = _backup_copy_literal()
    offload = payload["offload"]

    assert payload["service_name"] == "encrypted backup"
    assert payload["intro"]["title"] == "encrypted backup"
    # the journal-bound brand-lock — load-bearing trust beat (CSO-required)
    assert payload["brand_lock"] == "your journal is always private, only yours."
    assert (
        payload["intro"]["subtitle"]
        == "make an encrypted copy of your journal somewhere safe — only you can read it."
    )
    assert payload["intro"]["bullets"] == [
        "end-to-end encrypted",
        "optional, always",
        "delete anytime",
    ]
    # load-bearing honesty beats — must survive verbatim (CSO-required)
    assert (
        payload["educate"]["stakes"]
        == "if you lose your recovery key, no one can recover your journal — not even sol pbc."
    )
    assert (
        payload["key"]["theft_honesty"]
        == "anyone with your recovery key can read everything in your backup — store it like a master password."
    )
    assert payload["confirm"]["prompt"] == "enter the recovery key you just recorded."
    assert payload["confirm"]["escape"] == "see key again"
    assert (
        payload["key"]["pm_caution"]
        == "only store your recovery key in a password manager you trust. sol pbc doesn't recommend a specific one."
    )
    assert payload["management"]["destructive_action"] == "turn off & delete backup"
    assert (
        payload["management"]["destructive_caption"]
        == "this deletes all your backup data. no new backups will be created."
    )
    assert (
        payload["management"]["teardown_gate_lead"]
        == "{days} days of your journal ({size}) exist only in this backup. deleting the backup deletes them everywhere, forever."
    )
    assert "{days}" in payload["management"]["teardown_gate_lead"]
    assert "{size}" in payload["management"]["teardown_gate_lead"]
    assert (
        payload["management"]["teardown_gate_unavailable_lead"]
        == "can't verify what exists only in this backup right now. deleting the backup may destroy days of your journal that exist nowhere else."
    )
    assert (
        payload["management"]["teardown_gate_zero_lead"]
        == "nothing exists only in this backup right now. every day is still on your device."
    )
    assert payload["management"]["teardown_confirm_phrase"] == TEARDOWN_CONFIRM_PHRASE
    assert payload["management"]["teardown_confirm_prompt"] == TEARDOWN_CONFIRM_PROMPT
    assert static["management"]["teardown_confirm_phrase"] == TEARDOWN_CONFIRM_PHRASE
    assert static["management"]["teardown_confirm_prompt"] == TEARDOWN_CONFIRM_PROMPT
    assert payload["management"]["status_labels"]["ago"] == "{duration} ago"
    assert (
        payload["management"]["teardown_restore_first_action"]
        == "restore everything first"
    )
    # the byo covenant beat — "sol pbc is never in the path" (mode selector)
    assert (
        payload["destination"]["modes"]["byo"]["note"]
        == "sol pbc is never in the path."
    )
    assert payload["destination"]["modes"]["byo"]["title"] == "your own"
    assert payload["destination"]["modes"]["hosted"]["title"] == "operated by sol pbc"
    assert (
        payload["destination"]["modes"]["hosted"]["note"]
        == "sol pbc only ever holds an encrypted copy it can't read."
    )
    assert payload["destination"]["modes"]["hosted"]["cta"] == "set up backup →"
    assert (
        payload["destination"]["object_lock_warning"]
        == "don't enable Compliance-mode Object Lock on the bucket — it conflicts with backup pruning and lock cleanup. if you need immutability, use Governance mode."
    )
    assert (
        payload["intro"]["optional"]
        == "your journal lives on your device; backup is optional."
    )
    assert payload["key"]["save_password_manager"] == "save to my password manager"
    assert payload["key"]["copy_label"] == "copy"
    assert payload["key"]["continue"] == "continue"
    assert payload["destination"]["field_labels"]["b2_key_id"] == "key id"
    assert (
        payload["destination"]["field_labels"]["b2_application_key"]
        == "application key"
    )
    assert (
        offload["stakes"]
        == "after this, your backup holds the only copy of your older days. if you lose your recovery key, no one can recover them — not even sol pbc."
    )
    assert (
        offload["stalled_lead"]
        == "offload is paused: your backup isn't working. nothing has been deleted."
    )
    assert offload["backup_only_label"] == "in your backup"
    assert (
        offload["restore_expectation"]
        == "restoring {size} from your backup — a large restore can take a while."
    )
    assert "{size}" in offload["restore_expectation"]
    assert (
        offload["disable_note"]
        == "this stops. days already in your backup stay there — protected and restorable."
    )
    assert offload["unavailable_lead"] == "can't read offload status right now."
    assert (
        offload["invalid_limits"]
        == "enter a positive number for each limit, then save again."
    )
    assert offload["labels"]["mb_suffix"] == "MB"
    assert offload["labels"]["under_1mb"] == "under 1 MB"
    assert offload["labels"]["budget_short"] == "budget"
    assert offload["labels"]["floor_short"] == "floor"
    assert offload["labels"]["days"] == "offloaded days"
    assert offload["messages"]["show_all_days"] == "show all {count} days"


def test_offload_reason_copy_covers_closed_vocabularies() -> None:
    offload = backup_copy_payload()["offload"]
    stall = offload["stall_reason_labels"]
    restore = offload["restore_reason_labels"]

    assert set(stall) == set(OFFLOAD_STALL_REASONS)
    assert set(restore) == set(OFFLOAD_RESTORE_REASONS)
    for reason in OFFLOAD_STALL_REASONS:
        assert stall[reason]
    for reason in OFFLOAD_RESTORE_REASONS:
        assert restore[reason]
    assert stall["locked"] != offload["stalled_lead"]


def test_no_literal_copy_in_workspace_template() -> None:
    root = Path("core/crates/solstone-core-backup-web/assets")
    structural_values = {
        "B2",
        "S3",
        "Copy",
        "Restore",
        "done",
        "couldn't finish",
        "loading…",
        "not yet",
        "not yet available",
        "off",
        "on",
        # lowercased labels that coincide with structural code tokens
        # (form field names / panel + route names in backup.js), not display leaks
        "backend",
        "repository",
        "restore",
    }
    hits: list[tuple[Path, str]] = []
    path = root / "workspace.html"
    text = path.read_text(encoding="utf-8")
    for value in backup_copy_values():
        if not value or value in structural_values:
            continue
        literal_patterns = (
            re.compile(rf">\s*{re.escape(value)}\s*<"),
            re.compile(rf"(?<!=)['\"`]{re.escape(value)}['\"`]"),
        )
        if any(pattern.search(text) for pattern in literal_patterns):
            hits.append((path, value))

    assert hits == []


def test_all_copy_constants_referenced_by_render_surface() -> None:
    html = Path("core/crates/solstone-core-backup-web/assets/workspace.html").read_text(encoding="utf-8")
    static = Path("core/crates/solstone-core-backup-web/assets/backup.js").read_text(encoding="utf-8")
    surface = html + "\n" + static

    missing = [
        key
        for key in (
            "intro",
            "educate",
            "key",
            "confirm",
            "destination",
            "hosted",
            "management",
            "restore",
            "offload",
            "phase_labels",
            "operation_reason_labels",
            "action_labels",
            "error_intro",
        )
        if key not in surface
    ]

    assert missing == []
