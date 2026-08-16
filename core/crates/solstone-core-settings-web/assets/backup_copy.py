# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Owner-facing copy constants for the backup app."""

from __future__ import annotations

from typing import Any

SERVICE_NAME = "encrypted backup"

# The journal-bound brand-lock (entry point) — the trust promise binds to the
# journal (the memory store where Article 8 binds), never to the software.
JOURNAL_BRAND_LOCK = "your journal is always private, only yours."
INTRO_SUBTITLE = (
    "make an encrypted copy of your journal somewhere safe — only you can read it."
)
INTRO_BULLETS = [
    "end-to-end encrypted",
    "optional, always",
    "delete anytime",
]
INTRO_STEPS = "you'll save a recovery key, then choose where your backup lives."
# The byo ⟷ hosted-by-sol-pbc mode selector (destination step).
MODE_BYO_TITLE = "your own"
MODE_BYO_DESC = "your bucket, your credentials. the default."
# the byo covenant beat — load-bearing ("sol pbc is never in the path").
MODE_BYO_NOTE = "sol pbc is never in the path."
MODE_HOSTED_TITLE = "operated by sol pbc"
MODE_HOSTED_DESC = "sol pbc runs the off-device part for you."
MODE_HOSTED_NOTE = "sol pbc only ever holds an encrypted copy it can't read."
MODE_HOSTED_CTA = "set up backup →"
HOSTED_SETUP_HINT = "turning this on sets up encrypted backup, operated by sol pbc — you turn it on on the services page that opens, then come back here. your journal stays on your device; only the encrypted copy goes to storage sol pbc operates, and sol pbc can never read it."
HOSTED_RESTORE_HINT = "restore the encrypted copy sol pbc keeps for you — enter your recovery key, then turn it on on the services page."
HOSTED_LOCATION_LABEL = "operated by sol pbc"
HOSTED_MANAGE_LABEL = "manage in your services →"
HOSTED_MANAGE_URL = "https://services.solstone.app/services/backup"
EDUCATE_STAKES = (
    "if you lose your recovery key, no one can recover your journal — not even sol pbc."
)
THEFT_HONESTY = "anyone with your recovery key can read everything in your backup — store it like a master password."
CONFIRM_PROMPT = "enter the recovery key you just recorded."
CONFIRM_ESCAPE = "see key again"
PM_CAUTION = "only store your recovery key in a password manager you trust. sol pbc doesn't recommend a specific one."
DESTRUCTIVE_ACTION = "turn off & delete backup"
DESTRUCTIVE_CAPTION = (
    "this deletes all your backup data. no new backups will be created."
)
TEARDOWN_GATE_LEAD = "{days} days of your journal ({size}) exist only in this backup. deleting the backup deletes them everywhere, forever."
TEARDOWN_GATE_UNAVAILABLE_LEAD = "can't verify what exists only in this backup right now. deleting the backup may destroy days of your journal that exist nowhere else."
TEARDOWN_GATE_ZERO_LEAD = (
    "nothing exists only in this backup right now. every day is still on your device."
)
TEARDOWN_CONFIRM_PHRASE = "delete"
TEARDOWN_CONFIRM_PROMPT = "type delete to confirm"
TEARDOWN_RESTORE_FIRST_ACTION = "restore everything first"
OBJECT_LOCK_WARNING = "don't enable Compliance-mode Object Lock on the bucket — it conflicts with backup pruning and lock cleanup. if you need immutability, use Governance mode."
OBJECT_LOCK_SUMMARY = "bucket setup notes"
OPTIONAL_INVARIANT = "your journal lives on your device; backup is optional."
SAVE_PASSWORD_MANAGER = "save to my password manager"
SAVE_COPY = "copy"
SAVE_CONTINUE = "continue"
CLIPBOARD_CAVEAT = (
    "copying puts your recovery key on the clipboard — clear it after you save it."
)
REPOSITORY_HINT = (
    "the restic repository for your bucket — e.g. s3:s3.amazonaws.com/your-bucket"
)
RETENTION_HINT = "how many recent copies to keep at each interval."

PHASE_LABELS = {
    "setting_up": "setting up your backup…",
    "restoring": "restoring your journal…",
    "rotating": "making a new recovery key…",
    "tearing_down": "turning off…",
    "done": "done",
    "degraded": "restored, but not verified",
    "error": "couldn't finish",
    "loading": "loading…",
    "empty": "not set up yet",
}

