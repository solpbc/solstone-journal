# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Strict journal-entity merge and recorded-merge undo."""

from __future__ import annotations

import base64
import copy
import json
import logging
import shutil
import sqlite3
import uuid
from pathlib import Path
from typing import Any

from solstone.think.entities.history import (
    EntityHistoryError,
    EntityOperationContext,
    find_entity_merge_payload,
    iter_entity_history,
    load_entity_merge_payloads,
    move_entity_merge_payload,
    record_entity_merge_payload,
    remove_entity_merge_payload,
    trust_operation_lock,
)
from solstone.think.entities.journal import (
    save_journal_entity,
    scan_journal_entities,
)
from solstone.think.entities.observations import save_observations
from solstone.think.entities.relationships import (
    save_facet_relationship,
)
from solstone.think.entities.voiceprints import (
    load_existing_voiceprint_keys,
    normalize_embedding,
    save_voiceprints_batch,
)
from solstone.think.journal_io import (
    MalformedPolicy,
    append_jsonl,
    atomic_replace,
    contained_path,
    read_json,
    read_jsonl,
)
from solstone.think.journal_io.npz import load_npz
from solstone.think.utils import day_dirs, get_journal, iter_segments, now_ms

logger = logging.getLogger(__name__)

MISSING_SENTINEL = {"missing": True}
IDENTITY_SCALAR_EXCLUDES = {
    "id",
    "name",
    "aka",
    "emails",
    "created_at",
    "updated_at",
    "merged_into",
    "blocked",
    "is_principal",
}
FACET_SCALAR_FIELDS = ("attached_at", "updated_at", "last_seen", "description")


class MergePreflightError(RuntimeError):
    """Raised when strict merge preflight rejects malformed journal input."""


class MergeRepairRequired(RuntimeError):
    """Raised when rollback itself fails and operator repair is required."""


def _dedupe_akas(target_values: list[Any], source_values: list[Any]) -> list[str]:
    """Case-insensitive aka dedup, preserving first-seen spelling."""
    aka_by_lower: dict[str, str] = {}
    for values in (target_values, source_values):
        if not isinstance(values, list):
            continue
        for value in values:
            if not value:
                continue
            key = str(value).lower()
            if key not in aka_by_lower:
                aka_by_lower[key] = str(value)
    return sorted(aka_by_lower.values(), key=str.lower)


def _dedupe_emails(target_values: list[Any], source_values: list[Any]) -> list[str]:
    """Case-insensitive email dedup, preserving first-seen order/spelling."""
    merged_emails: list[str] = []
    seen_emails: set[str] = set()
    for values in (target_values, source_values):
        if not isinstance(values, list):
            continue
        for value in values:
            if not value:
                continue
            email = str(value)
            key = email.lower()
            if key in seen_emails:
                continue
            seen_emails.add(key)
            merged_emails.append(email)
    return merged_emails


def _dedupe_observations(
    source_observations: list[dict[str, Any]],
    target_observations: list[dict[str, Any]],
) -> list[dict[str, Any]]:
    """Deduplicate observations on (content, observed_at)."""
    seen = {
        (item.get("content", ""), item.get("observed_at"))
        for item in target_observations
    }
    merged_observations = list(target_observations)
    for item in source_observations:
        key = (item.get("content", ""), item.get("observed_at"))
        if key in seen:
            continue
        seen.add(key)
        merged_observations.append(item)
    return merged_observations


def _identity_section(
    akas_added: list[str],
    emails_added: list[str],
    principal_transferred: bool,
) -> dict[str, Any]:
    return {
        "akas_added": akas_added,
        "akas_added_count": len(akas_added),
        "emails_added": emails_added,
        "emails_added_count": len(emails_added),
        "principal_transferred": principal_transferred,
    }


def _voiceprint_section(
    added: int, skipped_duplicate: int, target_total: int
) -> dict[str, Any]:
    return {
        "added": added,
        "skipped_duplicate": skipped_duplicate,
        "target_total": target_total,
    }


def _facet_section(
    moved: list[str],
    merged: list[str],
    observations_appended: int,
    observation_relations_rewritten: int,
) -> dict[str, Any]:
    return {
        "moved": moved,
        "moved_count": len(moved),
        "merged": merged,
        "merged_count": len(merged),
        "observations_appended": observations_appended,
        "observation_relations_rewritten": observation_relations_rewritten,
    }


def _segment_section(
    labels_rewritten: int,
    corrections_rewritten: int,
    files_scanned: int,
) -> dict[str, Any]:
    return {
        "labels_rewritten": labels_rewritten,
        "corrections_rewritten": corrections_rewritten,
        "files_scanned": files_scanned,
        "errors": [],
    }


def _activity_section(
    records_rewritten: int = 0,
    fields_rewritten: int = 0,
    files_scanned: int = 0,
    files_rewritten: int = 0,
) -> dict[str, Any]:
    return {
        "records_rewritten": records_rewritten,
        "fields_rewritten": fields_rewritten,
        "files_scanned": files_scanned,
        "files_rewritten": files_rewritten,
        "errors": [],
    }


def _edges_section(rows_folded: int = 0, self_edges_dropped: int = 0) -> dict[str, Any]:
    return {
        "rows_folded": rows_folded,
        "self_edges_dropped": self_edges_dropped,
        "error": None,
    }


def _empty_result_section() -> dict[str, Any]:
    return {
        "identity": _identity_section([], [], False),
        "voiceprints": _voiceprint_section(0, 0, 0),
        "facets": _facet_section([], [], 0, 0),
        "segments": _segment_section(0, 0, 0),
        "activities": _activity_section(),
        "edges": _edges_section(),
    }


def _is_missing_value(value: Any) -> bool:
    return value in (None, "", [], {})


def _journal_root() -> Path:
    return Path(get_journal())


def _rel(path: Path) -> str:
    return path.resolve().relative_to(_journal_root().resolve()).as_posix()


def _contained(rel: str) -> Path:
    return contained_path(_journal_root(), rel)


def _load_json_object(
    path: Path, *, default: dict[str, Any] | None = None
) -> dict[str, Any] | None:
    data = read_json(path, on_error=MalformedPolicy.RAISE, default=default)
    if data is None:
        return None
    if not isinstance(data, dict):
        raise MergePreflightError(f"expected JSON object at {path}")
    return data


def _load_jsonl_objects(path: Path) -> list[dict[str, Any]]:
    rows = read_jsonl(path, on_error=MalformedPolicy.RAISE)
    out: list[dict[str, Any]] = []
    for row in rows:
        if not isinstance(row, dict):
            raise MergePreflightError(f"expected JSONL object rows at {path}")
        out.append(row)
    return out


def _load_journal_entity_strict(entity_id: str) -> dict[str, Any] | None:
    path = _journal_root() / "entities" / entity_id / "entity.json"
    entity = _load_json_object(path, default=None)
    if entity is not None:
        entity["id"] = entity_id
    return entity


def _snapshot_path(path: Path) -> dict[str, Any]:
    if not path.exists():
        return {"exists": False, "kind": "missing", "rel": _rel(path)}
    if path.is_file():
        stat = path.stat()
        return {
            "exists": True,
            "kind": "file",
            "rel": _rel(path),
            "mode": stat.st_mode,
            "bytes": base64.b64encode(path.read_bytes()).decode("ascii"),
        }
    if path.is_dir():
        files: list[dict[str, str]] = []
        dirs: list[str] = []
        for child in sorted(path.rglob("*")):
            child_rel = child.relative_to(path).as_posix()
            if child.is_dir():
                dirs.append(child_rel)
            elif child.is_file():
                stat = child.stat()
                files.append(
                    {
                        "rel": child_rel,
                        "mode": stat.st_mode,
                        "bytes": base64.b64encode(child.read_bytes()).decode("ascii"),
                    }
                )
        return {
            "exists": True,
            "kind": "dir",
            "rel": _rel(path),
            "dirs": dirs,
            "files": files,
        }
    raise MergePreflightError(f"unsupported path type for snapshot: {path}")


