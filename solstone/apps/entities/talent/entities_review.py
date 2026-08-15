# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import json
import logging
from datetime import datetime, timedelta
from itertools import combinations
from typing import Any

from solstone.think.entities import (
    AkaConflictError,
    EntityBlockedError,
    EntityExistsError,
    EntityNotFoundError,
    EntityWriteError,
    add_entity_aka,
    attach_or_reactivate_entity,
    detected_entities_path,
    entity_slug,
    find_matching_entity,
    is_name_variant_match,
    load_entities,
)
from solstone.think.entities.review_candidates import (
    load_candidates,
    record_merge_candidate,
)
from solstone.think.journal_io import LockTimeout
from solstone.think.utils import now_ms

logger = logging.getLogger(__name__)

REVIEW_WINDOW_DAYS = 7
TYPE_THRESHOLDS = {"Person": 2, "Company": 3, "Project": 3, "Tool": 5}
DEFAULT_THRESHOLD = 5


def _format_name_key(name: str) -> tuple[str, str]:
    return (name.casefold(), name)


def _review_days(day: str) -> list[str]:
    run_day = datetime.strptime(day, "%Y%m%d")
    # Offset 1 excludes the run day; a later review can flip this to 0.
    start_offset = 1
    return [
        (run_day - timedelta(days=i)).strftime("%Y%m%d")
        for i in range(REVIEW_WINDOW_DAYS + start_offset - 1, start_offset - 1, -1)
    ]


def _find_display_name(contexts: list[dict[str, str]]) -> str:
    latest_day = max(context["day"] for context in contexts)
    latest_names = [
        context["name"] for context in contexts if context["day"] == latest_day
    ]
    return sorted(latest_names, key=_format_name_key)[0]


def _format_contexts(contexts: list[dict[str, str]]) -> list[dict[str, str]]:
    return sorted(
        contexts,
        key=lambda context: (
            context["day"],
            context["name"].casefold(),
            context["name"],
            context["description"],
        ),
    )


def _compute_variant_hints(names: set[str]) -> list[tuple[str, str]]:
    hints: set[tuple[str, str]] = set()
    for name_a, name_b in combinations(sorted(names, key=_format_name_key), 2):
        slug_a = entity_slug(name_a)
        slug_b = entity_slug(name_b)
        if slug_a == slug_b:
            continue
        if not is_name_variant_match(name_a, name_b):
            continue
        hints.add(tuple(sorted((name_a, name_b), key=_format_name_key)))
    return sorted(
        hints, key=lambda pair: (_format_name_key(pair[0]), _format_name_key(pair[1]))
    )


def _load_prior_merges(facet: str) -> list[dict[str, str]]:
    prior: list[dict[str, str]] = []
    for row in load_candidates():
        if row.get("facet") != facet:
            continue
        source = row.get("source")
        target = row.get("target")
        status = row.get("status")
        if not isinstance(source, str) or not isinstance(target, str):
            continue
        prior.append(
            {
                "source": source,
                "canonical": target,
                "status": str(status or "open"),
            }
        )
    return sorted(
        prior,
        key=lambda row: (
            row["source"].casefold(),
            row["canonical"].casefold(),
            row["status"],
        ),
    )


def build_review_inputs(
    facet: str,
    day: str,
) -> tuple[list[dict[str, Any]], list[tuple[str, str]], list[dict[str, str]]]:
    buckets: dict[str, dict[str, Any]] = {}
    detected_names: set[str] = set()

    for review_day in _review_days(day):
        for entity in load_entities(facet, review_day):
            name = str(entity.get("name") or "").strip()
            entity_type = str(entity.get("type") or "").strip()
            description = str(entity.get("description") or "").strip()
            slug = entity_slug(name)
            if not name or not slug or not entity_type:
                continue

            bucket = buckets.setdefault(
                slug,
                {"days": set(), "types": set(), "contexts": []},
            )
            bucket["days"].add(review_day)
            bucket["types"].add(entity_type)
            bucket["contexts"].append(
                {
                    "day": review_day,
                    "name": name,
                    "description": description,
                }
            )
            detected_names.add(name)

    attached = load_entities(facet)
    eligible: list[dict[str, Any]] = []
    for slug, bucket in buckets.items():
        types = bucket["types"]
        if len(types) != 1:
            continue
        entity_type = next(iter(types))
        day_count = len(bucket["days"])
        if day_count < TYPE_THRESHOLDS.get(entity_type, DEFAULT_THRESHOLD):
            continue

        contexts = _format_contexts(bucket["contexts"])
        name = _find_display_name(contexts)
        match = find_matching_entity(name, attached)
        if match and match.is_high_confidence:
            continue

        eligible.append(
            {
                "name": name,
                "slug": slug,
                "type": entity_type,
                "day_count": day_count,
                "contexts": contexts,
            }
        )

    eligible.sort(
        key=lambda item: (_format_name_key(str(item["name"])), str(item["type"]))
    )
    return eligible, _compute_variant_hints(detected_names), _load_prior_merges(facet)


