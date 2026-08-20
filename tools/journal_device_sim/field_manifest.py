# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Build an explicit simulator manifest from the checked-in field journal corpus."""

from __future__ import annotations

import hashlib
import json
import subprocess
from pathlib import Path
from typing import Any

from .manifest import SCHEMA, ManifestError

_AUDIO_EXTENSIONS = {".flac", ".m4a", ".ogg", ".opus", ".wav"}
_SCREEN_EXTENSIONS = {".mp4", ".mov", ".webm"}


def _sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        while chunk := handle.read(1024 * 1024):
            digest.update(chunk)
    return digest.hexdigest()


def _tracked_files(root: Path) -> set[str]:
    try:
        result = subprocess.run(
            [
                "git",
                "-C",
                str(root),
                "ls-files",
                "-z",
                "--",
                "manifest.json",
                "journal",
            ],
            check=False,
            capture_output=True,
            timeout=30,
        )
    except (OSError, subprocess.TimeoutExpired) as error:
        raise ManifestError(
            f"cannot inventory the field journal Git tree: {error}"
        ) from error
    if result.returncode != 0:
        raise ManifestError(
            "field journal fixture root must be a readable Git worktree"
        )
    return {item.decode("utf-8") for item in result.stdout.split(b"\x00") if item}


def _field_revision(root: Path) -> str:
    result = subprocess.run(
        ["git", "-C", str(root), "rev-parse", "HEAD"],
        check=False,
        capture_output=True,
        text=True,
        timeout=10,
    )
    if result.returncode != 0 or not result.stdout.strip():
        raise ManifestError("cannot resolve the field journal fixture revision")
    return result.stdout.strip()


def _require_clean_inputs(root: Path, relative_paths: set[str]) -> None:
    result = subprocess.run(
        [
            "git",
            "-C",
            str(root),
            "diff",
            "--quiet",
            "HEAD",
            "--",
            *sorted(relative_paths),
        ],
        check=False,
        capture_output=True,
        timeout=30,
    )
    if result.returncode == 1:
        raise ManifestError(
            "field journal manifest or selected raw fixture bytes differ from HEAD"
        )
    if result.returncode != 0:
        raise ManifestError("cannot verify field journal fixture provenance")


def _raw_files(
    root: Path, tracked: set[str], day: str, stream: str, segment: str
) -> list[Path]:
    directory = root / "journal" / day / stream / segment
    if stream == "field.audio":
        prefix = "audio"
        extensions = _AUDIO_EXTENSIONS
    elif stream == "field.screen":
        prefix = "screen"
        extensions = _SCREEN_EXTENSIONS
    else:
        raise ManifestError(f"unsupported field stream {stream!r}")
    candidates = sorted(
        path
        for path in directory.glob(f"{prefix}.*")
        if path.is_file() and path.suffix.lower() in extensions
    )
    if not candidates:
        raise ManifestError(
            f"field segment {day}/{stream}/{segment} has no raw fixture"
        )
    if len(candidates) > 8:
        raise ManifestError(
            f"field segment {day}/{stream}/{segment} exceeds 8 raw files"
        )
    for path in candidates:
        relative = path.relative_to(root).as_posix()
        if relative not in tracked:
            raise ManifestError(
                f"field raw fixture is not tracked and cannot enter a manifest: {relative}"
            )
    return candidates


def _select_smoke(segments: list[dict[str, Any]]) -> list[str]:
    selected: list[str] = []
    seen_sources: set[str] = set()
    seen_extensions: set[str] = set()
    seen_streams: set[str] = set()
    for segment in segments:
        source = str(segment["meta"]["field_source"])
        extensions = {Path(item["path"]).suffix for item in segment["files"]}
        stream = str(segment["meta"]["field_stream"])
        if (
            source not in seen_sources
            or not extensions.issubset(seen_extensions)
            or stream not in seen_streams
        ):
            selected.append(str(segment["id"]))
            seen_sources.add(source)
            seen_extensions.update(extensions)
            seen_streams.add(stream)
    return selected