DESTINATION_REASON_LABELS = {
    "repo_exists": "destination is reachable and already set up.",
    "repo_missing": "destination is reachable and needs setup.",
    "auth_failed": "the destination rejected the key or credentials. check the recovery key and destination details.",
    "locked": "the destination is busy. try again shortly.",
    "timeout": "the destination took too long to respond. try again shortly.",
    "unreachable": "i couldn't reach the destination. check the repository path and try again.",
}

OPERATION_REASON_LABELS = {
    "backup_busy": "another backup task is already running. try again in a moment.",
    "backup_not_confirmed": "confirm your recovery key before turning on backup.",
    "backup_operation_failed": "i couldn't finish that backup action. check the recovery key and destination, then try again.",
    "backup_unavailable": "i couldn't ask the background service to start a backup. start it, then try again.",
    "invalid_key": "that recovery key didn't unlock the backup. re-enter the key from your saved copy.",
    "invalid_config_value": "use non-negative whole numbers, then save again.",
    "invalid_operation_for_state": "finish the current backup setup step, then try again.",
    "invalid_request_value": "check the destination details and try again.",
    "restic_unavailable": "i couldn't prepare the backup tool. try again after setup finishes.",
    "repo_missing": "i couldn't find a backup repository at that destination.",
    "auth_failed": "that recovery key didn't unlock the backup. check the key first, then the destination details.",
    "locked": "the destination is busy. try again shortly.",
    "timeout": "the destination took too long to respond. try again shortly.",
    "failed": "i couldn't finish the backup action. check the recovery key and destination, then try again.",
    "incomplete": "the backup action didn't finish. you can try again.",
    "body_rebuild_failed": "your journal was restored, but your body history couldn't be rebuilt from it. the restore wasn't finalized.",
    "integrity_failed": "your journal was restored to this device, but the backup copy failed its integrity check and may be damaged.",
    "integrity_unverified": "your journal was restored to this device, but the integrity check couldn't run (the backup was busy or timed out), so the backup copy is unverified.",
    "missing_required_field": "fill in the required fields, then try again.",
    "recovery_key_mismatch": "that didn't match your recovery key. re-enter the key from your saved copy.",
    "expired": "the approval took too long. try again.",
    "malformed": "the response couldn't be read. update your journal, then try again.",
    "network_error": "the services page couldn't be reached. check your connection, then try again.",
    "broker_unreachable": "encrypted backup couldn't be reached. check your connection, then try again.",
    "broker_error": "encrypted backup didn't return usable settings. try again shortly.",
    "hosted_entitlement_inactive": "set up backup on the services page that opens, then try again.",
}

ACTION_LABELS = {
    "start": "get started",
    "understand": "i understand",
    "save_destination": "save destination",
    "enable": "turn on backup",
    "backup_now": "back up now",
    "view_key": "view recovery key",
    "rotate_key": "regenerate recovery key",
    "teardown": DESTRUCTIVE_ACTION,
    "save_retention": "save retention",
    "restore": "restore",
    "try_again": "try again",
    "cancel": "cancel",
}

DESTINATION_FIELD_LABELS = {
    "repository": "repository",
    "backend": "backend",
    "s3": "S3",
    "b2": "B2",
    "access_key_id": "access key id",
    "secret_access_key": "secret access key",
    "b2_key_id": "key id",
    "b2_application_key": "application key",
}

RETENTION_FIELD_LABELS = {
    "hourly": "hourly",
    "daily": "daily",
    "weekly": "weekly",
    "monthly": "monthly",
}

STATUS_LABELS = {
    "last_backup": "last backup",
    "last_prune": "last prune",
    "storage_used": "storage used",
    "snapshot_history": "snapshot history",
    "not_available": "not yet available",
    "not_yet": "not yet",
    "ago": "{duration} ago",
    "enabled": "on",
    "disabled": "off",
    "destination": "where your backup lives",
    "retention": "retention",
    "setup": "set up your recovery key",
}

