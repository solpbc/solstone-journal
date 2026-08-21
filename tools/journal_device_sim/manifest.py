# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Fixture-manifest loading and validation for the journal device simulator."""

from __future__ import annotations

import hashlib
import json
import os
import re
import stat
from dataclasses import dataclass
from pathlib import Path
from typing import Any

SCHEMA = "solstone.journal-device-sim.fixtures.v1"
_DAY_RE = re.compile(r"^[0-9]{8}$")
_SEGMENT_RE = re.compile(r"^[0-9]{6}_[0-9]+$")
_SOURCE_RE = re.compile(r"^(?:[a-z0-9][a-z0-9_-]{0,63})?$")
_SHA256_RE = re.compile(r"^[0-9a-f]{64}$")
_RESERVED_NAMES = {
    "device.json",
    "events.jsonl",
    "ingest.json",
    "ingest.json.lock",
    "shape.json",
    "stream.json",
    "tombstone.json",
}
_UPLOAD_STATUSES = {"ok", "collision", "duplicate"}
_FILE_STATUSES = {"present", "processed", "missing"}
_VERIFICATION_SCOPES = {"contract", "custody", "processing"}
_PROCESSING_CONTRACT = {
    ".flac": "transcribe",
    ".m4a": "transcribe",
    ".mp3": "transcribe",
    ".ogg": "transcribe",
    ".opus": "transcribe",
    ".wav": "transcribe",
    ".mov": "describe",
    ".mp4": "describe",
    ".webm": "describe",
}


class ManifestError(ValueError):
    """The fixture manifest cannot safely drive an ingest run."""


@dataclass(frozen=True)
class FixtureFile:
    path: Path
    submitted: str
    sha256: str
    size: int
    device: int
    inode: int
    metadata: dict[str, Any]


@dataclass(frozen=True)
class ProcessingExpectation:
    input: str
    output: str
    handler: str


@dataclass(frozen=True)
class SegmentExpectation:
    upload_statuses: tuple[str, ...]
    file_statuses: tuple[str, ...]
    processing: tuple[ProcessingExpectation, ...]


@dataclass(frozen=True)
class FixtureSegment:
    fixture_id: str
    day: str
    segment: str
    source: str
    files: tuple[FixtureFile, ...]
    meta: dict[str, Any]
    expectation: SegmentExpectation


@dataclass(frozen=True)
class FixtureProfile:
    name: str
    segment_ids: tuple[str, ...]
    verify_duplicate: bool
    verification: str


@dataclass(frozen=True)
class FixtureManifest:
    path: Path
    root: Path
    digest: str
    segments: dict[str, FixtureSegment]
    profiles: dict[str, FixtureProfile]

    def profile_segments(self, name: str) -> tuple[FixtureSegment, ...]:
        try:
            profile = self.profiles[name]
        except KeyError as error:
            choices = ", ".join(sorted(self.profiles)) or "(none)"
            raise ManifestError(
                f"unknown profile {name!r}; available profiles: {choices}"
            ) from error
        return tuple(self.segments[item] for item in profile.segment_ids)


def _object(value: Any, where: str) -> dict[str, Any]:
    if not isinstance(value, dict):
        raise ManifestError(f"{where} must be an object")
    return value


def _string(value: Any, where: str) -> str:
    if not isinstance(value, str):
        raise ManifestError(f"{where} must be a string")
    return value


def _string_list(value: Any, where: str) -> tuple[str, ...]:
    if not isinstance(value, list) or not value:
        raise ManifestError(f"{where} must be a non-empty array of strings")
    result = tuple(_string(item, f"{where}[]") for item in value)
    if len(set(result)) != len(result):
        raise ManifestError(f"{where} contains duplicates")
    return result