def _restore_snapshot(snapshot: dict[str, Any]) -> None:
    rel = snapshot.get("rel")
    if not isinstance(rel, str):
        raise MergeRepairRequired("snapshot missing journal-relative path")
    path = _contained(rel)
    if path.exists():
        if path.is_dir():
            shutil.rmtree(path)
        else:
            path.unlink()
    if not snapshot.get("exists"):
        return
    kind = snapshot.get("kind")
    if kind == "file":
        payload = snapshot.get("bytes")
        if not isinstance(payload, str):
            raise MergeRepairRequired(f"file snapshot missing bytes for {rel}")
        atomic_replace(path, base64.b64decode(payload.encode("ascii")))
        mode = snapshot.get("mode")
        if isinstance(mode, int):
            path.chmod(mode)
        return
    if kind == "dir":
        path.mkdir(parents=True, exist_ok=True)
        for dir_rel in snapshot.get("dirs", []):
            if not isinstance(dir_rel, str) or dir_rel.startswith("../"):
                raise MergeRepairRequired(
                    f"invalid directory snapshot path: {dir_rel!r}"
                )
            (path / dir_rel).mkdir(parents=True, exist_ok=True)
        for item in snapshot.get("files", []):
            file_rel = item.get("rel")
            payload = item.get("bytes")
            if not isinstance(file_rel, str) or not isinstance(payload, str):
                raise MergeRepairRequired(f"invalid file snapshot item in {rel}")
            target = path / file_rel
            target.parent.mkdir(parents=True, exist_ok=True)
            atomic_replace(target, base64.b64decode(payload.encode("ascii")))
            mode = item.get("mode")
            if isinstance(mode, int):
                target.chmod(mode)
        return
    raise MergeRepairRequired(f"unknown snapshot kind for {rel}: {kind!r}")


class _Rollback:
    def __init__(self) -> None:
        self._snapshots: dict[str, dict[str, Any]] = {}

    def snapshot(self, path: Path) -> None:
        rel = _rel(path)
        if rel not in self._snapshots:
            self._snapshots[rel] = _snapshot_path(path)

    def rollback(self) -> None:
        errors: list[str] = []
        for snapshot in reversed(list(self._snapshots.values())):
            try:
                _restore_snapshot(snapshot)
            except Exception as exc:  # pragma: no cover - repair path
                logger.exception("rollback restore failed for %s", snapshot.get("rel"))
                errors.append(f"{snapshot.get('rel')}: {exc}")
        if errors:
            raise MergeRepairRequired("; ".join(errors))


def _active_payloads(entity_id: str) -> list[dict[str, Any]]:
    return load_entity_merge_payloads(entity_id)


def _merge_was_undone(merge_id: str) -> bool:
    for entity_id in scan_journal_entities():
        for event in iter_entity_history(entity_id):
            if event.get("kind") == "merge_undo":
                operation = event.get("operation")
                if isinstance(operation, dict) and operation.get("undo_of") == merge_id:
                    return True
    return False


def _plan_identity_merge(
    source_entity: dict[str, Any],
    target_entity: dict[str, Any],
    *,
    keep_source_as_aka: bool,
) -> tuple[dict[str, Any], dict[str, Any], dict[str, Any]]:
    target_after = dict(target_entity)
    source_display = str(source_entity.get("name", source_entity.get("id", "")))
    target_name = str(target_entity.get("name", ""))

    target_akas = target_entity.get("aka", [])
    if not isinstance(target_akas, list):
        target_akas = []
    source_aka_values = source_entity.get("aka", [])
    if not isinstance(source_aka_values, list):
        source_aka_values = []

    aka_candidates: list[str] = []
    if keep_source_as_aka and source_display and source_display != target_name:
        aka_candidates.append(source_display)
    aka_candidates.extend(str(value) for value in source_aka_values if value)

    target_aka_keys = {str(value).lower() for value in target_akas if value}
    added_akas: list[str] = []
    seen_added_akas: set[str] = set()
    aka_support: list[dict[str, Any]] = []
    for value in aka_candidates:
        key = value.lower()
        preexisting = key in target_aka_keys
        aka_support.append(
            {
                "value": value,
                "key": key,
                "target_preexisting": preexisting,
                "delta_applied": not preexisting and key not in seen_added_akas,
            }
        )
        if preexisting or key in seen_added_akas:
            continue
        seen_added_akas.add(key)
        added_akas.append(value)

    merged_akas = _dedupe_akas(target_akas, aka_candidates)
    if merged_akas:
        target_after["aka"] = merged_akas

    target_emails = target_entity.get("emails", [])
    if not isinstance(target_emails, list):
        target_emails = []
    source_emails = source_entity.get("emails", [])
    if not isinstance(source_emails, list):
        source_emails = []

    target_email_keys = {str(value).lower() for value in target_emails if value}
    added_emails: list[str] = []
    seen_added_emails: set[str] = set()
    email_support: list[dict[str, Any]] = []
    for value in source_emails:
        if not value:
            continue
        email = str(value)
        key = email.lower()
        preexisting = key in target_email_keys
        email_support.append(
            {
                "value": email,
                "key": key,
                "target_preexisting": preexisting,
                "delta_applied": not preexisting and key not in seen_added_emails,
            }
        )
        if preexisting or key in seen_added_emails:
            continue
        seen_added_emails.add(key)
        added_emails.append(email)

    merged_emails = _dedupe_emails(target_emails, source_emails)
    if merged_emails:
        target_after["emails"] = merged_emails

    principal_transferred = bool(
        source_entity.get("is_principal") and not target_entity.get("is_principal")
    )
    if principal_transferred:
        target_after["is_principal"] = True

    scalar_support: list[dict[str, Any]] = []
    for key, value in source_entity.items():
        if key in IDENTITY_SCALAR_EXCLUDES or _is_missing_value(value):
            continue
        target_prevalue = copy.deepcopy(target_after.get(key))
        target_missing = _is_missing_value(target_prevalue)
        delta_applied = target_missing
        scalar_support.append(
            {
                "key": key,
                "source_value": copy.deepcopy(value),
                "target_prevalue": MISSING_SENTINEL
                if target_missing
                else target_prevalue,
                "target_prevalue_missing": target_missing,
                "delta_applied": delta_applied,
            }
        )
        if delta_applied:
            target_after[key] = value

    target_after["updated_at"] = now_ms()
    section = _identity_section(added_akas, added_emails, principal_transferred)
    manifest = {
        "target_before": copy.deepcopy(target_entity),
        "target_after": copy.deepcopy(target_after),
        "source_before": copy.deepcopy(source_entity),
        "aka_support": aka_support,
        "email_support": email_support,
        "scalar_support": scalar_support,
        "principal_transferred": principal_transferred,
    }
    return target_after, section, manifest


def _load_voiceprints_strict(entity_id: str) -> tuple[Any, list[dict[str, Any]]] | None:
    path = _journal_root() / "entities" / entity_id / "voiceprints.npz"
    if not path.exists():
        return None
    data = load_npz(path)
    if data is None or "embeddings" not in data or "metadata" not in data:
        raise MergePreflightError(f"malformed voiceprint payload: {path}")
    metadata = [json.loads(str(item)) for item in data["metadata"]]
    return data["embeddings"], metadata


def _voiceprint_key(meta: dict[str, Any]) -> tuple[Any, Any, Any, Any]:
    return (
        meta.get("day"),
        meta.get("segment_key"),
        meta.get("source"),
        meta.get("sentence_id"),
    )


