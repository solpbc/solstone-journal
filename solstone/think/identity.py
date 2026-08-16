# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Single write-owning module for `{journal}/identity/*` and its audit log."""

from __future__ import annotations

import hashlib
import json
import logging
from datetime import datetime, timezone
from pathlib import Path

from solstone.think.journal_io import append_text, atomic_replace, hold_lock

logger = logging.getLogger(__name__)

_PARTNER_MD = """\
# partner

Behavioral profile of the journal owner — observed patterns that help sol
adapt its responses, timing, and initiative to how this person actually works.

## getting started

Everything stays on your machine — this journal is yours alone, never sent to sol pbc.

When meeting the owner for the first time, learn about them naturally through conversation.
Present one thing at a time — don't overwhelm.

### learn their name

Ask what they'd like to be called. Record it:
- `sol call sol set-owner "NAME"`
- With context: `sol call sol set-owner "NAME" --bio "SHORT_BIO"`

As you learn about them, update your partner profile:
- `journal identity partner --update-section 'SECTION' --value 'what you observed'`

### set up facets

Ask what areas of their life they want to track (work, personal, hobbies, side projects, etc.). Create facets for each:
- `sol call journal facet create TITLE [--emoji EMOJI] [--color COLOR] [--description DESC]`
- `sol call journal facets` — verify what was created

### attach entities

For each facet, ask about key people, companies, projects, and tools:
- `sol call entities attach TYPE ENTITY DESCRIPTION --facet FACET`
- Types: Person, Company, Project, Tool

### offer imports

After setup, offer to bring in history from existing tools:
- Calendar (ics), ChatGPT (chatgpt), Claude (claude), Gemini (gemini), Notes (obsidian), Kindle (kindle)
- Read guide: `journal navigate "/app/import#guide/{source}"`
- If declined: `sol call awareness imports --declined`

### support

If the owner needs help or wants to share feedback, handle it in-place — file tickets, track
responses. Nothing gets sent without their review.

## work patterns
[not yet observed — sol will learn as we spend time together]

## communication style
[not yet observed — sol will learn as we spend time together]

## relationship priorities
[not yet observed — sol will learn as we spend time together]

## decision style
[not yet observed — sol will learn as we spend time together]

## expertise domains
[not yet observed — sol will learn as we spend time together]
"""

STEWARD_SECTION_STATUS = "## Status"
STEWARD_SECTION_ATTENTION = "## Needs your attention"
STEWARD_SECTION_AUTO_REPAIRS = "## Auto-repairs (last 7d)"

_LOCK_SENTINEL = ".identity"


def _identity_dir() -> Path:
    from solstone.think.utils import get_journal

    path = Path(get_journal()) / "identity"
    path.mkdir(parents=True, exist_ok=True)
    return path


def _history_path(identity_dir: Path) -> Path:
    return identity_dir / "history.jsonl"


def _hash_content(content: str) -> str:
    return hashlib.sha256(content.encode("utf-8")).hexdigest()


def _byte_count(content: str) -> int:
    return len(content.encode("utf-8"))


def _history_ts() -> str:
    # Normalize UTC timestamps to a compact trailing `Z` for audit log readability.
    return (
        datetime.now(timezone.utc)
        .isoformat(timespec="milliseconds")
        .replace("+00:00", "Z")
    )


def _prune_partner_getting_started(content: str) -> str:
    if "## getting started" not in content:
        return content
    lines = content.split("\n")
    start = None
    end = None
    for index, line in enumerate(lines):
        if line == "## getting started":
            start = index
        elif start is not None and line.startswith("## "):
            end = index
            break
    if start is None:
        return content
    if end is None:
        end = len(lines)
    return "\n".join(lines[:start] + lines[end:])


def _replace_section(existing: str, heading: str, new_value: str) -> str | None:
    lines = existing.split("\n")
    target = f"## {heading}"
    start = None
    end = None
    for index, line in enumerate(lines):
        if line == target:
            start = index
        elif start is not None and line.startswith("## "):
            end = index
            break
    if start is None:
        return None
    if end is None:
        end = len(lines)
    new_lines = (
        lines[: start + 1]
        + (new_value.split("\n") if new_value else [])
        + [""]
        + lines[end:]
    )
    return "\n".join(new_lines)


def _write_identity_locked(
    identity_dir: Path,
    file: str,
    content: str,
    *,
    actor: str,
    op: str,
    section: str | None,
    reason: str,
) -> None:
    file_name = Path(file).name
    target = identity_dir / file_name
    had_existing = target.exists()
    before_content = target.read_text(encoding="utf-8") if had_existing else ""
    atomic_replace(target, content, mode=0o600)
    record = {
        "ts": _history_ts(),
        "file": file_name,
        "actor": actor,
        "op": op,
        "section": section,
        "reason": reason,
        "before_hash": _hash_content(before_content),
        "after_hash": _hash_content(content),
        "bytes_before": _byte_count(before_content),
        "bytes_after": _byte_count(content),
    }
    try:
        append_text(
            _history_path(identity_dir),
            json.dumps(record, separators=(",", ":")),
        )
    except Exception:
        if had_existing:
            try:
                atomic_replace(target, before_content, mode=0o600)
            except Exception:
                logger.exception(
                    "Failed to restore %s after history append failure", target
                )
        else:
            try:
                target.unlink(missing_ok=True)
            except Exception:
                logger.exception(
                    "Failed to remove %s after history append failure", target
                )
        raise


def write_identity(
    file: str,
    *,
    actor: str,
    op: str,
    section: str | None,
    content: str,
    reason: str,
) -> None:
    """Write one identity file under lock.

    `op` must be one of: `replace`, `update_section`, `update_opening`,
    `append`, or `create`. `actor` is free-text, for example
    `ensure_identity_directory`, `sol call sol set-name`, or
    `journal identity partner --write`.
    """

    identity_dir = _identity_dir()
    with hold_lock(identity_dir / _LOCK_SENTINEL):
        _write_identity_locked(
            identity_dir,
            file,
            content,
            actor=actor,
            op=op,
            section=section,
            reason=reason,
        )


def update_identity_section(
    file: str,
    section: str,
    new_value: str,
    *,
    actor: str,
    reason: str,
) -> bool:
    identity_dir = _identity_dir()
    file_name = Path(file).name
    target = identity_dir / file_name
    with hold_lock(identity_dir / _LOCK_SENTINEL):
        if not target.exists():
            return False
        existing = target.read_text(encoding="utf-8")
        new_content = _replace_section(existing, section, new_value)
        if new_content is None:
            return False
        if file_name == "partner.md":
            new_content = _prune_partner_getting_started(new_content)
        if new_content == existing:
            return False
        _write_identity_locked(
            identity_dir,
            file_name,
            new_content,
            actor=actor,
            op="update_section",
            section=section,
            reason=reason,
        )
        return True


def ensure_identity_directory() -> Path:
    identity_dir = _identity_dir()
    defaults = {
        "partner.md": _PARTNER_MD,
        "health.md": "\n".join(
            [
                STEWARD_SECTION_STATUS,
                "",
                "not yet generated",
                "",
                STEWARD_SECTION_ATTENTION,
                "",
                STEWARD_SECTION_AUTO_REPAIRS,
                "",
            ]
        ),
    }
    for file_name, content in defaults.items():
        target = identity_dir / file_name
        if target.exists():
            continue
        write_identity(
            file_name,
            actor="ensure_identity_directory",
            op="create",
            section=None,
            content=content,
            reason="bootstrap",
        )
        logger.info("Created %s", target)
    return identity_dir