RESTORE_EXPECTATION = (
    "a large restore can take a while. you can leave this page open while it runs."
)
OFFLOAD_TITLE = "media offload"
OFFLOAD_STAKES = "after this, your backup holds the only copy of your older days. if you lose your recovery key, no one can recover them — not even sol pbc."
OFFLOAD_STALLED_LEAD = (
    "offload is paused: your backup isn't working. nothing has been deleted."
)
OFFLOAD_BACKUP_ONLY_LABEL = "in your backup"
OFFLOAD_RESTORE_EXPECTATION = (
    "restoring {size} from your backup — a large restore can take a while."
)
OFFLOAD_DISABLE_NOTE = (
    "this stops. days already in your backup stay there — protected and restorable."
)
OFFLOAD_UNAVAILABLE_LEAD = "can't read offload status right now."
OFFLOAD_ACTION_ERROR = (
    "media offload couldn't finish. check backup setup, then try again."
)
OFFLOAD_ENABLE_HINT = (
    "choose how much older media can leave this device after backup verification."
)
OFFLOAD_NOT_READY = (
    "turn on encrypted backup and confirm your recovery key before using media offload."
)
OFFLOAD_INVALID_LIMITS = "enter a positive number for each limit, then save again."
OFFLOAD_LABELS = {
    "budget_gb": "raw media budget",
    "floor_gb": "device free-space floor",
    "budget_short": "budget",
    "floor_short": "floor",
    "raw_media": "on this device",
    "device_free": "device free",
    "device_total": "device total",
    "last_offload": "last offload",
    "last_verify": "last verification",
    "last_restore": "last restore",
    "days": "offloaded days",
    "mb_suffix": "MB",
    "under_1mb": "under 1 MB",
    "gb_suffix": "GB",
}
OFFLOAD_ACTIONS = {
    "enable": "turn on media offload",
    "save": "save limits",
    "disable": "turn off media offload",
    "restore_day": "restore this day",
}
OFFLOAD_MESSAGES = {
    "saved": "saved",
    "empty_days": "no offloaded media yet.",
    "show_all_days": "show all {count} days",
    "degraded": "some of the record of what's in your backup couldn't be read. these days may hold more than shown.",
}
OFFLOAD_STALL_REASON_LABELS = {
    "backup_not_ready": "encrypted backup needs to finish setup before media offload can run.",
    "backup_failing": "encrypted backup needs a healthy recent copy before media offload can run.",
    "verification_missing": "backup verification needs to run before media offload can start.",
    "verification_overdue": "backup verification is overdue. media offload will wait for a fresh verification.",
    "verification_failed": "backup verification failed. media offload will wait for a healthy verification.",
    "locked": "media offload is waiting for backup maintenance to finish.",
    "archive_failed": "media offload could not add older media to encrypted backup.",
    "confirm_failed": "media offload could not verify the backed-up media.",
    "confirm_tool_failed": "media offload could not run the verification tool.",
    "unexpected_error": "media offload stopped unexpectedly. try again after backup maintenance runs.",
}
OFFLOAD_RESTORE_REASON_LABELS = {
    "auth_failed": "encrypted backup rejected the recovery key or credentials.",
    "backup_not_ready": "encrypted backup is not ready to restore media.",
    "failed": "media restore could not finish.",
    "insufficient_free_space": "this device needs more free space before restoring media.",
    "ledger_degraded": "some of the record of what's in your backup couldn't be read, so a restore can't be trusted to be complete. try again after the next backup runs.",
    "locked": "media restore is waiting for backup maintenance to finish.",
    "missing_file_after_restore": "media restore finished, but a file was still missing.",
    "nothing_to_restore": "nothing to restore for that day.",
    "repo_missing": "encrypted backup could not find the repository.",
    "restic_unavailable": "the backup tool is not available yet.",
    "rclone_unavailable": "the storage access tool is not available yet.",
    "segment_missing": "that day is no longer available locally.",
    "timeout": "media restore took too long. try again later.",
    "verification_failed": "restored media did not match the backup checksum.",
}
ERROR_INTRO = (
    "start with the recovery key. if it still fails, check the destination details."
)


