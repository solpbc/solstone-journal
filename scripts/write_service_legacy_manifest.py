#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Write the inventory-exact service-legacy evidence manifest.

This is a hand-run evidence-capture tool, not a CI dependency. It binds the
git censuses, interpreter declarations, captured fixtures, negative bundles,
and parsed semantic deltas by content hash.
"""

from __future__ import annotations

import hashlib
import json
import subprocess
from pathlib import Path
from typing import Any

from normalize_service_legacy_evidence import (
    NORMALIZED_ROOT,
    PLATFORMS,
    RAW_ROOT,
    expected_profiles,
    normalized_variant,
    read_json,
)

ROOT = Path(__file__).resolve().parents[1]
EVIDENCE_ROOT = ROOT / "core/fixtures/service_legacy_evidence"
FOLLOW_PATH = EVIDENCE_ROOT / "follow-census.json"
TAG_PATH = EVIDENCE_ROOT / "tag-census.json"
INTERPRETERS_PATH = EVIDENCE_ROOT / "interpreters.json"
NEGATIVE_ROOT = EVIDENCE_ROOT / "negative"
DELTAS_PATH = EVIDENCE_ROOT / "semantic-deltas.json"
PACKAGING_PROVENANCE_PATH = EVIDENCE_ROOT / "packaging-provenance.json"
OUTPUT = EVIDENCE_ROOT / "manifest.json"
SCHEMA = "service-legacy-evidence"
SCHEMA_VERSION = 1
TOOLING_PATHS = (
    "scripts/capture_service_legacy_commit_census.py",
    "scripts/capture_service_legacy_tag_census.py",
    "scripts/acquire_service_legacy_cpython.py",
    "scripts/extract_service_legacy_generator_closure.py",
    "scripts/capture_service_legacy_raw.py",
    "scripts/promote_service_legacy_evidence_tree.py",
    "scripts/normalize_service_legacy_evidence.py",
    "scripts/generate_service_legacy_negative_twins.py",
    "scripts/derive_service_legacy_semantic_deltas.py",
    "scripts/build_service_legacy_packaging_provenance.py",
    "scripts/write_service_legacy_manifest.py",
)


class ManifestError(RuntimeError):
    """The evidence corpus is incomplete or internally inconsistent."""


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def relative(path: Path) -> str:
    return path.relative_to(ROOT).as_posix()


def reference(path: Path) -> dict[str, str]:
    if not path.is_file():
        raise ManifestError(f"required evidence file is missing: {path}")
    return {"path": relative(path), "sha256": sha256(path)}


def git_head() -> str:
    result = subprocess.run(
        ["git", "rev-parse", "HEAD"], cwd=ROOT, check=True, capture_output=True, text=True
    )
    return result.stdout.strip()


def canonical_sha256(value: dict[str, Any]) -> str:
    return hashlib.sha256(
        json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode("utf-8")
    ).hexdigest()


def raw_normalized_hash(blob: str, platform: str, profile: str) -> str:
    path = RAW_ROOT / blob / platform / f"{profile}.json"
    raw = read_json(path)
    if raw.get("blob") != blob or raw.get("platform") != platform or raw.get("profile") != profile:
        raise ManifestError(f"raw fixture identity mismatch: {path}")
    return canonical_sha256(normalized_variant(raw, path))


def read_deltas() -> list[dict[str, Any]]:
    payload = read_json(DELTAS_PATH)
    if set(payload) != {"deltas", "schema", "schema_version"}:
        raise ManifestError("semantic deltas top-level keys are not inventory-exact")
    if payload["schema"] != "service-legacy-semantic-deltas" or payload["schema_version"] != 1:
        raise ManifestError("semantic deltas schema declaration is invalid")
    deltas = payload["deltas"]
    if not isinstance(deltas, list) or len(deltas) != 43:
        raise ManifestError("semantic deltas must contain exactly 43 records")
    return deltas


def release_statuses(tag_census: dict[str, Any], tag_census_sha256: str) -> dict[str, list[str]]:
    tags_by_blob: dict[str, list[str]] = {}
    tags = tag_census.get("tags")
    if not isinstance(tags, list) or len(tags) != 66:
        raise ManifestError("tag census must contain exactly 66 tags")
    for row in tags:
        blob = row.get("blob")
        if blob is not None:
            tags_by_blob.setdefault(blob, []).append(row["tag"])
    return tags_by_blob


def profile_records(entry: dict[str, Any]) -> list[dict[str, Any]]:
    profiles: list[dict[str, Any]] = []
    for profile in expected_profiles(entry["index"]):
        raw = {
            platform: reference(RAW_ROOT / entry["blob"] / platform / f"{profile}.json")
            for platform in PLATFORMS
        }
        profiles.append({"name": profile, "raw": raw})
    return profiles


def interpreter_bucket(blob: str) -> str:
    buckets = {
        read_json(RAW_ROOT / blob / platform / "default.json").get("interpreter_bucket")
        for platform in PLATFORMS
    }
    if len(buckets) != 1:
        raise ManifestError(f"raw fixtures disagree on interpreter bucket: {blob}")
    bucket = next(iter(buckets))
    if bucket not in {"cpython37", "cpython39"}:
        raise ManifestError(f"raw fixture has invalid interpreter bucket: {blob}")
    return bucket


def shared_with(
    *,
    entries: list[dict[str, Any]],
    position: int,
    deltas: list[dict[str, Any]],
) -> str | None:
    if position == 0:
        return None
    previous = entries[position - 1]
    current = entries[position]
    delta = deltas[position - 1]
    if delta.get("from_blob") != previous["blob"] or delta.get("to_blob") != current["blob"]:
        raise ManifestError(f"semantic-delta order mismatch at position {position}")
    current_profiles = expected_profiles(current["index"])
    previous_profiles = expected_profiles(previous["index"])
    profile_hashes_match = current_profiles == previous_profiles and all(
        raw_normalized_hash(current["blob"], platform, profile)
        == raw_normalized_hash(previous["blob"], platform, profile)
        for platform in PLATFORMS
        for profile in current_profiles
    )
    changes_empty = delta.get("changes") == []
    if changes_empty != profile_hashes_match:
        raise ManifestError(
            f"semantic delta and normalized-profile evidence disagree at position {position}"
        )
    return previous["blob"] if changes_empty else None


def inventory() -> list[dict[str, str]]:
    files = sorted(path for path in EVIDENCE_ROOT.rglob("*") if path.is_file() and path != OUTPUT)
    return [{"path": relative(path), "sha256": sha256(path)} for path in files]


def packaging_provenance() -> dict[str, str]:
    if not PACKAGING_PROVENANCE_PATH.is_file():
        return {"status": "pending_phase_7"}
    payload = read_json(PACKAGING_PROVENANCE_PATH)
    if (
        set(payload) != {"build", "launcher_chain", "schema", "schema_version", "source", "wheels"}
        or payload["schema"] != "service-legacy-packaging-provenance"
        or payload["schema_version"] != 1
    ):
        raise ManifestError("packaging provenance schema declaration is invalid")
    return reference(PACKAGING_PROVENANCE_PATH)


def main() -> int:
    head = git_head()
    follow = read_json(FOLLOW_PATH)
    tag = read_json(TAG_PATH)
    entries = follow.get("entries")
    if not isinstance(entries, list) or len(entries) != 44:
        raise ManifestError("follow census must contain exactly 44 entries")
    deltas = read_deltas()
    follow_ref = {**reference(FOLLOW_PATH), "count": len(entries)}
    tag_ref = {**reference(TAG_PATH), "count": len(tag["tags"])}
    tagged = release_statuses(tag, tag_ref["sha256"])
    blobs: list[dict[str, Any]] = []
    for position, entry in enumerate(entries):
        blob = entry["blob"]
        tags = tagged.get(blob, [])
        if tags:
            release_status: dict[str, Any] = {"kind": "tagged", "tags": tags}
        else:
            if position == len(entries) - 1:
                raise ManifestError("last follow entry cannot be unreleased without a successor")
            successor = entries[position + 1]
            release_status = {
                "distance": 1,
                "follow_position": position,
                "kind": "unreleased_superseded",
                "successor_blob": successor["blob"],
                "successor_commit": successor["commit"],
                "tag_census_sha256": tag_ref["sha256"],
                "tag_matches": [],
            }
        blobs.append(
            {
                "blob": blob,
                "commit": entry["commit"],
                "index": position,
                "interpreter_bucket": interpreter_bucket(blob),
                "negative": {
                    platform: reference(NEGATIVE_ROOT / blob / f"{platform}.json")
                    for platform in PLATFORMS
                },
                "normalized": {
                    platform: reference(NORMALIZED_ROOT / blob / f"{platform}.json")
                    for platform in PLATFORMS
                },
                "path": entry["path"],
                "profiles": profile_records(entry),
                "release_status": release_status,
                "shared_with": shared_with(entries=entries, position=position, deltas=deltas),
            }
        )
    if len(tagged) != 14 or sum(1 for blob in blobs if blob["release_status"]["kind"] == "tagged") != 14:
        raise ManifestError("tagged-blob census does not contain exactly 14 blobs")
    if sum(1 for blob in blobs if blob["release_status"]["kind"] == "unreleased_superseded") != 30:
        raise ManifestError("unreleased disposition census does not contain exactly 30 blobs")
    payload = {
        "blobs": blobs,
        "follow_census": follow_ref,
        "interpreters": reference(INTERPRETERS_PATH),
        "inventory": inventory(),
        "packaging_provenance": packaging_provenance(),
        "schema": SCHEMA,
        "schema_version": SCHEMA_VERSION,
        "semantic_deltas": {**reference(DELTAS_PATH), "count": len(deltas)},
        "source": {
            "commit": head,
            "tooling": [{"path": path, "sha256": sha256(ROOT / path)} for path in TOOLING_PATHS],
        },
        "tag_census": tag_ref,
    }
    OUTPUT.write_text(json.dumps(payload, indent=2, sort_keys=True, ensure_ascii=False) + "\n", encoding="utf-8")
    print(f"wrote manifest for {len(blobs)} blobs and {len(payload['inventory'])} fixture files")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