def _format_review_packet(
    eligible: list[dict[str, Any]],
    variant_hints: list[tuple[str, str]],
    prior_merges: list[dict[str, str]],
) -> str:
    lines = [
        "These are recurring people and things noticed across recent days in one area of the owner's life.",
        "Judge from the facts below. Save stable, useful context; leave ambiguity out.",
        "",
        "## Recurring candidates",
        "",
    ]

    if eligible:
        for item in eligible:
            lines.extend(
                [
                    f"### {item['name']}",
                    f"Type: {item['type']}",
                    f"Distinct days seen: {item['day_count']}",
                    "What happened:",
                ]
            )
            for context in item["contexts"]:
                description = context["description"] or "No description saved."
                lines.append(f"- {context['day']}: {context['name']} — {description}")
            lines.append("")
    else:
        lines.extend(["None.", ""])

    lines.extend(["## Possible name variants", ""])
    if variant_hints:
        for name_a, name_b in variant_hints:
            lines.append(f"- {name_a} / {name_b}")
    else:
        lines.append("None.")
    lines.append("")

    lines.extend(["## Prior merge decisions", ""])
    if prior_merges:
        for row in prior_merges:
            lines.append(f"- {row['source']} -> {row['canonical']} ({row['status']})")
    else:
        lines.append("None.")

    return "\n".join(lines).strip() + "\n"


def pre_process(context: dict) -> dict | None:
    day = context.get("day")
    if not day:
        return {"skip_reason": "no_day"}
    facet = context.get("facet")
    if not facet:
        return {"skip_reason": "no_facet"}

    eligible, variant_hints, prior_merges = build_review_inputs(str(facet), str(day))
    if not eligible and not variant_hints:
        return {"skip_reason": "no_candidates"}

    return {
        "template_vars": {
            "review_packet": _format_review_packet(
                eligible,
                variant_hints,
                prior_merges,
            )
        }
    }


def _write_outcome(
    facet: str,
    day: str,
    counts: dict[str, int],
    error: str | None,
) -> None:
    payload = {**counts, "error": error, "ts": now_ms()}
    out = detected_entities_path(facet, day).parent / f"{day}_review_outcome.json"
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(json.dumps(payload, ensure_ascii=False) + "\n", encoding="utf-8")


def _apply_aliases(
    *,
    facet: str,
    entity_id: str,
    canonical_name: str,
    canonical_slug: str,
    aliases: list,
    counts: dict[str, int],
) -> str | None:
    error: str | None = None
    for alias in aliases:
        if not isinstance(alias, str) or not alias.strip():
            counts["skipped"] += 1
            continue
        clean_alias = alias.strip()
        if entity_slug(clean_alias) == canonical_slug:
            counts["skipped"] += 1
            continue
        try:
            add_entity_aka(facet, entity_id, clean_alias)
            counts["aliased"] += 1
        except AkaConflictError:
            counts["skipped"] += 1
        except EntityNotFoundError as exc:
            counts["errored"] += 1
            error = f"{type(exc).__name__}: {exc}"
        except LockTimeout as exc:
            counts["errored"] += 1
            error = f"{type(exc).__name__}: {exc}"
            break
        except EntityWriteError as exc:
            counts["errored"] += 1
            error = f"{type(exc).__name__}: {exc}"
    return error