def _plan_voiceprint_merge(source_id: str, target_id: str) -> dict[str, Any]:
    source_vp = _load_voiceprints_strict(source_id)
    target_vp = _load_voiceprints_strict(target_id)
    existing_keys = load_existing_voiceprint_keys(target_id)

    new_items: list[tuple[Any, dict[str, Any]]] = []
    support: list[dict[str, Any]] = []
    skipped_duplicate = 0
    if source_vp is not None:
        source_embeddings, source_metadata = source_vp
        for emb, meta in zip(source_embeddings, source_metadata):
            key = _voiceprint_key(meta)
            preexisting = key in existing_keys
            normalized = normalize_embedding(emb)
            support.append(
                {
                    "key": list(key),
                    "metadata": copy.deepcopy(meta),
                    "target_preexisting": preexisting,
                    "delta_applied": bool(normalized is not None and not preexisting),
                }
            )
            if preexisting:
                skipped_duplicate += 1
                continue
            if normalized is None:
                continue
            new_items.append((normalized, meta))
            existing_keys.add(key)

    target_existing_total = len(target_vp[0]) if target_vp else 0
    added = len(new_items)
    return {
        "items": new_items,
        "section": _voiceprint_section(
            added=added,
            skipped_duplicate=skipped_duplicate,
            target_total=target_existing_total + added,
        ),
        "manifest": {"support": support},
    }


def _plan_facet_merge(source_id: str, target_id: str) -> dict[str, Any]:
    journal = _journal_root()
    facets_dir = journal / "facets"
    operations: list[dict[str, Any]] = []
    support: list[dict[str, Any]] = []
    moved: list[str] = []
    merged: list[str] = []
    observations_appended = 0

    if not facets_dir.exists():
        return {
            "operations": operations,
            "section": _facet_section(moved, merged, observations_appended, 0),
            "manifest": {"entries": support},
        }

    for facet_entry in sorted(facets_dir.iterdir()):
        if not facet_entry.is_dir():
            continue
        facet_name = facet_entry.name
        source_rel_dir = facet_entry / "entities" / source_id
        source_rel_path = source_rel_dir / "entity.json"
        if not source_rel_path.is_file():
            continue

        source_rel = _load_json_object(source_rel_path) or {}
        target_rel_dir = facet_entry / "entities" / target_id
        target_rel_path = target_rel_dir / "entity.json"
        source_obs = _load_jsonl_objects(source_rel_dir / "observations.jsonl")

        if not target_rel_path.is_file():
            moved.append(facet_name)
            operations.append(
                {
                    "kind": "move",
                    "facet": facet_name,
                    "source_rel_dir": source_rel_dir,
                    "target_rel_dir": target_rel_dir,
                    "relationship": {**source_rel, "entity_id": target_id},
                    "observations": source_obs,
                }
            )
            support.append(
                {
                    "kind": "move",
                    "facet": facet_name,
                    "target_preexisting": False,
                    "relationship": copy.deepcopy(source_rel),
                    "observations": [
                        _observation_support_entry(item, False, True)
                        for item in source_obs
                    ],
                    "scalar_support": _facet_scalar_support(source_rel, {}),
                }
            )
            observations_appended += len(source_obs)
            continue

        target_rel = _load_json_object(target_rel_path) or {}
        merged_rel = dict(target_rel)
        scalar_support = _facet_scalar_support(source_rel, target_rel)
        source_attached = source_rel.get("attached_at")
        target_attached = merged_rel.get("attached_at")
        if source_attached and (
            not target_attached or source_attached < target_attached
        ):
            merged_rel["attached_at"] = source_attached

        for field in ("updated_at", "last_seen"):
            source_ts = source_rel.get(field)
            target_ts = merged_rel.get(field)
            if source_ts and (not target_ts or source_ts > target_ts):
                merged_rel[field] = source_ts

        if not merged_rel.get("description") and source_rel.get("description"):
            merged_rel["description"] = source_rel["description"]

        target_obs = _load_jsonl_objects(target_rel_dir / "observations.jsonl")
        target_obs_keys = {_observation_key(item) for item in target_obs}
        merged_obs = _dedupe_observations(source_obs, target_obs)
        observation_support = [
            _observation_support_entry(
                item,
                _observation_key(item) in target_obs_keys,
                _observation_key(item) not in target_obs_keys,
            )
            for item in source_obs
        ]
        observations_added = len(merged_obs) - len(target_obs)
        observations_appended += observations_added

        operations.append(
            {
                "kind": "merge",
                "facet": facet_name,
                "source_rel_dir": source_rel_dir,
                "target_rel_dir": target_rel_dir,
                "relationship": merged_rel,
                "observations": merged_obs,
                "observations_added": observations_added,
            }
        )
        support.append(
            {
                "kind": "merge",
                "facet": facet_name,
                "target_preexisting": True,
                "scalar_support": scalar_support,
                "observations": observation_support,
            }
        )
        merged.append(facet_name)

    return {
        "operations": operations,
        "section": _facet_section(moved, merged, observations_appended, 0),
        "manifest": {"entries": support},
    }


def _facet_scalar_support(
    source_rel: dict[str, Any],
    target_rel: dict[str, Any],
) -> list[dict[str, Any]]:
    support = []
    for field in FACET_SCALAR_FIELDS:
        source_value = source_rel.get(field)
        if _is_missing_value(source_value):
            continue
        target_prevalue = target_rel.get(field)
        target_missing = _is_missing_value(target_prevalue)
        if field == "attached_at":
            delta_applied = bool(
                source_value and (target_missing or source_value < target_prevalue)
            )
        elif field in {"updated_at", "last_seen"}:
            delta_applied = bool(
                source_value and (target_missing or source_value > target_prevalue)
            )
        else:
            delta_applied = bool(target_missing)
        support.append(
            {
                "field": field,
                "source_value": copy.deepcopy(source_value),
                "target_prevalue": MISSING_SENTINEL
                if target_missing
                else copy.deepcopy(target_prevalue),
                "target_prevalue_missing": target_missing,
                "delta_applied": delta_applied,
            }
        )
    return support


def _observation_key(item: dict[str, Any]) -> tuple[Any, Any]:
    return (item.get("content", ""), item.get("observed_at"))


def _observation_support_entry(
    item: dict[str, Any],
    target_preexisting: bool,
    delta_applied: bool,
) -> dict[str, Any]:
    return {
        "key": list(_observation_key(item)),
        "row": copy.deepcopy(item),
        "target_preexisting": target_preexisting,
        "delta_applied": delta_applied,
    }


def _plan_segment_rewrites(source_id: str, target_id: str) -> dict[str, Any]:
    labels_rewritten = 0
    corrections_rewritten = 0
    files_scanned = 0
    operations: list[dict[str, Any]] = []
    entries: list[dict[str, Any]] = []
    source_id_bytes = source_id.encode("utf-8")

    for day_path in _segment_day_dirs():
        for _stream, _seg_key, seg_path in iter_segments(day_path):
            files_scanned += 1
            talents_dir = seg_path / "talents"
            labels_path = talents_dir / "speaker_labels.json"
            if labels_path.is_file():
                raw = labels_path.read_bytes()
                if source_id_bytes in raw:
                    data = _load_json_object(labels_path) or {}
                    changed = False
                    for index, label in enumerate(data.get("labels", [])):
                        if (
                            isinstance(label, dict)
                            and label.get("speaker") == source_id
                        ):
                            changed = True
                            entries.append(
                                {
                                    "kind": "speaker_labels",
                                    "path": _rel(labels_path),
                                    "index": index,
                                    "field": "speaker",
                                    "prevalue": source_id,
                                    "row_key": {
                                        "sentence_id": label.get("sentence_id"),
                                        "speaker": source_id,
                                    },
                                    "row_preimage": copy.deepcopy(label),
                                }
                            )
                    if changed:
                        labels_rewritten += 1
                        operations.append(
                            {"kind": "speaker_labels", "path": labels_path}
                        )

            corrections_path = talents_dir / "speaker_corrections.json"
            if corrections_path.is_file():
                raw = corrections_path.read_bytes()
                if source_id_bytes in raw:
                    data = _load_json_object(corrections_path) or {}
                    changed = False
                    for index, correction in enumerate(data.get("corrections", [])):
                        if not isinstance(correction, dict):
                            continue
                        for field in ("original_speaker", "corrected_speaker"):
                            if correction.get(field) == source_id:
                                changed = True
                                entries.append(
                                    {
                                        "kind": "speaker_corrections",
                                        "path": _rel(corrections_path),
                                        "index": index,
                                        "field": field,
                                        "prevalue": source_id,
                                        "row_key": {
                                            "sentence_id": correction.get(
                                                "sentence_id"
                                            ),
                                            "field": field,
                                            "value": source_id,
                                        },
                                        "row_preimage": copy.deepcopy(correction),
                                    }
                                )
                    if changed:
                        corrections_rewritten += 1
                        operations.append(
                            {"kind": "speaker_corrections", "path": corrections_path}
                        )

    return {
        "operations": operations,
        "section": _segment_section(
            labels_rewritten, corrections_rewritten, files_scanned
        ),
        "manifest": {"entries": entries},
    }