def _digest_file(
    path: Path, expected: os.stat_result, where: str
) -> tuple[str, os.stat_result]:
    flags = os.O_RDONLY | getattr(os, "O_CLOEXEC", 0)
    flags |= getattr(os, "O_NOFOLLOW", 0)
    descriptor: int | None = None
    try:
        descriptor = os.open(path, flags)
        opened = os.fstat(descriptor)
        if (
            not stat.S_ISREG(opened.st_mode)
            or opened.st_dev != expected.st_dev
            or opened.st_ino != expected.st_ino
        ):
            raise ManifestError(f"{where} changed identity while it was loaded")
        digest = hashlib.sha256()
        with os.fdopen(descriptor, "rb") as handle:
            descriptor = None
            while chunk := handle.read(1024 * 1024):
                digest.update(chunk)
        return digest.hexdigest(), opened
    except ManifestError:
        raise
    except OSError as error:
        raise ManifestError(
            f"cannot read {where} safely: {type(error).__name__}"
        ) from error
    finally:
        if descriptor is not None:
            os.close(descriptor)


def _resolve_fixture_path(
    root: Path, relative: str, where: str
) -> tuple[Path, os.stat_result]:
    relative_path = Path(relative)
    if (
        relative_path.is_absolute()
        or not relative_path.parts
        or any(component in {"", ".", ".."} for component in relative_path.parts)
    ):
        raise ManifestError(f"{where} escapes the fixture root")
    candidate = root / relative_path
    try:
        current = root
        for index, component in enumerate(relative_path.parts):
            current = current / component
            metadata = current.lstat()
            if stat.S_ISLNK(metadata.st_mode):
                raise ManifestError(f"{where} cannot contain symlinks")
            final = index == len(relative_path.parts) - 1
            if not final and not stat.S_ISDIR(metadata.st_mode):
                raise ManifestError(f"{where} has a non-directory parent")
        if not stat.S_ISREG(metadata.st_mode):
            raise ManifestError(f"{where} is not a regular file: {candidate}")
        if candidate.resolve(strict=True) != candidate:
            raise ManifestError(f"{where} does not resolve to its lexical path")
    except ManifestError:
        raise
    except (OSError, RuntimeError) as error:
        raise ManifestError(
            f"cannot inspect {where}: {type(error).__name__}"
        ) from error
    return candidate, metadata


def _validate_submitted(name: str, where: str) -> None:
    if not name or len(name.encode("utf-8")) > 128:
        raise ManifestError(f"{where} must be 1..128 UTF-8 bytes")
    if (
        name in {".", ".."}
        or name != Path(name).name
        or "/" in name
        or "\\" in name
        or any(ord(character) < 32 or ord(character) == 127 for character in name)
    ):
        raise ManifestError(f"{where} must be a single safe filename")
    if name.lower() in _RESERVED_NAMES:
        raise ManifestError(f"{where} is journal-authored and cannot be submitted")
    if '"' in name:
        raise ManifestError(
            f"{where} cannot be represented safely in multipart headers"
        )


def _parse_file(raw: Any, root: Path, where: str) -> FixtureFile:
    value = _object(raw, where)
    relative = _string(value.get("path"), f"{where}.path")
    submitted = _string(value.get("submitted"), f"{where}.submitted")
    _validate_submitted(submitted, f"{where}.submitted")
    expected_sha = _string(value.get("sha256"), f"{where}.sha256")
    if not _SHA256_RE.fullmatch(expected_sha):
        raise ManifestError(f"{where}.sha256 must be lowercase SHA-256 hex")
    expected_size = value.get("size")
    if (
        not isinstance(expected_size, int)
        or isinstance(expected_size, bool)
        or expected_size < 0
    ):
        raise ManifestError(f"{where}.size must be a non-negative integer")
    metadata = value.get("metadata", {})
    metadata = _object(metadata, f"{where}.metadata")
    path, path_metadata = _resolve_fixture_path(root, relative, f"{where}.path")
    actual_sha, opened_metadata = _digest_file(
        path, path_metadata, f"{where}.path"
    )
    actual_size = opened_metadata.st_size
    if actual_size != expected_size:
        raise ManifestError(
            f"{where}.size is {expected_size}, but {path} is {actual_size} bytes"
        )
    if actual_sha != expected_sha:
        raise ManifestError(
            f"{where}.sha256 is {expected_sha}, but {path} hashes to {actual_sha}"
        )
    return FixtureFile(
        path=path,
        submitted=submitted,
        sha256=expected_sha,
        size=expected_size,
        device=opened_metadata.st_dev,
        inode=opened_metadata.st_ino,
        metadata=dict(metadata),
    )


