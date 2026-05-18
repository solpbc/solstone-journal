# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Shared rollup helpers for timeline aggregation."""

from __future__ import annotations

import json

# Use the full flash, not lite — qualitative ranking benefits from headroom.
MODEL = "gemini-3-flash-preview"


# Schema: a list of indices into the candidate array. We don't ask the model
# to re-emit titles/descriptions because (a) we want lossless preservation of
# the originals and (b) it's cheaper. The caller dereferences indices.
# Schema is built per-call so descriptions can reference the requested n.
def build_rollup_schema(n: int) -> dict:
    return {
        "type": "object",
        "properties": {
            "picks": {
                "type": "array",
                "description": (
                    f"Zero-based indices of the most important events from the "
                    f"candidate list, in order of importance (most important first). "
                    f"Length must be exactly {n} (or fewer if input has fewer)."
                ),
                "items": {"type": "integer"},
            },
            "rationale": {
                "type": "string",
                "description": (
                    "ONE sentence, max 100 chars, naming the criterion that "
                    "drove the pick. For debugging — not shown in the UI."
                ),
            },
        },
        "required": ["picks", "rationale"],
        "additionalProperties": False,
    }


def build_system_instruction(scope_label: str, n: int) -> str:
    return (
        f"You are picking the {n} MOST IMPORTANT events from a list of "
        f"candidate events that occurred during one {scope_label} of a "
        f"personal life-journal. The picked events become the headline cells "
        f"in the {scope_label} view of a multi-scale timeline UI.\n"
        "\n"
        "IMPORTANT-EVENT CRITERIA, in priority order:\n"
        "  1. Consequence — decisions, shipments, milestones, externally-visible "
        "actions outweigh routine maintenance.\n"
        "  2. Specificity — concrete events outweigh generic activity descriptors. "
        "'Trademark Filed' beats 'Email Sent'.\n"
        "  3. Diversity — when several candidates describe the same underlying "
        "thread (e.g., five 'KDE Crash' debugging segments), pick at most one. "
        "Reserve the other slots for distinct events.\n"
        "  4. Owner-relevance — events involving identifiable people, decisions, "
        "or commitments outweigh tooling housekeeping.\n"
        "\n"
        f'Return JSON: {{ "picks": [<indices>], "rationale": "<short>" }}.\n'
        f"  - picks: zero-based indices into the input list, in importance order, "
        f"length exactly {n} (or fewer if input has fewer).\n"
        "  - rationale: one sentence naming the criterion (for debugging).\n"
        "\n"
        "Do NOT rewrite titles or descriptions. Do NOT invent events. Pick from "
        "the given list only."
    )


def build_user_prompt(events: list[dict]) -> str:
    lines = ["Candidate events:\n"]
    for i, e in enumerate(events):
        title = e.get("title", "")
        desc = e.get("description", "")
        lines.append(f"  [{i}] {title} — {desc}")
    return "\n".join(lines)


async def pick_top_events_async(
    events: list[dict],
    n: int,
    scope_label: str = "hour",
) -> dict:
    """
    Returns {"picks": [<event dicts>], "indices": [...], "rationale": "..."}.
    Picks are dereferenced from the input list (verbatim, lossless).
    Raises on parse or model error.
    """
    if len(events) <= n:
        # Nothing to rank — return all.
        return {
            "picks": list(events),
            "indices": list(range(len(events))),
            "rationale": "fewer than N candidates; returning all",
        }

    from solstone.think.batch import Batch

    batch = Batch(max_concurrent=1)
    req = batch.create(
        contents=build_user_prompt(events),
        context="timeline.scratch.rollup",
        model=MODEL,
        system_instruction=build_system_instruction(scope_label, n),
        json_output=True,
        json_schema=build_rollup_schema(n),
        temperature=0.3,
        max_output_tokens=2048,
        timeout_s=60.0,
    )
    batch.add(req)

    async for done in batch.drain_batch():
        if done.error:
            raise RuntimeError(f"rollup model call failed: {done.error}")
        try:
            payload = json.loads(done.response)
        except (json.JSONDecodeError, TypeError) as e:
            raise RuntimeError(
                f"rollup payload parse error: {e}; response={done.response!r}"
            )
        raw_indices = payload.get("picks", [])
        # Sanitize: dedupe, in-range, cap at N.
        seen = set()
        indices = []
        for i in raw_indices:
            if isinstance(i, int) and 0 <= i < len(events) and i not in seen:
                seen.add(i)
                indices.append(i)
            if len(indices) == n:
                break
        return {
            "picks": [events[i] for i in indices],
            "indices": indices,
            "rationale": payload.get("rationale", ""),
        }

    raise RuntimeError("rollup batch drained empty")


async def pick_top_events_batch(
    jobs: list[dict],
    n: int,
    scope_label: str,
    max_concurrent: int = 5,
) -> list[dict]:
    """
    Run many rollup picks in parallel.

    Each job in `jobs` is `{"key": <opaque>, "events": [...]}`. Returns the
    same list shape as the input, with each entry augmented:
        {"key": <opaque>, "events": [...], "result": {picks, indices, rationale}}
    Jobs whose events length <= n short-circuit (no model call).
    Order is preserved.
    """
    from solstone.think.batch import Batch

    batch = Batch(max_concurrent=max_concurrent)
    handle_to_job: dict[int, dict] = {}
    out: list[dict] = []

    for i, job in enumerate(jobs):
        events = job["events"]
        rec = {"key": job.get("key"), "events": events, "result": None}
        out.append(rec)
        if len(events) <= n:
            rec["result"] = {
                "picks": list(events),
                "indices": list(range(len(events))),
                "rationale": "fewer than N candidates; returning all",
            }
            continue
        req = batch.create(
            contents=build_user_prompt(events),
            context="timeline.scratch.rollup",
            model=MODEL,
            system_instruction=build_system_instruction(scope_label, n),
            json_output=True,
            json_schema=build_rollup_schema(n),
            temperature=0.3,
            max_output_tokens=2048,
            timeout_s=60.0,
        )
        req.job_index = i
        handle_to_job[i] = rec
        batch.add(req)

    async for done in batch.drain_batch():
        rec = handle_to_job[done.job_index]
        if done.error:
            rec["result"] = {"error": done.error}
            continue
        try:
            payload = json.loads(done.response)
        except (json.JSONDecodeError, TypeError) as e:
            rec["result"] = {"error": f"parse: {e}; response={done.response!r}"}
            continue
        events = rec["events"]
        seen = set()
        indices = []
        for idx in payload.get("picks", []):
            if isinstance(idx, int) and 0 <= idx < len(events) and idx not in seen:
                seen.add(idx)
                indices.append(idx)
            if len(indices) == n:
                break
        rec["result"] = {
            "picks": [events[i] for i in indices],
            "indices": indices,
            "rationale": payload.get("rationale", ""),
        }

    return out