def _segment_day_dirs() -> list[Path]:
    chronicle_days = [Path(path) for _, path in sorted(day_dirs().items())]
    journal = _journal_root()
    flat_days = sorted(
        entry
        for entry in journal.iterdir()
        if entry.is_dir() and entry.name.isdigit() and len(entry.name) == 8
    )
    return chronicle_days or flat_days


def _check_aka_cross_references(
    source_id: str, source_display: str, target_id: str
) -> list[str]:
    offenders: list[str] = []
    for entity_id in scan_journal_entities():
        if entity_id in {source_id, target_id}:
            continue
        entity = _load_journal_entity_strict(entity_id)
        if not entity:
            continue
        aka_values = entity.get("aka", [])
        if not isinstance(aka_values, list):
            continue
        if source_id in aka_values or source_display in aka_values:
            offenders.append(entity_id)
    offenders.sort()
    return offenders


ACTIVITY_LIST_FIELDS = {
    "participation": ("entity_id",),
    "commitments": ("owner_entity_id", "counterparty_entity_id"),
    "closures": ("owner_entity_id", "counterparty_entity_id"),
    "decisions": ("owner_entity_id", "counterparty_entity_id"),
    "relations": ("from_entity_id", "to_entity_id"),
}


def _activity_record_paths() -> list[Path]:
    facets_dir = _journal_root() / "facets"
    if not facets_dir.is_dir():
        return []
    return sorted(
        path for path in facets_dir.glob("*/activities/*.jsonl") if path.is_file()
    )


def _plan_activity_remaps(source_id: str, target_id: str) -> dict[str, Any]:
    entries: list[dict[str, Any]] = []
    records_rewritten = 0
    fields_rewritten = 0
    files_rewritten = 0
    files_scanned = 0
    for path in _activity_record_paths():
        files_scanned += 1
        rows = _load_jsonl_objects(path)
        file_changed = False
        for record_index, record in enumerate(rows):
            record_id = record.get("id")
            record_changed = False
            active = record.get("active_entities")
            if isinstance(active, list):
                source_occurrence = 0
                for index, value in enumerate(active):
                    if value == source_id:
                        entries.append(
                            {
                                "path": _rel(path),
                                "record_id": record_id,
                                "record_index": record_index,
                                "field_path": ["active_entities", index],
                                "occurrence": source_occurrence,
                                "prevalue": source_id,
                            }
                        )
                        source_occurrence += 1
                        fields_rewritten += 1
                        record_changed = True
            for list_field, id_fields in ACTIVITY_LIST_FIELDS.items():
                items = record.get(list_field)
                if not isinstance(items, list):
                    continue
                for item_index, item in enumerate(items):
                    if not isinstance(item, dict):
                        continue
                    for id_field in id_fields:
                        if item.get(id_field) == source_id:
                            entries.append(
                                {
                                    "path": _rel(path),
                                    "record_id": record_id,
                                    "record_index": record_index,
                                    "field_path": [list_field, item_index, id_field],
                                    "item_preimage": copy.deepcopy(item),
                                    "prevalue": source_id,
                                }
                            )
                            fields_rewritten += 1
                            record_changed = True
            if record_changed:
                records_rewritten += 1
                file_changed = True
        if file_changed:
            files_rewritten += 1
    return {
        "section": _activity_section(
            records_rewritten=records_rewritten,
            fields_rewritten=fields_rewritten,
            files_scanned=files_scanned,
            files_rewritten=files_rewritten,
        ),
        "manifest": {"entries": entries},
    }


def _observation_relation_paths() -> list[Path]:
    facets_dir = _journal_root() / "facets"
    if not facets_dir.exists():
        return []
    return sorted(facets_dir.glob("*/entities/*/observations.jsonl"))


def _plan_observation_relation_remaps(source_id: str) -> dict[str, Any]:
    entries: list[dict[str, Any]] = []
    for path in _observation_relation_paths():
        rows = _load_jsonl_objects(path)
        for index, observation in enumerate(rows):
            relation = observation.get("relation")
            if (
                isinstance(relation, dict)
                and relation.get("target_entity_id") == source_id
            ):
                entries.append(
                    {
                        "path": _rel(path),
                        "index": index,
                        "prevalue": source_id,
                        "row_preimage": copy.deepcopy(observation),
                    }
                )
    return {"entries": entries, "count": len(entries)}


def _apply_facet_additive_plan(
    operations: list[dict[str, Any]],
    target_id: str,
) -> None:
    for operation in operations:
        save_facet_relationship(
            operation["facet"], target_id, copy.deepcopy(operation["relationship"])
        )
        save_observations(
            operation["facet"], target_id, copy.deepcopy(operation["observations"])
        )


def _apply_destructive_plan(
    operations: list[dict[str, Any]],
    source_id: str,
) -> list[str]:
    for operation in operations:
        source_rel_dir = operation["source_rel_dir"]
        if source_rel_dir.exists():
            shutil.rmtree(source_rel_dir)

    discovery_cache = _journal_root() / "awareness" / "discovery_clusters.json"
    caches_cleared: list[str] = []
    if discovery_cache.exists():
        discovery_cache.unlink()
        caches_cleared.append("discovery_clusters")

    source_entity_dir = _journal_root() / "entities" / source_id
    if source_entity_dir.exists():
        shutil.rmtree(source_entity_dir)

    return caches_cleared


def _apply_observation_relation_remaps(
    entries: list[dict[str, Any]],
    target_id: str,
) -> int:
    rewritten = 0
    for path_rel in sorted({entry["path"] for entry in entries}):
        path = _contained(path_rel)
        rows = _load_jsonl_objects(path)
        changed = False
        for entry in [item for item in entries if item["path"] == path_rel]:
            index = int(entry["index"])
            if index >= len(rows):
                continue
            relation = rows[index].get("relation")
            if (
                isinstance(relation, dict)
                and relation.get("target_entity_id") == entry["prevalue"]
            ):
                relation["target_entity_id"] = target_id
                changed = True
                rewritten += 1
        if changed:
            facet = path.parent.parent.parent.name
            entity_id = path.parent.name
            save_observations(facet, entity_id, rows)
    return rewritten


def _apply_activity_remaps(
    entries: list[dict[str, Any]], target_id: str
) -> dict[str, Any]:
    from solstone.think.activities import locked_modify

    by_path: dict[str, list[dict[str, Any]]] = {}
    for entry in entries:
        by_path.setdefault(entry["path"], []).append(entry)
    records_rewritten = 0
    fields_rewritten = 0
    files_rewritten = 0
    for path_rel, path_entries in by_path.items():
        path = _contained(path_rel)
        file_fields = 0
        file_records: set[tuple[Any, int]] = set()

        def modify(rows: list[dict[str, Any]]) -> list[dict[str, Any]]:
            nonlocal file_fields
            for entry in path_entries:
                record_index = int(entry["record_index"])
                if record_index >= len(rows):
                    continue
                if rows[record_index].get("id") != entry.get("record_id"):
                    continue
                row = copy.deepcopy(rows[record_index])
                if _set_nested_if_value(
                    row,
                    entry["field_path"],
                    entry["prevalue"],
                    target_id,
                ):
                    rows[record_index] = row
                    file_fields += 1
                    file_records.add((entry.get("record_id"), record_index))
            return rows

        locked_modify(path, modify)
        if file_fields:
            files_rewritten += 1
            fields_rewritten += file_fields
            records_rewritten += len(file_records)
    return _activity_section(
        records_rewritten, fields_rewritten, len(by_path), files_rewritten
    )


