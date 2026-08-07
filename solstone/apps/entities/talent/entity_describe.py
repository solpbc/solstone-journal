# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Pre-hook for the entity description generate talent."""

from __future__ import annotations

import logging

from solstone.think.indexer.journal import search_journal

logger = logging.getLogger(__name__)

_NO_EVIDENCE = "No journal evidence found for this entity."


def pre_process(config: dict) -> dict | None:
    """Parse entity context and attach bounded journal evidence."""
    fields = _parse_prompt(str(config.get("prompt") or ""))
    entity_name = fields["entity_name"]
    if not entity_name:
        return {"skip_reason": "missing entity name"}

    evidence = _render_evidence(entity_name, fields["facet"])
    return {
        "template_vars": {
            "entity_type": fields["entity_type"] or "Entity",
            "entity_name": entity_name,
            "facet": fields["facet"] or "(none)",
            "current_description": fields["current_description"] or "(none)",
            "evidence": evidence,
        }
    }


def _parse_prompt(prompt: str) -> dict[str, str]:
    fields = {
        "entity_type": "",
        "entity_name": "",
        "facet": "",
        "current_description": "",
    }
    prefixes = {
        "Entity Type:": "entity_type",
        "Entity Name:": "entity_name",
        "Facet:": "facet",
        "Current Description:": "current_description",
    }
    for line in prompt.splitlines():
        for prefix, key in prefixes.items():
            if line.startswith(prefix):
                fields[key] = line[len(prefix) :].strip()
                break
    if fields["current_description"] == "(none)":
        fields["current_description"] = ""
    return fields


def _render_evidence(entity_name: str, facet: str) -> str:
    try:
        _, results = search_journal(
            entity_name,
            limit=5,
            facet=facet or None,
            include_total=False,
        )
    except Exception as exc:
        logger.warning("entity_describe evidence search unavailable: %s", exc)
        return f"Journal evidence unavailable: {exc}"

    if not results:
        return _NO_EVIDENCE

    lines = []
    for result in results:
        metadata = result.get("metadata") or {}
        source_id = str(result.get("id") or "")
        day = str(metadata.get("day") or "unknown")
        result_facet = str(metadata.get("facet") or "unknown")
        text = _single_line(str(result.get("text") or ""))
        lines.append(f"- {source_id} [{day}, {result_facet}]: {text}")
    return "\n".join(lines)


def _single_line(value: str) -> str:
    return " ".join(value.strip().split())