def _parse_expectation(
    raw: Any, where: str, files: tuple[FixtureFile, ...]
) -> SegmentExpectation:
    value = _object(raw if raw is not None else {}, where)
    upload_statuses = tuple(value.get("upload_statuses", ["ok", "collision"]))
    file_statuses = tuple(value.get("file_statuses", ["present", "processed"]))
    if not upload_statuses or any(
        not isinstance(item, str) or item not in _UPLOAD_STATUSES
        for item in upload_statuses
    ):
        raise ManifestError(
            f"{where}.upload_statuses must use ok, collision, or duplicate"
        )
    if not file_statuses or any(
        not isinstance(item, str) or item not in _FILE_STATUSES
        for item in file_statuses
    ):
        raise ManifestError(
            f"{where}.file_statuses must use present, processed, or missing"
        )
    raw_processing = value.get("processing", [])
    if not isinstance(raw_processing, list):
        raise ManifestError(f"{where}.processing must be an array")
    submitted = {item.submitted for item in files}
    processing: list[ProcessingExpectation] = []
    for index, raw_item in enumerate(raw_processing):
        item_where = f"{where}.processing[{index}]"
        item = _object(raw_item, item_where)
        input_name = _string(item.get("input"), f"{item_where}.input")
        output = _string(item.get("output"), f"{item_where}.output")
        handler = _string(item.get("handler"), f"{item_where}.handler")
        if input_name not in submitted:
            raise ManifestError(f"{item_where}.input must name a submitted file")
        contract = _PROCESSING_CONTRACT.get(Path(input_name).suffix.lower())
        if contract is None:
            raise ManifestError(f"{item_where}.input has no processing contract")
        expected_handler = contract
        expected_output = Path(input_name).with_suffix(".jsonl").name
        if handler != expected_handler or output != expected_output:
            raise ManifestError(
                f"{item_where} must map {input_name} to {expected_handler}/{expected_output}"
            )
        if output in submitted or output.lower() in _RESERVED_NAMES:
            raise ManifestError(f"{item_where}.output must be a derived filename")
        processing.append(
            ProcessingExpectation(
                input=input_name,
                output=output,
                handler=handler,
            )
        )
    inputs = [item.input for item in processing]
    outputs = [item.output for item in processing]
    if len(set(inputs)) != len(inputs) or len(set(outputs)) != len(outputs):
        raise ManifestError(f"{where}.processing cannot alias inputs or outputs")
    return SegmentExpectation(
        upload_statuses=upload_statuses,
        file_statuses=file_statuses,
        processing=tuple(processing),
    )


def _parse_segment(raw: Any, root: Path, index: int) -> FixtureSegment:
    where = f"segments[{index}]"
    value = _object(raw, where)
    fixture_id = _string(value.get("id"), f"{where}.id")
    if not fixture_id or any(character.isspace() for character in fixture_id):
        raise ManifestError(f"{where}.id must be a non-empty token")
    day = _string(value.get("day"), f"{where}.day")
    if not _DAY_RE.fullmatch(day):
        raise ManifestError(f"{where}.day must be YYYYMMDD")
    segment = _string(value.get("segment"), f"{where}.segment")
    if not _SEGMENT_RE.fullmatch(segment):
        raise ManifestError(f"{where}.segment must be HHMMSS_LEN")
    source = _string(value.get("source", ""), f"{where}.source")
    if not _SOURCE_RE.fullmatch(source):
        raise ManifestError(
            f"{where}.source must be <=64 lowercase ASCII letters/digits/_/-"
        )
    raw_files = value.get("files")
    if not isinstance(raw_files, list) or not raw_files or len(raw_files) > 8:
        raise ManifestError(f"{where}.files must contain 1..8 entries")
    files = tuple(
        _parse_file(item, root, f"{where}.files[{file_index}]")
        for file_index, item in enumerate(raw_files)
    )
    submitted = [item.submitted for item in files]
    if len(set(submitted)) != len(submitted):
        raise ManifestError(f"{where}.files contains duplicate submitted names")
    meta = _object(value.get("meta", {}), f"{where}.meta")
    return FixtureSegment(
        fixture_id=fixture_id,
        day=day,
        segment=segment,
        source=source,
        files=files,
        meta=dict(meta),
        expectation=_parse_expectation(
            value.get("expect"), f"{where}.expect", files
        ),
    )