def _undo_activity_remaps(
    entries: list[dict[str, Any]], source_id: str, target_id: str
) -> None:
    from solstone.think.activities import apply_entity_merge_activity_inverse

    apply_entity_merge_activity_inverse(
        entries,
        source_id=source_id,
        target_id=target_id,
    )


def _set_nested_if_value(
    row: dict[str, Any],
    field_path: list[Any],
    expected: Any,
    replacement: Any,
) -> bool:
    if len(field_path) == 2 and field_path[0] == "active_entities":
        values = row.get("active_entities")
        index = int(field_path[1])
        if (
            isinstance(values, list)
            and index < len(values)
            and values[index] == expected
        ):
            values[index] = replacement
            return True
    if len(field_path) == 3:
        values = row.get(str(field_path[0]))
        index = int(field_path[1])
        key = str(field_path[2])
        if isinstance(values, list) and index < len(values):
            item = values[index]
            if isinstance(item, dict) and item.get(key) == expected:
                item[key] = replacement
                return True
    return False


def _apply_segment_plan(
    operations: list[dict[str, Any]],
    source_id: str,
    target_id: str,
) -> None:
    from solstone.apps.speakers.attribution import (
        remap_speaker_corrections_for_entity_merge,
        update_speaker_labels,
    )

    def remap_label_speakers(current: dict | None) -> dict | None:
        if current is None:
            return None
        labels = current.get("labels", [])
        if not isinstance(labels, list):
            return None
        changed = False
        for label in labels:
            if not isinstance(label, dict):
                continue
            if label.get("speaker") == source_id:
                label["speaker"] = target_id
                changed = True
        return current if changed else None

    for operation in operations:
        seg_dir = operation["path"].parent.parent
        if operation["kind"] == "speaker_labels":
            update_speaker_labels(seg_dir, remap_label_speakers)
        elif operation["kind"] == "speaker_corrections":
            remap_speaker_corrections_for_entity_merge(seg_dir, source_id, target_id)
        else:
            raise ValueError(f"Unknown segment merge operation: {operation['kind']}")


def _undo_segment_remaps(
    entries: list[dict[str, Any]], source_id: str, target_id: str
) -> None:
    from solstone.apps.speakers.attribution import apply_entity_merge_segment_inverse

    apply_entity_merge_segment_inverse(
        entries,
        source_id=source_id,
        target_id=target_id,
    )


def _audit_counts(result: dict[str, Any]) -> dict[str, Any]:
    return {
        "identity": {
            "akas_added": result["identity"]["akas_added_count"],
            "emails_added": result["identity"]["emails_added_count"],
            "principal_transferred": result["identity"]["principal_transferred"],
        },
        "voiceprints": {
            "added": result["voiceprints"]["added"],
            "skipped_duplicate": result["voiceprints"]["skipped_duplicate"],
            "target_total": result["voiceprints"]["target_total"],
        },
        "facets": {
            "moved": result["facets"]["moved_count"],
            "merged": result["facets"]["merged_count"],
            "observations_appended": result["facets"]["observations_appended"],
            "observation_relations_rewritten": result["facets"][
                "observation_relations_rewritten"
            ],
        },
        "segments": {
            "labels_rewritten": result["segments"]["labels_rewritten"],
            "corrections_rewritten": result["segments"]["corrections_rewritten"],
            "files_scanned": result["segments"]["files_scanned"],
            "errors": 0,
        },
        "activities": {
            "records_rewritten": result["activities"]["records_rewritten"],
            "fields_rewritten": result["activities"]["fields_rewritten"],
            "files_scanned": result["activities"]["files_scanned"],
            "files_rewritten": result["activities"]["files_rewritten"],
            "errors": 0,
        },
        "edges": {
            "rows_folded": result["edges"]["rows_folded"],
            "self_edges_dropped": result["edges"]["self_edges_dropped"],
            "error": None,
        },
    }


def _append_audit_log(
    *,
    source_id: str,
    source_display_name: str,
    target_id: str,
    target_display_name: str,
    merge_id: str,
    result: dict[str, Any],
    caller: str,
) -> str:
    logs_dir = _journal_root() / "logs"
    audit_path = logs_dir / "entity-merges.jsonl"
    payload = {
        "ts": now_ms(),
        "merge_id": merge_id,
        "source_id": source_id,
        "source_display_name": source_display_name,
        "target_id": target_id,
        "target_display_name": target_display_name,
        "principal_transferred": result["identity"]["principal_transferred"],
        "counts": _audit_counts(result),
        "caller": caller,
    }
    append_jsonl(audit_path, payload)
    return str(audit_path)


def _edge_db_paths() -> list[Path]:
    # The index is derived owner data. Snapshot existing files without opening
    # SQLite: opening through the Python reference writer would mutate schema.
    base = _journal_root() / "indexer" / "journal.sqlite"
    return [base, Path(f"{base}-wal"), Path(f"{base}-shm")]


def _edge_hash() -> str:
    from solstone.think.indexer.edges import fingerprint_edge_rows

    return fingerprint_edge_rows(str(_journal_root()))


def _fold_edges(source_id: str, target_id: str) -> dict[str, int]:
    from solstone.think.indexer.edges import fold_entity_edges_for_recorded_merge

    return fold_entity_edges_for_recorded_merge(
        source_id,
        target_id,
        str(_journal_root()),
    )


def _strict_preflight(source_id: str, target_id: str) -> None:
    source = _load_journal_entity_strict(source_id)
    target = _load_journal_entity_strict(target_id)
    if source is None:
        raise MergePreflightError(f"Source entity not found: {source_id}")
    if target is None:
        raise MergePreflightError(f"Target entity not found: {target_id}")
    _load_voiceprints_strict(source_id)
    _load_voiceprints_strict(target_id)
    for path in _observation_relation_paths():
        _load_jsonl_objects(path)
    for path in _activity_record_paths():
        _load_jsonl_objects(path)
    for day_path in _segment_day_dirs():
        for _stream, _seg_key, seg_path in iter_segments(day_path):
            for name in ("speaker_labels.json", "speaker_corrections.json"):
                path = seg_path / "talents" / name
                if path.is_file():
                    _load_json_object(path)
    facets_dir = _journal_root() / "facets"
    if facets_dir.is_dir():
        for path in facets_dir.glob("*/entities/*/entity.json"):
            _load_json_object(path)
        for path in facets_dir.glob("*/entities/*/observations.jsonl"):
            _load_jsonl_objects(path)
    _strict_preflight_edge_sources()


def _strict_preflight_edge_sources() -> None:
    from solstone.think.indexer.edges import discover_edge_files

    for rel, abs_path in discover_edge_files(str(_journal_root())).items():
        path = Path(abs_path)
        try:
            if path.suffix == ".json":
                read_json(path, on_error=MalformedPolicy.RAISE)
            else:
                read_jsonl(path, on_error=MalformedPolicy.RAISE)
        except Exception as exc:
            raise MergePreflightError(
                f"edge source preflight failed for {rel}: {exc}"
            ) from exc