def backup_copy_payload() -> dict[str, Any]:
    """Return copy constants for templates and browser code."""

    return {
        "service_name": SERVICE_NAME,
        "brand_lock": JOURNAL_BRAND_LOCK,
        "intro": {
            "title": SERVICE_NAME,
            "subtitle": INTRO_SUBTITLE,
            "bullets": list(INTRO_BULLETS),
            "optional": OPTIONAL_INVARIANT,
            "steps": INTRO_STEPS,
        },
        "educate": {
            "stakes": EDUCATE_STAKES,
        },
        "key": {
            "theft_honesty": THEFT_HONESTY,
            "pm_caution": PM_CAUTION,
            "save_password_manager": SAVE_PASSWORD_MANAGER,
            "copy_label": SAVE_COPY,
            "continue": SAVE_CONTINUE,
            "clipboard_caveat": CLIPBOARD_CAVEAT,
        },
        "confirm": {
            "prompt": CONFIRM_PROMPT,
            "escape": CONFIRM_ESCAPE,
        },
        "destination": {
            "repository_hint": REPOSITORY_HINT,
            "object_lock_warning": OBJECT_LOCK_WARNING,
            "object_lock_summary": OBJECT_LOCK_SUMMARY,
            "field_labels": dict(DESTINATION_FIELD_LABELS),
            "reason_labels": dict(DESTINATION_REASON_LABELS),
            "modes": {
                "byo": {
                    "title": MODE_BYO_TITLE,
                    "desc": MODE_BYO_DESC,
                    "note": MODE_BYO_NOTE,
                },
                "hosted": {
                    "title": MODE_HOSTED_TITLE,
                    "desc": MODE_HOSTED_DESC,
                    "note": MODE_HOSTED_NOTE,
                    "cta": MODE_HOSTED_CTA,
                },
            },
        },
        "hosted": {
            "setup_hint": HOSTED_SETUP_HINT,
            "restore_hint": HOSTED_RESTORE_HINT,
            "location_label": HOSTED_LOCATION_LABEL,
            "manage_label": HOSTED_MANAGE_LABEL,
            "manage_url": HOSTED_MANAGE_URL,
        },
        "management": {
            "destructive_action": DESTRUCTIVE_ACTION,
            "destructive_caption": DESTRUCTIVE_CAPTION,
            "teardown_gate_lead": TEARDOWN_GATE_LEAD,
            "teardown_gate_unavailable_lead": TEARDOWN_GATE_UNAVAILABLE_LEAD,
            "teardown_gate_zero_lead": TEARDOWN_GATE_ZERO_LEAD,
            "teardown_confirm_phrase": TEARDOWN_CONFIRM_PHRASE,
            "teardown_confirm_prompt": TEARDOWN_CONFIRM_PROMPT,
            "teardown_restore_first_action": TEARDOWN_RESTORE_FIRST_ACTION,
            "retention_hint": RETENTION_HINT,
            "status_labels": dict(STATUS_LABELS),
            "retention_labels": dict(RETENTION_FIELD_LABELS),
        },
        "restore": {
            "expectation": RESTORE_EXPECTATION,
        },
        "offload": {
            "title": OFFLOAD_TITLE,
            "stakes": OFFLOAD_STAKES,
            "stalled_lead": OFFLOAD_STALLED_LEAD,
            "backup_only_label": OFFLOAD_BACKUP_ONLY_LABEL,
            "restore_expectation": OFFLOAD_RESTORE_EXPECTATION,
            "disable_note": OFFLOAD_DISABLE_NOTE,
            "unavailable_lead": OFFLOAD_UNAVAILABLE_LEAD,
            "action_error": OFFLOAD_ACTION_ERROR,
            "invalid_limits": OFFLOAD_INVALID_LIMITS,
            "enable_hint": OFFLOAD_ENABLE_HINT,
            "not_ready": OFFLOAD_NOT_READY,
            "labels": dict(OFFLOAD_LABELS),
            "actions": dict(OFFLOAD_ACTIONS),
            "messages": dict(OFFLOAD_MESSAGES),
            "stall_reason_labels": dict(OFFLOAD_STALL_REASON_LABELS),
            "restore_reason_labels": dict(OFFLOAD_RESTORE_REASON_LABELS),
        },
        "phase_labels": dict(PHASE_LABELS),
        "operation_reason_labels": dict(OPERATION_REASON_LABELS),
        "action_labels": dict(ACTION_LABELS),
        "error_intro": ERROR_INTRO,
    }


def backup_copy_values() -> list[str]:
    """Return all verbatim copy values, flattening nested constants."""

    values: list[str] = []

    def visit(value: Any) -> None:
        if isinstance(value, str):
            values.append(value)
        elif isinstance(value, dict):
            for item in value.values():
                visit(item)
        elif isinstance(value, list):
            for item in value:
                visit(item)

    visit(backup_copy_payload())
    return values