def build_field_manifest(root: Path) -> dict[str, Any]:
    """Return exact simulator fixtures from field_journal's manifest and tracked raw files."""

    field_root = root.resolve()
    manifest_path = field_root / "manifest.json"
    try:
        manifest_bytes = manifest_path.read_bytes()
        source_manifest = json.loads(manifest_bytes)
    except (OSError, UnicodeDecodeError, json.JSONDecodeError) as error:
        raise ManifestError(f"cannot read field journal manifest: {error}") from error
    if not isinstance(source_manifest, dict) or source_manifest.get("version") != 1:
        raise ManifestError("field journal manifest must be a version-1 object")
    raw_segments = source_manifest.get("segments")
    if not isinstance(raw_segments, list) or not raw_segments:
        raise ManifestError("field journal manifest has no segments")
    tracked = _tracked_files(field_root)
    if "manifest.json" not in tracked:
        raise ManifestError("field journal manifest itself is not tracked")
    segments: list[dict[str, Any]] = []
    selected_paths = {"manifest.json"}
    for index, raw in enumerate(raw_segments):
        if not isinstance(raw, dict):
            raise ManifestError(f"field manifest segments[{index}] is not an object")
        try:
            day = str(raw["day"])
            stream = str(raw["stream"])
            segment = str(raw["segment"])
            field_source = str(raw["source"])
        except KeyError as error:
            raise ManifestError(
                f"field manifest segments[{index}] omits {error.args[0]}"
            ) from error
        files = _raw_files(field_root, tracked, day, stream, segment)
        selected_paths.update(path.relative_to(field_root).as_posix() for path in files)
        device_source = "audio" if stream == "field.audio" else "screen"
        required_output = "audio.jsonl" if device_source == "audio" else "screen.jsonl"
        fixture_id = f"{day}-{device_source}-{segment}-{field_source}"
        metadata = {
            "field_source": field_source,
            "field_stream": stream,
            "field_source_id": raw.get("source_id"),
            "license": raw.get("license"),
            "description": raw.get("description"),
            "exercises": raw.get("exercises", []),
            "has_reference": raw.get("has_reference", False),
        }
        segments.append(
            {
                "id": fixture_id,
                "day": day,
                "segment": segment,
                "source": device_source,
                "files": [
                    {
                        "path": path.relative_to(field_root).as_posix(),
                        "submitted": path.name,
                        "size": path.stat().st_size,
                        "sha256": _sha256(path),
                    }
                    for path in files
                ],
                "meta": metadata,
                "expect": {
                    "upload_statuses": ["ok"],
                    "file_statuses": ["present", "processed"],
                    "required_outputs": [required_output],
                },
            }
        )
    _require_clean_inputs(field_root, selected_paths)
    canary_id = "20260201-audio-080000_600-chime6"
    canary = next((segment for segment in segments if segment["id"] == canary_id), None)
    if canary is None:
        raise ManifestError(
            f"field journal is missing the large-body canary {canary_id}"
        )
    canary_files = canary["files"]
    if len(canary_files) != 1 or canary_files[0]["submitted"] != "audio.wav":
        raise ManifestError("field journal large-body canary must be exactly audio.wav")
    if canary_files[0]["size"] != 19_200_078:
        raise ManifestError(
            "field journal large-body canary must remain exactly 19,200,078 bytes"
        )
    canary_path = field_root / canary_files[0]["path"]
    with canary_path.open("rb") as handle:
        riff_magic = handle.read(4)
    if riff_magic != b"RIFF":
        raise ManifestError("field journal large-body canary is not a RIFF WAV")
    return {
        "schema": SCHEMA,
        "fixture": {
            "kind": "field_journal",
            "revision": _field_revision(field_root),
            "manifest_sha256": hashlib.sha256(manifest_bytes).hexdigest(),
        },
        "profiles": {
            "field-smoke": {
                "segments": _select_smoke(segments),
                "verify_duplicate": False,
                "verify_processing": True,
            },
            "field-large": {
                "segments": [canary_id],
                "verify_duplicate": True,
                "verify_processing": False,
            },
            "field-full": {
                "segments": [segment["id"] for segment in segments],
                "verify_duplicate": False,
                "verify_processing": True,
            },
        },
        "segments": segments,
    }