def _plan_merge(
    source_id: str,
    target_id: str,
    *,
    keep_source_as_aka: bool,
) -> dict[str, Any]:
    source_entity = _load_journal_entity_strict(source_id)
    target_entity = _load_journal_entity_strict(target_id)
    if not source_entity:
        raise MergePreflightError(f"Source entity not found: {source_id}")
    if not target_entity:
        raise MergePreflightError(f"Target entity not found: {target_id}")
    if source_entity.get("blocked"):
        raise MergePreflightError(f"Cannot merge blocked entity: {source_id}")
    if target_entity.get("blocked"):
        raise MergePreflightError(f"Cannot merge blocked entity: {target_id}")
    if source_entity.get("is_principal") and target_entity.get("is_principal"):
        raise MergePreflightError("Cannot merge two principal entities.")
    source_display = str(source_entity.get("name", source_id))
    offenders = _check_aka_cross_references(source_id, source_display, target_id)
    if offenders:
        offender_str = ", ".join(offenders)
        raise MergePreflightError(
            f"Cannot merge '{source_id}': referenced in aka lists of entity ids: {offender_str}"
        )

    planned_target, identity_section, identity_manifest = _plan_identity_merge(
        source_entity,
        target_entity,
        keep_source_as_aka=keep_source_as_aka,
    )
    voiceprint_plan = _plan_voiceprint_merge(source_id, target_id)
    facet_plan = _plan_facet_merge(source_id, target_id)
    observation_relations = _plan_observation_relation_remaps(source_id)
    facet_plan["section"]["observation_relations_rewritten"] = observation_relations[
        "count"
    ]
    segment_plan = _plan_segment_rewrites(source_id, target_id)
    activity_plan = _plan_activity_remaps(source_id, target_id)
    return {
        "source_entity": source_entity,
        "target_entity": target_entity,
        "planned_target": planned_target,
        "identity": identity_section,
        "identity_manifest": identity_manifest,
        "voiceprints": voiceprint_plan,
        "facets": facet_plan,
        "observation_relations": observation_relations,
        "segments": segment_plan,
        "activities": activity_plan,
    }


def _result_from_plan(
    source_id: str,
    target_id: str,
    plan: dict[str, Any],
    *,
    commit: bool,
    would_fold_edges: int | None,
) -> dict[str, Any]:
    zero = _empty_result_section()
    return {
        "merged": commit,
        "source_id": source_id,
        "target_id": target_id,
        "identity": plan["identity"] if commit else zero["identity"],
        "voiceprints": plan["voiceprints"]["section"]
        if commit
        else zero["voiceprints"],
        "facets": plan["facets"]["section"] if commit else zero["facets"],
        "segments": plan["segments"]["section"] if commit else zero["segments"],
        "activities": plan["activities"]["section"] if commit else zero["activities"],
        "edges": zero["edges"],
        "caches_cleared": [],
        "audit_log_path": None,
        "would_identity": None if commit else plan["identity"],
        "would_voiceprints": None if commit else plan["voiceprints"]["section"],
        "would_facets": None if commit else plan["facets"]["section"],
        "would_segments": None if commit else plan["segments"]["section"],
        "would_activities": None if commit else plan["activities"]["section"],
        "would_fold_edges": None if commit else would_fold_edges,
    }


def _snapshot_merge_paths(
    rollback: _Rollback,
    source_id: str,
    target_id: str,
    plan: dict[str, Any],
) -> None:
    rollback.snapshot(_journal_root() / "entities" / source_id)
    rollback.snapshot(_journal_root() / "entities" / target_id)
    rollback.snapshot(_journal_root() / "logs" / "entity-merges.jsonl")
    rollback.snapshot(_journal_root() / "awareness" / "discovery_clusters.json")
    for path in _edge_db_paths():
        rollback.snapshot(path)
    for operation in plan["facets"]["operations"]:
        rollback.snapshot(operation["source_rel_dir"])
        rollback.snapshot(operation["target_rel_dir"])
    for entry in plan["activities"]["manifest"]["entries"]:
        rollback.snapshot(_contained(entry["path"]))
    for entry in plan["segments"]["manifest"]["entries"]:
        rollback.snapshot(_contained(entry["path"]))
    for entry in plan["observation_relations"]["entries"]:
        rollback.snapshot(_contained(entry["path"]))


def _source_state_payload(source_id: str, plan: dict[str, Any]) -> dict[str, Any]:
    """Capture source-owned directories destroyed by merge cleanup.

    These byte snapshots are persisted only for the source entity directory and
    source facet relationship directories that `_apply_destructive_plan` removes
    wholesale. Target-side owner stores are never restored from this payload;
    undo applies typed inverse sections through each store's owner helper.
    """

    snapshots = [_snapshot_path(_journal_root() / "entities" / source_id)]
    seen: set[str] = {snapshots[0]["rel"]}
    for operation in plan["facets"]["operations"]:
        snapshot = _snapshot_path(operation["source_rel_dir"])
        if snapshot["rel"] not in seen:
            snapshots.append(snapshot)
            seen.add(snapshot["rel"])
    return {"snapshots": snapshots}


def _payload_for_merge(
    merge_id: str,
    source_id: str,
    target_id: str,
    plan: dict[str, Any],
    result: dict[str, Any],
) -> dict[str, Any]:
    return {
        "schema_version": 1,
        "merge_id": merge_id,
        "source_id": source_id,
        "target_id": target_id,
        "commit_seq": None,
        "source_state": _source_state_payload(source_id, plan),
        "result_counts": _audit_counts(result),
        "manifest": {
            "identity": plan["identity_manifest"],
            "voiceprints": plan["voiceprints"]["manifest"],
            "facets": plan["facets"]["manifest"],
            "observation_relations": {
                "entries": plan["observation_relations"]["entries"]
            },
            "segments": plan["segments"]["manifest"],
            "activities": plan["activities"]["manifest"],
            "rebased_merge_ids": [],
        },
    }


def _write_private_payload(
    target_id: str, merge_id: str, payload: dict[str, Any]
) -> str:
    return record_entity_merge_payload(target_id, merge_id, payload)


def _commit_merge(
    source_id: str,
    target_id: str,
    *,
    keep_source_as_aka: bool,
    caller: str,
) -> dict[str, Any]:
    with trust_operation_lock():
        rollback = _Rollback()
        failed_phase = "preflight"
        mutation_applied = False
        try:
            _strict_preflight(source_id, target_id)
            plan = _plan_merge(
                source_id, target_id, keep_source_as_aka=keep_source_as_aka
            )
            result = _result_from_plan(
                source_id, target_id, plan, commit=True, would_fold_edges=None
            )
            merge_id = f"em_{uuid.uuid4().hex}"
            result["merge_id"] = merge_id
            _snapshot_merge_paths(rollback, source_id, target_id, plan)
            payload = _payload_for_merge(merge_id, source_id, target_id, plan, result)

            failed_phase = "private_payload"
            private_payload_rel = _write_private_payload(target_id, merge_id, payload)
            mutation_applied = True

            failed_phase = "voiceprints"
            if plan["voiceprints"]["items"]:
                save_voiceprints_batch(target_id, plan["voiceprints"]["items"])

            failed_phase = "facets"
            _apply_facet_additive_plan(plan["facets"]["operations"], target_id)

            failed_phase = "segments"
            _apply_segment_plan(plan["segments"]["operations"], source_id, target_id)

            failed_phase = "activities"
            result["activities"] = _apply_activity_remaps(
                plan["activities"]["manifest"]["entries"], target_id
            )

            failed_phase = "lineage"
            rebased = _rebase_descendant_payloads(
                source_id, target_id, plan["planned_target"], caller
            )
            if rebased:
                payload["manifest"]["rebased_merge_ids"] = rebased
                private_payload_rel = _write_private_payload(
                    target_id, merge_id, payload
                )

            failed_phase = "cleanup"
            result["caches_cleared"] = _apply_destructive_plan(
                plan["facets"]["operations"], source_id
            )

            failed_phase = "observation relation remap"
            result["facets"]["observation_relations_rewritten"] = (
                _apply_observation_relation_remaps(
                    plan["observation_relations"]["entries"], target_id
                )
            )

            failed_phase = "history"
            save_journal_entity(
                plan["planned_target"],
                operation=EntityOperationContext(
                    kind="merge",
                    caller=caller,
                    metadata={
                        "merge_id": merge_id,
                        "source_id": source_id,
                        "target_id": target_id,
                        "private_payload": private_payload_rel,
                        "counts": _audit_counts(result),
                    },
                ),
            )
            merge_event = list(iter_entity_history(target_id))[-1]
            payload["commit_seq"] = merge_event["seq"]
            _write_private_payload(target_id, merge_id, payload)

            failed_phase = "edges"
            fold = _fold_edges(source_id, target_id)
            result["edges"] = _edges_section(
                rows_folded=fold["rows_folded"],
                self_edges_dropped=fold["self_edges_dropped"],
            )
            payload["result_counts"] = _audit_counts(result)
            _write_private_payload(target_id, merge_id, payload)

            failed_phase = "audit"
            source_display = str(plan["source_entity"].get("name", source_id))
            target_display = str(plan["planned_target"].get("name", target_id))
            result["audit_log_path"] = _append_audit_log(
                source_id=source_id,
                source_display_name=source_display,
                target_id=target_id,
                target_display_name=target_display,
                merge_id=merge_id,
                result=result,
                caller=caller,
            )
            return result
        except Exception as exc:
            logger.exception(
                "entity merge failed during %s (source=%s target=%s)",
                failed_phase,
                source_id,
                target_id,
            )
            try:
                rollback.rollback()
            except MergeRepairRequired as rollback_exc:
                return {
                    "error": {
                        "code": "repair_required",
                        "message": str(exc),
                        "rollback_error": str(rollback_exc),
                    },
                    "failed_phase": failed_phase,
                    "source_id": source_id,
                    "target_id": target_id,
                    "operation_state": "repair_required",
                    "mutation_applied": mutation_applied,
                    "source_state": _safe_entity_state(source_id),
                    "target_state": _safe_entity_state(target_id),
                    "safe_remediation": (
                        "Inspect and repair the recorded entity state before retrying."
                    ),
                }
            return {
                "error": str(exc),
                "failed_phase": failed_phase,
                "source_id": source_id,
                "target_id": target_id,
            }