def _apply_promotions(
    *,
    facet: str,
    raw_promotions: list,
    eligible_by_slug: dict[str, dict[str, Any]],
    counts: dict[str, int],
) -> str | None:
    error: str | None = None
    for row in raw_promotions:
        if not isinstance(row, dict):
            counts["skipped"] += 1
            continue
        name = row.get("name")
        description = row.get("description")
        promote = row.get("promote")
        aliases = row.get("aliases")
        if (
            not isinstance(name, str)
            or not name.strip()
            or not isinstance(description, str)
            or not description.strip()
            or not isinstance(promote, bool)
            or not isinstance(aliases, list)
        ):
            counts["skipped"] += 1
            continue

        slug = entity_slug(name.strip())
        eligible = eligible_by_slug.get(slug)
        if eligible is None:
            counts["skipped"] += 1
            continue
        if promote is not True:
            counts["skipped"] += 1
            continue

        canonical_name = str(eligible["name"])
        entity_id: str | None = None
        try:
            relationship, _ = attach_or_reactivate_entity(
                facet,
                entity_type=str(eligible["type"]),
                name=canonical_name,
                description=description.strip(),
            )
            counts["promoted"] += 1
            entity_id = str(relationship["entity_id"])
        except EntityExistsError:
            counts["skipped"] += 1
            entity_id = entity_slug(canonical_name)
        except EntityBlockedError:
            counts["skipped"] += 1
            continue
        except (EntityNotFoundError, LockTimeout, EntityWriteError) as exc:
            counts["errored"] += 1
            error = f"{type(exc).__name__}: {exc}"
            continue

        alias_error = _apply_aliases(
            facet=facet,
            entity_id=entity_id,
            canonical_name=canonical_name,
            canonical_slug=str(eligible["slug"]),
            aliases=aliases,
            counts=counts,
        )
        if alias_error is not None:
            error = alias_error
    return error


def _apply_merges(
    *,
    facet: str,
    day: str,
    raw_merges: list,
    hint_slug_pairs: set[frozenset[str]],
    counts: dict[str, int],
) -> str | None:
    error: str | None = None
    for row in raw_merges:
        if not isinstance(row, dict):
            counts["skipped"] += 1
            continue
        source = row.get("source")
        canonical = row.get("canonical")
        evidence = row.get("evidence")
        if (
            not isinstance(source, str)
            or not source.strip()
            or not isinstance(canonical, str)
            or not canonical.strip()
            or not isinstance(evidence, str)
            or not evidence.strip()
        ):
            counts["skipped"] += 1
            continue

        clean_source = source.strip()
        clean_canonical = canonical.strip()
        source_slug = entity_slug(clean_source)
        canonical_slug = entity_slug(clean_canonical)
        if source_slug == canonical_slug:
            counts["skipped"] += 1
            continue
        if frozenset((source_slug, canonical_slug)) not in hint_slug_pairs:
            counts["skipped"] += 1
            continue

        try:
            record_merge_candidate(
                facet=facet,
                day=day,
                source=clean_source,
                source_slug=source_slug,
                target=clean_canonical,
                target_slug=canonical_slug,
                evidence=evidence.strip(),
                basis="name-variant",
            )
            counts["merges"] += 1
        except Exception as exc:
            counts["errored"] += 1
            error = f"{type(exc).__name__}: {exc}"
    return error


def post_process(result: str, context: dict) -> None:
    counts = {"promoted": 0, "aliased": 0, "merges": 0, "skipped": 0, "errored": 0}
    error: str | None = None
    facet = context.get("facet")
    day = context.get("day")
    if not facet or not day:
        return None

    facet = str(facet)
    day = str(day)

    try:
        eligible, variant_hints, _ = build_review_inputs(facet, day)
        eligible_by_slug = {str(item["slug"]): item for item in eligible}
        hint_slug_pairs = {
            frozenset((entity_slug(name_a), entity_slug(name_b)))
            for name_a, name_b in variant_hints
        }

        try:
            data = json.loads(result)
        except json.JSONDecodeError:
            logger.warning("entities_review post-hook received invalid JSON")
            return None

        if not isinstance(data, dict):
            logger.warning("entities_review post-hook result is not a JSON object")
            return None
        raw_promotions = data.get("promotions")
        raw_merges = data.get("merges")
        if not isinstance(raw_promotions, list) or not isinstance(raw_merges, list):
            logger.warning("entities_review post-hook result missing expected arrays")
            return None

        promotion_error = _apply_promotions(
            facet=facet,
            raw_promotions=raw_promotions,
            eligible_by_slug=eligible_by_slug,
            counts=counts,
        )
        if promotion_error is not None:
            error = promotion_error

        merge_error = _apply_merges(
            facet=facet,
            day=day,
            raw_merges=raw_merges,
            hint_slug_pairs=hint_slug_pairs,
            counts=counts,
        )
        if merge_error is not None:
            error = merge_error
    except Exception as exc:
        counts["errored"] += 1
        error = f"{type(exc).__name__}: {exc}"
        logger.warning("entities_review post-hook failed: %s", exc)
    finally:
        try:
            _write_outcome(facet, day, counts, error)
        except Exception as exc:
            logger.warning("entities_review outcome write failed: %s", exc)

    return None