def _parse_profile(
    name: str, raw: Any, segments: dict[str, FixtureSegment]
) -> FixtureProfile:
    where = f"profiles.{name}"
    value = _object(raw, where)
    segment_ids = _string_list(value.get("segments"), f"{where}.segments")
    missing = [item for item in segment_ids if item not in segments]
    if missing:
        raise ManifestError(
            f"{where}.segments names unknown fixtures: {', '.join(missing)}"
        )
    verify_duplicate = value.get("verify_duplicate", False)
    if not isinstance(verify_duplicate, bool):
        raise ManifestError(f"{where}.verify_duplicate must be boolean")
    verification = value.get("verification")
    if verification not in _VERIFICATION_SCOPES:
        raise ManifestError(
            f"{where}.verification must be contract, custody, or processing"
        )
    if verification == "processing":
        for segment_id in segment_ids:
            segment = segments[segment_id]
            if len(segment.expectation.processing) != len(segment.files):
                raise ManifestError(
                    f"{where} requires one processing oracle per submitted file in {segment_id}"
                )
    return FixtureProfile(
        name=name,
        segment_ids=segment_ids,
        verify_duplicate=verify_duplicate,
        verification=verification,
    )


def load_manifest(path: Path, fixture_root: Path | None = None) -> FixtureManifest:
    """Load, validate, and hash every byte a fixture run may submit."""

    manifest_path = path.resolve()
    try:
        manifest_bytes = manifest_path.read_bytes()
    except OSError as error:
        raise ManifestError(f"cannot read manifest {manifest_path}: {error}") from error
    try:
        raw = json.loads(manifest_bytes)
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise ManifestError(f"manifest is not valid UTF-8 JSON: {error}") from error
    value = _object(raw, "manifest")
    if value.get("schema") != SCHEMA:
        raise ManifestError(f"manifest.schema must equal {SCHEMA!r}")
    root = (fixture_root or manifest_path.parent).resolve()
    if not root.is_dir():
        raise ManifestError(f"fixture root is not a directory: {root}")
    raw_segments = value.get("segments")
    if not isinstance(raw_segments, list) or not raw_segments:
        raise ManifestError("manifest.segments must be a non-empty array")
    parsed_segments = [
        _parse_segment(item, root, index) for index, item in enumerate(raw_segments)
    ]
    segment_ids = [item.fixture_id for item in parsed_segments]
    if len(set(segment_ids)) != len(segment_ids):
        raise ManifestError("manifest.segments contains duplicate ids")
    segments = {item.fixture_id: item for item in parsed_segments}
    raw_profiles = _object(value.get("profiles"), "manifest.profiles")
    if not raw_profiles:
        raise ManifestError("manifest.profiles must not be empty")
    profiles = {
        _string(name, "manifest.profiles key"): _parse_profile(name, profile, segments)
        for name, profile in raw_profiles.items()
    }
    return FixtureManifest(
        path=manifest_path,
        root=root,
        digest=hashlib.sha256(manifest_bytes).hexdigest(),
        segments=segments,
        profiles=profiles,
    )