def _rebase_descendant_payloads(
    source_id: str,
    target_id: str,
    target_entity: dict[str, Any],
    caller: str,
) -> list[str]:
    rebased: list[str] = []
    for payload in _active_payloads(source_id):
        descendant_id = str(payload["merge_id"])
        payload, private_payload_rel = move_entity_merge_payload(
            source_id,
            target_id,
            descendant_id,
            rebased_from_entity_id=source_id,
        )
        save_journal_entity(
            target_entity,
            operation=EntityOperationContext(
                kind="merge",
                caller=caller,
                metadata={
                    "merge_id": descendant_id,
                    "source_id": payload["source_id"],
                    "target_id": target_id,
                    "rebased_from_entity_id": source_id,
                    "private_payload": private_payload_rel,
                },
            ),
        )
        rebased.append(descendant_id)
    return rebased


def merge_entity(
    source_id: str,
    target_id: str,
    *,
    keep_source_as_aka: bool = True,
    commit: bool = False,
    caller: str = "entities.merge",
) -> dict[str, Any]:
    if source_id == target_id:
        return {"error": "Source and target must be different entities."}
    try:
        plan = _plan_merge(source_id, target_id, keep_source_as_aka=keep_source_as_aka)
    except Exception as exc:
        return {"error": str(exc)}

    would_fold_edges: int | None = None
    if not commit:
        from solstone.think.indexer.edges import count_entity_edges

        try:
            would_fold_edges = count_entity_edges(source_id)
        except (sqlite3.Error, OSError):
            logger.exception(
                "entity merge dry-run edge count failed (source=%s target=%s)",
                source_id,
                target_id,
            )
        return _result_from_plan(
            source_id,
            target_id,
            plan,
            commit=False,
            would_fold_edges=would_fold_edges,
        )

    return _commit_merge(
        source_id,
        target_id,
        keep_source_as_aka=keep_source_as_aka,
        caller=caller,
    )


def undo_entity_merge(
    merge_id: str, *, caller: str = "entities.merge.undo"
) -> dict[str, Any]:
    try:
        found = find_entity_merge_payload(merge_id)
    except Exception as exc:
        return {"error": str(exc), "merge_id": merge_id}
    if found is None:
        if _merge_was_undone(merge_id):
            return {"error": f"Merge already undone: {merge_id}"}
        return {"error": f"Recorded merge not found: {merge_id}"}
    target_id, payload = found
    source_id = str(payload["source_id"])
    rollback = _Rollback()
    with trust_operation_lock():
        pre_edge_hash = _edge_hash()
        mutation_applied = False
        try:
            rollback.snapshot(_journal_root() / "entities" / target_id)
            rollback.snapshot(_journal_root() / "entities" / source_id)
            for snapshot in payload["source_state"]["snapshots"]:
                rollback.snapshot(_contained(snapshot["rel"]))
            for path in _edge_db_paths():
                rollback.snapshot(path)

            target_entity = _load_journal_entity_strict(target_id)
            if target_entity is None:
                raise EntityHistoryError(f"Merge target not found: {target_id}")
            active_payloads = [
                item
                for item in _active_payloads(target_id)
                if item.get("merge_id") != payload.get("merge_id")
            ]

            mutation_applied = True
            _restore_source_state(payload)
            next_target = _target_after_identity_undo(target_id, payload)
            _undo_voiceprints(target_id, payload, active_payloads)
            _undo_facets(target_id, payload, active_payloads)
            _undo_observations(target_id, source_id, payload, active_payloads)
            _undo_segment_remaps(
                payload["manifest"]["segments"]["entries"], source_id, target_id
            )
            _undo_activity_remaps(
                payload["manifest"]["activities"]["entries"], source_id, target_id
            )
            _delete_rebased_descendants(target_id, payload)

            save_journal_entity(
                next_target,
                operation=EntityOperationContext(
                    kind="merge_undo",
                    caller=caller,
                    metadata={
                        "undo_of": merge_id,
                        "source_id": source_id,
                        "target_id": target_id,
                    },
                ),
            )
            undo_event = list(iter_entity_history(target_id))[-1]
            remove_entity_merge_payload(target_id, merge_id)

            edge_fingerprint = _rebuild_edges_fingerprint(expect_restore=True)
            return {
                "undone": True,
                "merge_id": merge_id,
                "source_id": source_id,
                "target_id": target_id,
                "restored_reference_counts": copy.deepcopy(
                    payload.get("result_counts") or {}
                ),
                "history_version_id": undo_event["version_id"],
                "edge_rebuild": {
                    "rebuilt": True,
                    "fingerprint": edge_fingerprint,
                },
            }
        except Exception as exc:
            logger.exception("entity merge undo failed (merge_id=%s)", merge_id)
            try:
                rollback.rollback()
                if _edge_hash() != pre_edge_hash:
                    raise MergeRepairRequired(
                        "edge state did not return to pre-undo fingerprint"
                    )
            except Exception as rollback_exc:
                return {
                    "error": {
                        "code": "repair_required",
                        "message": str(exc),
                        "rollback_error": str(rollback_exc),
                    },
                    "merge_id": merge_id,
                    "source_id": source_id,
                    "target_id": target_id,
                    "operation_state": "repair_required",
                    "mutation_applied": mutation_applied,
                    "source_state": _safe_entity_state(source_id),
                    "target_state": _safe_entity_state(target_id),
                    "safe_remediation": (
                        "Inspect and repair the recorded entity state before retrying."
                    ),
                }
            return {"error": str(exc), "merge_id": merge_id}


def _restore_source_state(payload: dict[str, Any]) -> None:
    for snapshot in payload["source_state"]["snapshots"]:
        _restore_snapshot(snapshot)


