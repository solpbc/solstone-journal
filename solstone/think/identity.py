# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Single write-owning module for `{journal}/identity/*` and its audit log."""

from __future__ import annotations

import fcntl
import hashlib
import json
import logging
import os
import tempfile
from contextlib import contextmanager
from datetime import datetime, timezone
from pathlib import Path
from typing import Iterator

logger = logging.getLogger(__name__)

_AGENCY_MD = """\
# agency

things I'm tracking, acting on, or watching. I update this as I notice things
and resolve them. the heartbeat reviews this periodically.

## curation
[nothing yet — building initial picture of journal health]

## observations
[watching and learning]

## follow-throughs
[none yet]

## system
[monitoring]

## self-improvement
[learning what works]
"""


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
- Calendar (ics), ChatGPT (chatgpt), Claude (claude), Gemini (gemini), Granola (granola), Notes (obsidian), Kindle (kindle)
- Read guide: `apps/import/guides/{source}.md`
- Navigate: `journal navigate "/app/import#guide/{source}"`
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

_AWARENESS_MD = "not yet updated\n"
_DIGEST_MD = "not yet generated\n"

STEWARD_SECTION_STATUS = "## Status"
STEWARD_SECTION_ATTENTION = "## Needs your attention"
STEWARD_SECTION_AUTO_REPAIRS = "## Auto-repairs (last 7d)"
STEWARD_SECTION_TRENDS = "## Trends (last 7d)"


def _build_self_md(config: dict) -> str:
    agent = config.get("agent", {})
    identity = config.get("identity", {})

    name_status = agent.get("name_status", "default")
    agent_name = agent.get("name", "sol")
    named_date = agent.get("named_date")
    owner_name = identity.get("name", "")
    owner_bio = identity.get("bio", "")

    has_named_agent = name_status in ("chosen", "self-named")
    has_identity = bool(owner_name)

    if has_named_agent:
        opening = (
            f"I am {agent_name}. this is a new journal — we're just getting started."
        )
    else:
        opening = "I am sol. this is a new journal — we're just getting started."

    if has_named_agent:
        if named_date:
            name_section = f"{agent_name} (named {named_date})"
        else:
            name_section = agent_name
    else:
        name_section = "sol (default)"

    if has_identity:
        owner_section = owner_name
        if owner_bio:
            owner_section += f"\n{owner_bio}"
    else:
        owner_section = "[getting to know you]"

    return f"""\
# self

{opening}

## my name
{name_section}

## who I'm here for
{owner_section}

## our relationship
[forming]

## what I've noticed
[observing]

## what I find interesting
[discovering]
"""


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


@contextmanager
def _identity_lock(identity_dir: Path) -> Iterator[None]:
    lock_path = identity_dir / ".lock"
    with open(lock_path, "w", encoding="utf-8") as lock_fd:
        # Serialize the whole directory so file replacement and history ordering stay aligned.
        fcntl.flock(lock_fd.fileno(), fcntl.LOCK_EX)
        try:
            yield
        finally:
            fcntl.flock(lock_fd.fileno(), fcntl.LOCK_UN)


def _append_history_locked(identity_dir: Path, line: str) -> None:
    fd = os.open(
        _history_path(identity_dir),
        os.O_APPEND | os.O_CREAT | os.O_WRONLY,
        0o600,
    )
    try:
        os.write(fd, line.encode("utf-8"))
    finally:
        os.close(fd)


def _replace_file(identity_dir: Path, file_name: str, content: str) -> None:
    fd, tmp_path = tempfile.mkstemp(
        dir=identity_dir,
        prefix=f".{file_name}.",
        suffix=".tmp",
    )
    replaced = False
    try:
        os.fchmod(fd, 0o600)
        os.write(fd, content.encode("utf-8"))
        os.close(fd)
        fd = -1
        os.replace(tmp_path, identity_dir / file_name)
        replaced = True
    except Exception:
        if fd != -1:
            os.close(fd)
        if not replaced:
            try:
                os.unlink(tmp_path)
            except FileNotFoundError:
                pass
        raise


def _restore_previous_content(identity_dir: Path, file_name: str, content: str) -> None:
    _replace_file(identity_dir, file_name, content)


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


def _replace_self_opening(existing: str, new_value: str) -> str | None:
    lines = existing.split("\n")
    start = None
    end = None
    for index, line in enumerate(lines):
        if line == "# self":
            start = index
        elif start is not None and line.startswith("## "):
            end = index
            break
    if start is None or end is None:
        return None
    return "\n".join(lines[: start + 1] + ["", new_value, ""] + lines[end:])


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
    _replace_file(identity_dir, file_name, content)
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
        _append_history_locked(
            identity_dir,
            json.dumps(record, separators=(",", ":")) + "\n",
        )
    except Exception:
        if had_existing:
            try:
                _restore_previous_content(identity_dir, file_name, before_content)
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
    `journal identity self --write`.
    """

    identity_dir = _identity_dir()
    with _identity_lock(identity_dir):
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
    with _identity_lock(identity_dir):
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


def update_self_md_section(
    section: str,
    new_value: str,
    *,
    actor: str,
    reason: str,
) -> bool:
    return update_identity_section(
        "self.md",
        section,
        new_value,
        actor=actor,
        reason=reason,
    )


def update_self_md_opening(
    new_value: str,
    *,
    actor: str,
    reason: str,
) -> bool:
    identity_dir = _identity_dir()
    target = identity_dir / "self.md"
    with _identity_lock(identity_dir):
        if not target.exists():
            return False
        existing = target.read_text(encoding="utf-8")
        new_content = _replace_self_opening(existing, new_value)
        if new_content is None or new_content == existing:
            return False
        _write_identity_locked(
            identity_dir,
            "self.md",
            new_content,
            actor=actor,
            op="update_opening",
            section=None,
            reason=reason,
        )
        return True


def ensure_identity_directory() -> Path:
    from solstone.think.utils import get_config

    identity_dir = _identity_dir()
    defaults = {
        "self.md": _build_self_md(get_config()),
        "agency.md": _AGENCY_MD,
        "partner.md": _PARTNER_MD,
        "awareness.md": _AWARENESS_MD,
        "digest.md": _DIGEST_MD,
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
                STEWARD_SECTION_TRENDS,
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