def _target_after_identity_undo(
    target_id: str, payload: dict[str, Any]
) -> dict[str, Any]:
    current = _load_journal_entity_strict(target_id)
    if current is None:
        raise EntityHistoryError(f"target entity not found: {target_id}")
    manifest = payload["manifest"]["identity"]
    merge_seq = int(payload.get("commit_seq") or 0)
    active = [
        item
        for item in _active_payloads(target_id)
        if item.get("merge_id") != payload.get("merge_id")
    ]
    _remove_set_values(current, "aka", manifest["aka_support"], active, merge_seq)
    _remove_set_values(current, "emails", manifest["email_support"], active, merge_seq)
    _replay_identity_set_housekeeping(current, manifest)
    _replay_identity_scalars(
        current, target_id, manifest["scalar_support"], active, merge_seq
    )
    _replay_identity_housekeeping(current, target_id, manifest, merge_seq)
    return current


def _remove_set_values(
    current: dict[str, Any],
    field: str,
    support: list[dict[str, Any]],
    active_payloads: list[dict[str, Any]],
    merge_seq: int,
) -> None:
    values = current.get(field)
    if not isinstance(values, list):
        return
    remove_keys: set[str] = set()
    for entry in support:
        if entry.get("target_preexisting"):
            continue
        key = str(entry.get("key"))
        if _other_payload_supports(field, key, active_payloads):
            continue
        if _later_owner_event_introduced(str(current["id"]), field, key, merge_seq):
            continue
        remove_keys.add(key)
    if remove_keys:
        current[field] = [
            value for value in values if str(value).lower() not in remove_keys
        ]


def _replay_identity_set_housekeeping(
    current: dict[str, Any], manifest: dict[str, Any]
) -> None:
    target_before = manifest.get("target_before", {})
    for field in ("aka", "emails"):
        if field not in target_before and current.get(field) == []:
            current.pop(field, None)


def _other_payload_supports(
    field: str,
    key: str,
    payloads: list[dict[str, Any]],
) -> bool:
    section = "aka_support" if field == "aka" else "email_support"
    for payload in payloads:
        for entry in payload["manifest"]["identity"][section]:
            if str(entry.get("key")) == key:
                return True
    return False


def _later_owner_event_introduced(
    entity_id: str, field: str, key: str, merge_seq: int
) -> bool:
    for event in iter_entity_history(entity_id):
        seq = event.get("seq")
        if not isinstance(seq, int) or seq <= merge_seq:
            continue
        if event.get("kind") in {"merge", "merge_undo"}:
            continue
        before = event.get("identity_before") or {}
        after = event.get("identity_after") or {}
        before_values = before.get(field, [])
        after_values = after.get(field, [])
        before_keys = {
            str(value).lower()
            for value in before_values
            if isinstance(before_values, list)
        }
        after_keys = {
            str(value).lower()
            for value in after_values
            if isinstance(after_values, list)
        }
        if key not in before_keys and key in after_keys:
            return True
    return False


def _replay_identity_scalars(
    current: dict[str, Any],
    target_id: str,
    support: list[dict[str, Any]],
    active_payloads: list[dict[str, Any]],
    merge_seq: int,
) -> None:
    for entry in support:
        key = entry["key"]
        if _later_owner_scalar_changed(target_id, key, merge_seq):
            continue
        if entry.get("target_prevalue_missing"):
            value: Any = None
            missing = True
        else:
            value = copy.deepcopy(entry.get("target_prevalue"))
            missing = False
        for payload in active_payloads:
            if int(payload.get("commit_seq") or 0) <= merge_seq:
                continue
            for other in payload["manifest"]["identity"]["scalar_support"]:
                if other.get("key") != key:
                    continue
                if missing and not _is_missing_value(other.get("source_value")):
                    value = copy.deepcopy(other["source_value"])
                    missing = False
        if missing:
            current.pop(key, None)
        else:
            current[key] = value


def _later_owner_scalar_changed(entity_id: str, key: str, merge_seq: int) -> bool:
    for event in iter_entity_history(entity_id):
        seq = event.get("seq")
        if not isinstance(seq, int) or seq <= merge_seq:
            continue
        if event.get("kind") in {"merge", "merge_undo"}:
            continue
        before = event.get("identity_before") or {}
        after = event.get("identity_after") or {}
        if before.get(key) != after.get(key):
            return True
    return False


def _replay_identity_housekeeping(
    current: dict[str, Any],
    target_id: str,
    manifest: dict[str, Any],
    merge_seq: int,
) -> None:
    target_before = manifest.get("target_before", {})
    if not _later_owner_scalar_changed(target_id, "updated_at", merge_seq):
        if "updated_at" in target_before:
            current["updated_at"] = copy.deepcopy(target_before["updated_at"])
        else:
            current.pop("updated_at", None)
    if manifest.get("principal_transferred") and not _later_owner_scalar_changed(
        target_id, "is_principal", merge_seq
    ):
        if target_before.get("is_principal"):
            current["is_principal"] = target_before["is_principal"]
        else:
            current.pop("is_principal", None)


def _undo_voiceprints(
    target_id: str,
    payload: dict[str, Any],
    active_payloads: list[dict[str, Any]],
) -> None:
    from solstone.think.entities.voiceprints import (
        apply_entity_merge_voiceprint_inverse,
    )

    active_support = [
        item["manifest"]["voiceprints"]["support"] for item in active_payloads
    ]
    apply_entity_merge_voiceprint_inverse(
        target_id,
        payload["manifest"]["voiceprints"]["support"],
        active_support,
    )


def _undo_facets(
    target_id: str,
    payload: dict[str, Any],
    active_payloads: list[dict[str, Any]],
) -> None:
    from solstone.think.entities.relationships import (
        apply_entity_merge_relationship_inverse,
    )

    active_entries: list[dict[str, Any]] = []
    for active in active_payloads:
        for entry in active["manifest"]["facets"]["entries"]:
            active_entry = copy.deepcopy(entry)
            active_entry["_commit_seq"] = int(active.get("commit_seq") or 0)
            active_entries.append(active_entry)
    apply_entity_merge_relationship_inverse(
        target_id=target_id,
        entries=payload["manifest"]["facets"]["entries"],
        active_entries=active_entries,
        merge_seq=int(payload.get("commit_seq") or 0),
    )


def _undo_observations(
    target_id: str,
    source_id: str,
    payload: dict[str, Any],
    active_payloads: list[dict[str, Any]],
) -> None:
    from solstone.think.entities.observations import (
        apply_entity_merge_observation_inverse,
    )

    active_facet_entries: list[dict[str, Any]] = []
    for active in active_payloads:
        active_facet_entries.extend(active["manifest"]["facets"]["entries"])
    apply_entity_merge_observation_inverse(
        target_id=target_id,
        source_id=source_id,
        facet_entries=payload["manifest"]["facets"]["entries"],
        relation_entries=payload["manifest"]["observation_relations"]["entries"],
        active_facet_entries=active_facet_entries,
    )


def _delete_rebased_descendants(target_id: str, payload: dict[str, Any]) -> None:
    for merge_id in payload["manifest"]["rebased_merge_ids"]:
        remove_entity_merge_payload(target_id, str(merge_id))


def _safe_entity_state(entity_id: str) -> dict[str, Any]:
    """Return an authoritative best-effort identity state for repair responses."""
    try:
        entity = _load_journal_entity_strict(entity_id)
    except Exception as exc:
        return {"entity_id": entity_id, "readable": False, "error": str(exc)}
    return {
        "entity_id": entity_id,
        "readable": True,
        "exists": entity is not None,
        "entity": entity,
    }


def _rebuild_edges_fingerprint(*, expect_restore: bool) -> str:
    from solstone.think.indexer.edges import rebuild_edges_for_recorded_merge_undo

    fingerprint = rebuild_edges_for_recorded_merge_undo(str(_journal_root()))
    if expect_restore and fingerprint == "":
        raise sqlite3.DatabaseError("edge rebuild produced no fingerprint")
    return fingerprint
