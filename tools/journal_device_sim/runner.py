# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Resumable fixture upload, reconciliation, and evidence generation."""

from __future__ import annotations

import hashlib
import json
import math
import os
import re
import stat
import subprocess
import time
import uuid
from contextlib import ExitStack, contextmanager
from dataclasses import dataclass
from datetime import date, datetime, timedelta, timezone
from enum import Enum
from pathlib import Path
from typing import Any, Callable, Iterator

from .http_client import (
    BridgeHttpClient,
    HttpRequestError,
    HttpResponseError,
    HttpResponse,
    MultipartUpload,
)
from .manifest import (
    FixtureFile,
    FixtureManifest,
    FixtureSegment,
    ManifestError,
    ProcessingExpectation,
)
from .process import LinkBridge, LinkProcessError

STATE_SCHEMA = "solstone.journal-device-sim.state.v1"
EVIDENCE_SCHEMA = "solstone.journal-device-sim.evidence.v2"
_SEGMENT_KEY_RE = re.compile(r"^[0-9]{6}_[0-9]+$")
_SHA256_RE = re.compile(r"^[0-9a-f]{64}$")
_RUN_ID_RE = re.compile(r"^[0-9a-f]{32}$")
_AUDIO_START_RE = re.compile(r"^(?:[01][0-9]|2[0-3]):[0-5][0-9]:[0-5][0-9]$")
_STATE_PHASES = {
    "sending",
    "uncertain",
    "accepted",
    "reconciled",
    "complete",
    "contract_failed",
}
_FILE_STATUSES = {"present", "processed", "missing"}
_MAX_LOCAL_JSON_BYTES = 4 * 1024 * 1024
_MAX_LOCAL_JSONL_BYTES = 64 * 1024 * 1024
_DESCRIBE_PRIMARY = {
    "browsing",
    "calendar",
    "code",
    "gaming",
    "media",
    "meeting",
    "messaging",
    "productivity",
    "reading",
    "social",
    "terminal",
}
_DESCRIBE_SECONDARY = _DESCRIBE_PRIMARY | {"none"}
_RECEIVER_STATUS_EVIDENCE_FIELDS = {"instance_id", "posture", "reason_code"}


class RunOutcome(str, Enum):
    PASS = "PASS"
    FAIL = "FAIL"
    BLOCKED = "BLOCKED"
    INCONCLUSIVE = "INCONCLUSIVE"


class SimulationFailure(RuntimeError):
    """A product or fixture assertion failed."""


class SimulationInconclusive(RuntimeError):
    """The simulator could not determine whether bytes landed."""


@dataclass(frozen=True)
class SimulatorConfig:
    manifest: FixtureManifest
    profile: str
    carrier: str
    state_dir: Path
    evidence_path: Path
    bridge_url: str | None = None
    pair_code: str | None = None
    paired: bool = False
    solstone_bin: str | None = None
    relay_url: str | None = None
    convey_port: int | None = None
    date_mode: str = "shift"
    anchor_day: str | None = None
    journal_root: Path | None = None
    expected_cid: str | None = None
    request_timeout: float = 90.0
    processing_timeout: float = 0.0
    poll_interval: float = 1.0
    max_attempts: int = 3
    keep_credentials: bool = False


def _utc_now() -> str:
    return datetime.now(timezone.utc).isoformat().replace("+00:00", "Z")


def _atomic_json(path: Path, value: dict[str, Any], mode: int = 0o600) -> None:
    path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    temporary = path.with_name(f".{path.name}.{uuid.uuid4().hex}.tmp")
    encoded = json.dumps(value, indent=2, sort_keys=True, ensure_ascii=True) + "\n"
    descriptor = os.open(temporary, os.O_WRONLY | os.O_CREAT | os.O_EXCL, mode)
    try:
        with os.fdopen(descriptor, "w", encoding="utf-8") as handle:
            handle.write(encoded)
            handle.flush()
            os.fsync(handle.fileno())
        os.replace(temporary, path)
    finally:
        if temporary.exists():
            temporary.unlink()


def _parse_day(raw: str, where: str) -> date:
    try:
        return datetime.strptime(raw, "%Y%m%d").date()
    except ValueError as error:
        raise SimulationFailure(f"{where} must be YYYYMMDD") from error


def build_day_map(
    segments: tuple[FixtureSegment, ...], date_mode: str, anchor_day: str | None
) -> dict[str, str]:
    days = sorted({_parse_day(segment.day, "fixture day") for segment in segments})
    if date_mode == "preserve":
        return {day.strftime("%Y%m%d"): day.strftime("%Y%m%d") for day in days}
    if date_mode != "shift":
        raise SimulationFailure("date mode must be shift or preserve")
    anchor = _parse_day(anchor_day, "anchor day") if anchor_day else date.today()
    last = days[-1]
    return {
        day.strftime("%Y%m%d"): (anchor + timedelta(days=(day - last).days)).strftime(
            "%Y%m%d"
        )
        for day in days
    }


def _git_revision(path: Path) -> str | None:
    try:
        result = subprocess.run(
            ["git", "-C", str(path), "rev-parse", "HEAD"],
            check=False,
            capture_output=True,
            text=True,
            timeout=5,
        )
    except (OSError, subprocess.TimeoutExpired):
        return None
    if result.returncode != 0:
        return None
    return result.stdout.strip() or None


def _git_dirty(path: Path) -> bool | None:
    try:
        result = subprocess.run(
            ["git", "-C", str(path), "status", "--porcelain", "--", "."],
            check=False,
            capture_output=True,
            text=True,
            timeout=5,
        )
    except (OSError, subprocess.TimeoutExpired):
        return None
    if result.returncode != 0:
        return None
    return bool(result.stdout)


def _simulator_code_provenance() -> dict[str, Any]:
    module_root = Path(__file__).resolve().parent
    repo_root = module_root.parents[1]
    sources = sorted(path for path in module_root.glob("*.py") if path.is_file())
    digest = hashlib.sha256()
    names: list[str] = []
    for path in sources:
        name = path.relative_to(repo_root).as_posix()
        names.append(name)
        digest.update(name.encode("utf-8"))
        digest.update(b"\0")
        digest.update(path.read_bytes())
        digest.update(b"\0")
    return {
        "revision": _git_revision(repo_root),
        "dirty": _git_dirty(module_root),
        "source_sha256": digest.hexdigest(),
        "sources": names,
    }


def _plain_descendant(root: Path, path: Path, *, directory: bool) -> bool:
    """Return whether path is a confined, symlink-free descendant of root."""

    try:
        relative = path.relative_to(root)
    except ValueError:
        return False
    if not relative.parts:
        return False
    current = root
    try:
        for index, component in enumerate(relative.parts):
            current = current / component
            metadata = current.lstat()
            if stat.S_ISLNK(metadata.st_mode):
                return False
            final = index == len(relative.parts) - 1
            if final:
                wanted = (
                    stat.S_ISDIR(metadata.st_mode)
                    if directory
                    else stat.S_ISREG(metadata.st_mode)
                )
                if not wanted:
                    return False
            elif not stat.S_ISDIR(metadata.st_mode):
                return False
        return current.resolve(strict=True).is_relative_to(root)
    except (OSError, RuntimeError):
        return False


def _paths_overlap(left: Path, right: Path) -> bool:
    """Return whether either canonical path contains the other."""

    return left == right or left.is_relative_to(right) or right.is_relative_to(left)


def _safe_content_name(value: Any) -> bool:
    """Return whether value is one bounded, path-free journal content name."""

    if not isinstance(value, str):
        return False
    try:
        encoded_size = len(value.encode("utf-8"))
    except UnicodeEncodeError:
        return False
    return (
        1 <= encoded_size <= 128
        and value not in {".", ".."}
        and value == Path(value).name
        and "/" not in value
        and "\\" not in value
        and not any(ord(character) < 32 or ord(character) == 127 for character in value)
    )


def _confined_bytes(root: Path, path: Path, limit: int, label: str) -> bytes:
    """Open a root-relative regular file once and read at most limit+1 bytes."""

    try:
        relative = path.relative_to(root)
    except ValueError as error:
        raise SimulationFailure(f"{label} escapes its root") from error
    if not relative.parts or any(part in {"", ".", ".."} for part in relative.parts):
        raise SimulationFailure(f"{label} is not a confined path")
    directory_flags = os.O_RDONLY | getattr(os, "O_CLOEXEC", 0)
    directory_flags |= getattr(os, "O_DIRECTORY", 0)
    directory_flags |= getattr(os, "O_NOFOLLOW", 0)
    file_flags = os.O_RDONLY | getattr(os, "O_CLOEXEC", 0)
    file_flags |= getattr(os, "O_NOFOLLOW", 0)
    descriptors: list[int] = []
    try:
        current = os.open(root, directory_flags)
        descriptors.append(current)
        for component in relative.parts[:-1]:
            current = os.open(component, directory_flags, dir_fd=current)
            descriptors.append(current)
        file_descriptor = os.open(relative.parts[-1], file_flags, dir_fd=current)
        descriptors.append(file_descriptor)
        metadata = os.fstat(file_descriptor)
        if not stat.S_ISREG(metadata.st_mode):
            raise SimulationFailure(f"{label} must be a confined regular file")
        if metadata.st_size > limit:
            raise SimulationFailure(f"{label} exceeds its {limit}-byte bound")
        chunks: list[bytes] = []
        remaining = limit + 1
        while remaining:
            chunk = os.read(file_descriptor, min(1024 * 1024, remaining))
            if not chunk:
                break
            chunks.append(chunk)
            remaining -= len(chunk)
        raw = b"".join(chunks)
        if len(raw) > limit:
            raise SimulationFailure(f"{label} exceeds its {limit}-byte bound")
        return raw
    except SimulationFailure:
        raise
    except OSError as error:
        raise SimulationFailure(f"{label} cannot be opened safely") from error
    finally:
        for descriptor in reversed(descriptors):
            try:
                os.close(descriptor)
            except OSError:
                pass


def _confined_digest(
    root: Path,
    path: Path,
    expected_size: int,
    expected_sha256: str,
    label: str,
) -> None:
    """Stream and verify one root-relative regular file without buffering it."""

    try:
        relative = path.relative_to(root)
    except ValueError as error:
        raise SimulationFailure(f"{label} escapes its root") from error
    if not relative.parts or any(part in {"", ".", ".."} for part in relative.parts):
        raise SimulationFailure(f"{label} is not a confined path")
    directory_flags = os.O_RDONLY | getattr(os, "O_CLOEXEC", 0)
    directory_flags |= getattr(os, "O_DIRECTORY", 0)
    directory_flags |= getattr(os, "O_NOFOLLOW", 0)
    file_flags = os.O_RDONLY | getattr(os, "O_CLOEXEC", 0)
    file_flags |= getattr(os, "O_NOFOLLOW", 0)
    descriptors: list[int] = []
    try:
        current = os.open(root, directory_flags)
        descriptors.append(current)
        for component in relative.parts[:-1]:
            current = os.open(component, directory_flags, dir_fd=current)
            descriptors.append(current)
        file_descriptor = os.open(relative.parts[-1], file_flags, dir_fd=current)
        descriptors.append(file_descriptor)
        opened = os.fstat(file_descriptor)
        if not stat.S_ISREG(opened.st_mode):
            raise SimulationFailure(f"{label} must be a confined regular file")
        if opened.st_size != expected_size:
            raise SimulationFailure(f"{label} does not match fixture bytes")
        digest = hashlib.sha256()
        byte_count = 0
        while chunk := os.read(file_descriptor, 1024 * 1024):
            byte_count += len(chunk)
            if byte_count > expected_size:
                raise SimulationFailure(f"{label} does not match fixture bytes")
            digest.update(chunk)
        closed = os.fstat(file_descriptor)
        if (
            byte_count != expected_size
            or digest.hexdigest() != expected_sha256
            or closed.st_dev != opened.st_dev
            or closed.st_ino != opened.st_ino
            or closed.st_size != opened.st_size
        ):
            raise SimulationFailure(f"{label} does not match fixture bytes")
    except SimulationFailure:
        raise
    except OSError as error:
        raise SimulationFailure(f"{label} cannot be opened safely") from error
    finally:
        for descriptor in reversed(descriptors):
            try:
                os.close(descriptor)
            except OSError:
                pass


def _strict_json(raw: bytes, label: str) -> Any:
    def reject_constant(value: str) -> None:
        raise ValueError(f"non-finite JSON constant {value}")

    try:
        return json.loads(raw, parse_constant=reject_constant)
    except (UnicodeDecodeError, json.JSONDecodeError, ValueError, RecursionError) as error:
        raise SimulationFailure(f"{label} is not valid bounded JSON") from error


def _valid_audio_semantic_row(row: dict[str, Any]) -> bool:
    sentence_id = row.get("sentence_id")
    source = row.get("source")
    speaker = row.get("speaker")
    speaker_ok = isinstance(speaker, str) or (
        isinstance(speaker, int)
        and not isinstance(speaker, bool)
        and -(2**63) <= speaker <= (2**64 - 1)
    )
    return (
        isinstance(row.get("start"), str)
        and _AUDIO_START_RE.fullmatch(row["start"]) is not None
        and isinstance(row.get("text"), str)
        and isinstance(sentence_id, int)
        and not isinstance(sentence_id, bool)
        and 1 <= sentence_id <= 2_147_483_647
        and (source is None or (isinstance(source, str) and bool(source)))
        and (speaker is None or speaker_ok)
    )


def _finite_number(value: Any, *, nonnegative: bool = False) -> bool:
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        return False
    try:
        normalized = float(value)
    except (OverflowError, ValueError):
        return False
    return math.isfinite(normalized) and (not nonnegative or normalized >= 0.0)


def _valid_describe_request(value: Any, *, first: bool) -> bool:
    if not isinstance(value, dict):
        return False
    duration = value.get("duration")
    retries = value.get("retries")
    expected_type = "describe" if first else "category"
    return (
        value.get("type") == expected_type
        and isinstance(value.get("model"), str)
        and _finite_number(duration, nonnegative=True)
        and (
            retries is None
            or (
                isinstance(retries, int)
                and not isinstance(retries, bool)
                and 1 <= retries <= 4
            )
        )
        and (
            (first and "category" not in value)
            or (not first and value.get("category") in _DESCRIBE_PRIMARY)
        )
    )


def _valid_describe_semantic_row(row: dict[str, Any]) -> bool:
    frame_id = row.get("frame_id")
    timestamp = row.get("timestamp")
    analysis = row.get("analysis")
    requests = row.get("requests")
    return (
        isinstance(frame_id, int)
        and not isinstance(frame_id, bool)
        and 1 <= frame_id <= (2**64 - 1)
        and _finite_number(timestamp)
        and isinstance(analysis, dict)
        and set(analysis) == {
            "visual_description",
            "primary",
            "secondary",
            "overlap",
        }
        and isinstance(analysis.get("visual_description"), str)
        and analysis.get("primary") in _DESCRIBE_PRIMARY
        and analysis.get("secondary") in _DESCRIBE_SECONDARY
        and isinstance(analysis.get("overlap"), bool)
        and isinstance(requests, list)
        and bool(requests)
        and all(
            _valid_describe_request(request, first=index == 0)
            for index, request in enumerate(requests)
        )
        and isinstance(row.get("enhanced"), bool)
        and (
            (
                row["enhanced"] is True
                and isinstance(row.get("content"), dict)
                and bool(row["content"])
            )
            or (row["enhanced"] is False and "content" not in row)
        )
        and (
            "detections" not in row or isinstance(row.get("detections"), dict)
        )
        and "error" not in row
    )


def _bounded_json_file(root: Path, path: Path, label: str) -> dict[str, Any]:
    value = _strict_json(
        _confined_bytes(root, path, _MAX_LOCAL_JSON_BYTES, label), label
    )
    if not isinstance(value, dict):
        raise SimulationFailure(f"{label} must contain a JSON object")
    return value


def _bounded_jsonl_data(
    root: Path, path: Path, label: str
) -> tuple[list[dict[str, Any]], str]:
    raw = _confined_bytes(root, path, _MAX_LOCAL_JSONL_BYTES, label)
    rows: list[dict[str, Any]] = []
    for index, line in enumerate(raw.splitlines(), 1):
        if not line.strip():
            continue
        value = _strict_json(line, f"{label} line {index}")
        if not isinstance(value, dict):
            raise SimulationFailure(f"{label} line {index} must be an object")
        rows.append(value)
    if not rows:
        raise SimulationFailure(f"{label} must contain at least one object")
    return rows, hashlib.sha256(raw).hexdigest()


def _bounded_jsonl_file(root: Path, path: Path, label: str) -> list[dict[str, Any]]:
    return _bounded_jsonl_data(root, path, label)[0]


def _bounded_event_file(
    root: Path, path: Path, label: str
) -> tuple[list[dict[str, Any]], dict[str, int]]:
    """Mirror the durable-event reader's skip-torn/unknown-row recovery boundary."""

    raw = _confined_bytes(root, path, _MAX_LOCAL_JSONL_BYTES, label)
    rows: list[dict[str, Any]] = []
    total = 0
    unparseable = 0
    unrecognized = 0
    wrong_family = 0

    def device_ingest_shape(value: dict[str, Any]) -> bool:
        cid = value.get("cid", value.get("did"))
        integer_fields = (value.get("record_version"), value.get("protocol_version"))
        files = value.get("files")
        if (
            any(isinstance(item, bool) or not isinstance(item, int) or not 0 <= item <= 255 for item in integer_fields)
            or not isinstance(cid, str)
            or any(
                not isinstance(value.get(name), str)
                for name in ("record_type", "outcome", "source", "stream", "day", "segment")
            )
            or not isinstance(files, list)
            or not isinstance(value.get("meta"), dict)
        ):
            return False
        return all(
            isinstance(item, dict)
            and isinstance(item.get("submitted"), str)
            and isinstance(item.get("written"), str)
            and isinstance(item.get("size"), int)
            and not isinstance(item.get("size"), bool)
            and item["size"] >= 0
            and isinstance(item.get("sha256"), str)
            for item in files
        )

    for line in raw.splitlines():
        if not line.strip():
            continue
        total += 1
        try:
            value = _strict_json(line, label)
        except SimulationFailure:
            unparseable += 1
            continue
        if not isinstance(value, dict):
            unrecognized += 1
            continue
        if value.get("record_type") == "device_ingest":
            if device_ingest_shape(value):
                rows.append(value)
            else:
                unparseable += 1
            continue
        if isinstance(value.get("tract"), str) and isinstance(
            value.get("event"), str
        ):
            wrong_family += 1
            continue
        unrecognized += 1
    return rows, {
        "total_rows": total,
        "device_ingest_rows": len(rows),
        "wrong_family": wrong_family,
        "unparseable": unparseable,
        "unrecognized": unrecognized,
    }


def _journal_state_identity(root: Path) -> tuple[str, str]:
    """Load the journal's public state identity with product legacy-first precedence."""

    legacy = root / "link" / "state.json"
    native = root / "link" / "ca" / "state.json"
    selected = legacy if os.path.lexists(legacy) else native
    relative = selected.relative_to(root).as_posix()
    value = _bounded_json_file(root, selected, f"journal identity state {relative}")
    instance_id = value.get("instance_id")
    if not isinstance(instance_id, str) or not instance_id.strip():
        raise SimulationFailure(
            f"journal identity state {relative} has no nonempty instance_id"
        )
    return relative, instance_id


class Simulator:
    """Drive one manifest profile through a real local link bridge."""

    def __init__(self, config: SimulatorConfig) -> None:
        if config.carrier not in {"direct", "relay"}:
            raise ManifestError("carrier must be direct or relay")
        connection_modes = sum(
            (bool(config.bridge_url), bool(config.pair_code), config.paired)
        )
        if connection_modes != 1:
            raise ManifestError(
                "provide exactly one of bridge_url, pair_code, or paired"
            )
        if config.convey_port is not None and not 1 <= config.convey_port <= 65535:
            raise ManifestError("convey_port must be an integer from 1 to 65535")
        if config.bridge_url and config.convey_port is not None:
            raise ManifestError("convey_port applies only to a simulator-owned bridge")
        if config.max_attempts < 1:
            raise ManifestError("max_attempts must be positive")
        if not math.isfinite(config.request_timeout) or config.request_timeout <= 0:
            raise ManifestError("request_timeout must be a positive finite number")
        if (
            not math.isfinite(config.processing_timeout)
            or config.processing_timeout < 0
        ):
            raise ManifestError("processing_timeout must be a finite nonnegative number")
        if not math.isfinite(config.poll_interval) or config.poll_interval <= 0:
            raise ManifestError("poll_interval must be a positive finite number")
        if config.expected_cid is not None and re.fullmatch(
            r"sha256:[0-9a-f]{64}", config.expected_cid
        ) is None:
            raise ManifestError(
                "expected_cid must be sha256: followed by 64 lowercase hex"
            )
        if os.path.lexists(config.state_dir):
            try:
                state_metadata = config.state_dir.lstat()
            except OSError as error:
                raise ManifestError(
                    f"state directory could not be inspected: {type(error).__name__}"
                ) from error
            if not stat.S_ISDIR(state_metadata.st_mode):
                raise ManifestError("state directory must be a plain directory")
        try:
            fixture_root = config.manifest.root.resolve()
            state_dir = config.state_dir.resolve()
            evidence_path = config.evidence_path.resolve()
        except (OSError, RuntimeError) as error:
            raise ManifestError(
                f"simulator path could not be resolved: {type(error).__name__}"
            ) from error
        for path, label in [(state_dir, "state directory"), (evidence_path, "evidence path")]:
            if _paths_overlap(path, fixture_root):
                raise ManifestError(f"{label} cannot overlap the fixture root")
        if evidence_path == state_dir / "state.json":
            raise ManifestError("evidence path cannot overwrite simulator state")
        if config.journal_root is not None:
            try:
                root_metadata = config.journal_root.lstat()
            except OSError as error:
                raise ManifestError(
                    f"journal_root could not be inspected: {type(error).__name__}"
                ) from error
            if not stat.S_ISDIR(root_metadata.st_mode):
                raise ManifestError("journal_root must be a plain directory")
            try:
                journal_root = config.journal_root.resolve(strict=True)
            except (OSError, RuntimeError) as error:
                raise ManifestError(
                    f"journal_root could not be resolved: {type(error).__name__}"
                ) from error
            if _paths_overlap(journal_root, fixture_root):
                raise ManifestError(
                    "receiving journal root cannot overlap the fixture root"
                )
            if not journal_root.is_dir():
                raise ManifestError("journal_root must be an existing directory")
            for path, label in [
                (state_dir, "state directory"),
                (evidence_path, "evidence path"),
            ]:
                if _paths_overlap(path, journal_root):
                    raise ManifestError(
                        f"{label} cannot overlap the receiving journal root"
                    )
        else:
            journal_root = None
        self.config = config
        self.fixture_root = fixture_root
        self.state_dir = state_dir
        self.journal_root = journal_root
        self.expected_cid = config.expected_cid
        self.profile = config.manifest.profiles.get(config.profile)
        if self.profile is None:
            config.manifest.profile_segments(config.profile)
            raise AssertionError("profile lookup should have failed")
        self.segments = config.manifest.profile_segments(config.profile)
        if self.profile.verification in {"custody", "processing"} and journal_root is None:
            raise ManifestError(
                f"{self.profile.verification} verification requires an explicit receiving journal root"
            )
        if journal_root is not None and config.bridge_url and self.expected_cid is None:
            raise ManifestError(
                "white-box verification through an external bridge requires expected_cid"
            )
        self.day_map = build_day_map(self.segments, config.date_mode, config.anchor_day)
        self.state_path = self.state_dir / "state.json"
        self.state = self._load_or_create_state()
        self._segment_request_counts: dict[str, int] = {}
        self.evidence: dict[str, Any] = {
            "schema": EVIDENCE_SCHEMA,
            "run_id": self.state["run_id"],
            "started_at": self.state["started_at"],
            "finished_at": None,
            "result": None,
            "error": None,
            "profile": config.profile,
            "verification": self.profile.verification,
            "carrier": config.carrier,
            "bridge": {
                "ownership": "external" if config.bridge_url else "simulator",
                "carrier_assurance": (
                    "caller-asserted" if config.bridge_url else None
                ),
                "convey_port": config.convey_port,
            },
            "manifest": {
                "path": str(config.manifest.path),
                "sha256": config.manifest.digest,
                "fixture_revision": _git_revision(config.manifest.root),
                "fixture_dirty": _git_dirty(config.manifest.root),
            },
            "simulator": _simulator_code_provenance(),
            "receiver": None,
            "day_map": self.day_map,
            "request_count": 0,
            "segments": [],
            "contract_reads": [],
            "http_receipts": [],
            "upload_receipts": [],
        }

    def _load_or_create_state(self) -> dict[str, Any]:
        try:
            self.config.state_dir.mkdir(mode=0o700, parents=True, exist_ok=True)
            os.chmod(self.config.state_dir, 0o700)
        except OSError as error:
            raise SimulationFailure(
                f"state directory could not be prepared: {type(error).__name__}"
            ) from error
        if os.path.lexists(self.state_path):
            state = _bounded_json_file(
                self.state_dir, self.state_path, "simulator state.json"
            )
            expected = {
                "schema": STATE_SCHEMA,
                "manifest_sha256": self.config.manifest.digest,
                "profile": self.config.profile,
                "carrier": self.config.carrier,
                "day_map": self.day_map,
            }
            for key, value in expected.items():
                if state.get(key) != value:
                    raise SimulationFailure(
                        f"existing state {key} does not match this run; choose a new state directory"
                    )
            self._validate_state(state)
            return state
        state = {
            "schema": STATE_SCHEMA,
            "run_id": uuid.uuid4().hex,
            "started_at": _utc_now(),
            "manifest_sha256": self.config.manifest.digest,
            "profile": self.config.profile,
            "carrier": self.config.carrier,
            "day_map": self.day_map,
            "segments": {},
        }
        _atomic_json(self.state_path, state)
        return state

    def _validate_state(self, state: dict[str, Any]) -> None:
        run_id = state.get("run_id")
        if not isinstance(run_id, str) or not _RUN_ID_RE.fullmatch(run_id):
            raise SimulationFailure("existing state run_id is malformed")
        started_at = state.get("started_at")
        if not isinstance(started_at, str) or not started_at.strip():
            raise SimulationFailure("existing state started_at is malformed")
        receiver = state.get("receiver_instance_id")
        if receiver is not None and (
            not isinstance(receiver, str) or not receiver.strip()
        ):
            raise SimulationFailure(
                "existing state receiver_instance_id is malformed"
            )
        client_cid = state.get("client_cid")
        if client_cid is not None and (
            not isinstance(client_cid, str)
            or re.fullmatch(r"sha256:[0-9a-f]{64}", client_cid) is None
        ):
            raise SimulationFailure("existing state client_cid is malformed")
        entries = state.get("segments")
        if not isinstance(entries, dict):
            raise SimulationFailure("existing state segments must be an object")
        selected = {segment.fixture_id: segment for segment in self.segments}
        for fixture_id, entry in entries.items():
            if fixture_id not in selected:
                raise SimulationFailure(
                    f"existing state contains unselected fixture {fixture_id!r}"
                )
            if not isinstance(entry, dict):
                raise SimulationFailure(
                    f"existing state segment {fixture_id} must be an object"
                )
            phase = entry.get("phase")
            if phase is not None and phase not in _STATE_PHASES:
                raise SimulationFailure(
                    f"existing state segment {fixture_id} has invalid phase"
                )
            expected_segment = selected[fixture_id]
            mapped_day = entry.get("mapped_day")
            if mapped_day is not None and mapped_day != self.day_map[expected_segment.day]:
                raise SimulationFailure(
                    f"existing state segment {fixture_id} has invalid mapped_day"
                )
            requested = entry.get("requested_segment")
            if requested is not None and requested != expected_segment.segment:
                raise SimulationFailure(
                    f"existing state segment {fixture_id} has invalid requested_segment"
                )
            landed = entry.get("landed_segment")
            if landed is not None and (
                not isinstance(landed, str) or not _SEGMENT_KEY_RE.fullmatch(landed)
            ):
                raise SimulationFailure(
                    f"existing state segment {fixture_id} has invalid landed_segment"
                )
            for name in ("upload_attempts", "duplicate_attempts"):
                count = entry.get(name)
                if count is not None and (
                    isinstance(count, bool) or not isinstance(count, int) or count < 0
                ):
                    raise SimulationFailure(
                        f"existing state segment {fixture_id} has invalid {name}"
                    )
            duplicate_proven = entry.get("duplicate_proven")
            if duplicate_proven is not None and not isinstance(
                duplicate_proven, bool
            ):
                raise SimulationFailure(
                    f"existing state segment {fixture_id} has invalid duplicate_proven"
                )
            response_status = entry.get("last_response_status")
            if response_status is not None and (
                isinstance(response_status, bool)
                or not isinstance(response_status, int)
                or not 100 <= response_status <= 599
            ):
                raise SimulationFailure(
                    f"existing state segment {fixture_id} has invalid last_response_status"
                )
            accepted_response = entry.get("accepted_response")
            if accepted_response is not None:
                if not isinstance(accepted_response, dict):
                    raise SimulationFailure(
                        f"existing state segment {fixture_id} has invalid accepted_response"
                    )
                if accepted_response.get("http_status") != 200 or not isinstance(
                    accepted_response.get("body"), dict
                ):
                    raise SimulationFailure(
                        f"existing state segment {fixture_id} has invalid accepted_response"
                    )
            contract_failure = entry.get("contract_failure")
            if contract_failure is not None and not isinstance(contract_failure, dict):
                raise SimulationFailure(
                    f"existing state segment {fixture_id} has invalid contract_failure"
                )
            if phase == "accepted" and (
                landed is None or accepted_response is None
            ):
                raise SimulationFailure(
                    f"existing state segment {fixture_id} has incomplete accepted state"
                )
            if phase == "contract_failed" and contract_failure is None:
                raise SimulationFailure(
                    f"existing state segment {fixture_id} has incomplete contract failure"
                )

    def _save_state(self) -> None:
        _atomic_json(self.state_path, self.state)

    def _verify_fixture_bytes(self, segment: FixtureSegment) -> None:
        with self._fixture_uploads(segment):
            pass

    @contextmanager
    def _fixture_uploads(
        self, segment: FixtureSegment
    ) -> Iterator[tuple[MultipartUpload, ...]]:
        with ExitStack() as stack:
            uploads: list[MultipartUpload] = []
            for item in segment.files:
                flags = os.O_RDONLY | getattr(os, "O_CLOEXEC", 0)
                flags |= getattr(os, "O_NOFOLLOW", 0)
                try:
                    descriptor = os.open(item.path, flags)
                except OSError as error:
                    raise SimulationFailure(
                        f"fixture {segment.fixture_id}/{item.submitted} cannot be opened safely: "
                        f"{type(error).__name__}"
                    ) from error
                try:
                    metadata = os.fstat(descriptor)
                    if (
                        not stat.S_ISREG(metadata.st_mode)
                        or metadata.st_dev != item.device
                        or metadata.st_ino != item.inode
                    ):
                        raise SimulationFailure(
                            f"fixture {segment.fixture_id}/{item.submitted} changed identity during the run"
                        )
                    if metadata.st_size != item.size:
                        raise SimulationFailure(
                            f"fixture {segment.fixture_id}/{item.submitted} changed size during the run"
                        )
                    handle = os.fdopen(descriptor, "rb")
                except Exception:
                    os.close(descriptor)
                    raise
                handle = stack.enter_context(handle)
                digest = hashlib.sha256()
                while chunk := handle.read(1024 * 1024):
                    digest.update(chunk)
                if digest.hexdigest() != item.sha256:
                    raise SimulationFailure(
                        f"fixture {segment.fixture_id}/{item.submitted} changed digest during the run"
                    )
                handle.seek(0)
                uploads.append(
                    MultipartUpload(
                        filename=item.submitted,
                        handle=handle,
                        size=item.size,
                        sha256=item.sha256,
                    )
                )
            yield tuple(uploads)

    def _record_upload_request(self, fixture_id: str) -> None:
        self.evidence["request_count"] += 1
        self._segment_request_counts[fixture_id] = (
            self._segment_request_counts.get(fixture_id, 0) + 1
        )

    def _get_json(
        self,
        client: BridgeHttpClient,
        path: str,
        query: dict[str, str] | None = None,
        *,
        purpose: str,
        evidence_body: Callable[[dict[str, Any]], dict[str, Any]] | None = None,
    ) -> HttpResponse:
        base = {
            "method": "GET",
            "path": path,
            "query": dict(query) if query else None,
            "purpose": purpose,
        }
        try:
            response = client.get_json(path, query)
        except HttpResponseError as error:
            self.evidence["http_receipts"].append(
                {**base, "response_error": error.receipt}
            )
            raise
        self.evidence["http_receipts"].append(
            {
                **base,
                "http_status": response.status,
                "body": (
                    evidence_body(response.body)
                    if evidence_body is not None
                    else response.body
                ),
            }
        )
        return response

    def _record_upload_response(
        self,
        fixture_id: str,
        kind: str,
        response: HttpResponse | None = None,
        error: HttpResponseError | None = None,
    ) -> None:
        receipt: dict[str, Any] = {
            "fixture_id": fixture_id,
            "kind": kind,
        }
        if response is not None:
            receipt.update(
                {"http_status": response.status, "body": response.body}
            )
        if error is not None:
            receipt["response_error"] = error.receipt
        self.evidence["upload_receipts"].append(receipt)

    def _envelope(self, segment: FixtureSegment, mapped_day: str) -> dict[str, Any]:
        return {
            "day": mapped_day,
            "segment": segment.segment,
            "source": segment.source,
            "meta": {
                **segment.meta,
                "fixture_id": segment.fixture_id,
                "fixture_manifest_sha256": self.config.manifest.digest,
            },
            "files": [
                {**item.metadata, "submitted": item.submitted} for item in segment.files
            ],
        }

    def _listing(
        self, client: BridgeHttpClient, segment: FixtureSegment, mapped_day: str
    ) -> dict[str, Any]:
        query = {"source": segment.source} if segment.source else None
        response = self._get_json(
            client,
            f"/app/devices/ingest/segments/{mapped_day}",
            query,
            purpose=f"segment listing {segment.fixture_id}",
        )
        if response.status != 200:
            reason = response.body.get("reason_code", "unknown")
            if response.status >= 500:
                raise HttpRequestError(
                    f"listing for {segment.fixture_id} returned transient HTTP "
                    f"{response.status} ({reason})"
                )
            raise SimulationFailure(
                f"listing for {segment.fixture_id} returned HTTP {response.status} ({reason})"
            )
        if response.body.get("protocol_version") != 3 or not isinstance(
            response.body.get("items"), list
        ):
            raise SimulationFailure(
                "ingest listing does not carry the v3 response shape"
            )
        items = response.body["items"]
        total = response.body.get("total")
        if isinstance(total, bool) or not isinstance(total, int) or total != len(items):
            raise SimulationFailure("ingest listing total does not match its items")
        for item in items:
            if not isinstance(item, dict):
                raise SimulationFailure("ingest listing item is not an object")
            key = item.get("key")
            if not isinstance(key, str) or not _SEGMENT_KEY_RE.fullmatch(key):
                raise SimulationFailure("ingest listing item has an invalid key")
            if not isinstance(item.get("observed"), bool):
                raise SimulationFailure("ingest listing item observed must be boolean")
            original = item.get("original_key")
            if original is not None and (
                not isinstance(original, str)
                or not _SEGMENT_KEY_RE.fullmatch(original)
            ):
                raise SimulationFailure(
                    "ingest listing item has an invalid original_key"
                )
            raw_files = item.get("files")
            if not isinstance(raw_files, list):
                raise SimulationFailure("ingest listing files must be an array")
            for entry in raw_files:
                if not isinstance(entry, dict):
                    raise SimulationFailure("ingest listing file is not an object")
                name = entry.get("name")
                size = entry.get("size")
                digest = entry.get("sha256")
                submitted_name = entry.get("submitted_name")
                if not _safe_content_name(name):
                    raise SimulationFailure("ingest listing file name is invalid")
                if isinstance(size, bool) or not isinstance(size, int) or size < 0:
                    raise SimulationFailure("ingest listing file size is invalid")
                if not isinstance(digest, str) or not _SHA256_RE.fullmatch(digest):
                    raise SimulationFailure("ingest listing file digest is invalid")
                if entry.get("status") not in _FILE_STATUSES:
                    raise SimulationFailure("ingest listing file status is invalid")
                if submitted_name is not None and not _safe_content_name(
                    submitted_name
                ):
                    raise SimulationFailure(
                        "ingest listing submitted_name is invalid"
                    )
        return response.body

    def _bind_receiver(self, client: BridgeHttpClient) -> None:
        if self.expected_cid is not None:
            prior_cid = self.state.get("client_cid")
            if prior_cid is not None and prior_cid != self.expected_cid:
                raise SimulationFailure(
                    "state directory belongs to a different linked-device identity"
                )
            self.state["client_cid"] = self.expected_cid
            self._save_state()
        self.evidence["receiver"] = {
            "identity": None,
            "status": None,
            "carrier": self.config.carrier,
            "accepted_postures": (
                ["direct", "spl"]
                if self.config.carrier == "direct"
                else ["spl"]
            ),
            "observed_posture": None,
            "posture_compatible": None,
            "identity_status_match": False,
            "journal_root": None,
        }
        identity = self._get_json(
            client, "/app/link/api/identity", purpose="receiver identity"
        )
        self.evidence["receiver"]["identity"] = {
            "http_status": identity.status,
            "body": identity.body,
        }
        if identity.status != 200:
            reason = identity.body.get("reason_code", "unknown")
            raise SimulationFailure(
                f"receiver identity returned HTTP {identity.status} ({reason})"
            )
        instance_id = identity.body.get("instance_id")
        if (
            identity.body.get("committed") is not True
            or not isinstance(instance_id, str)
            or not instance_id.strip()
        ):
            raise SimulationFailure(
                "receiver does not expose a committed journal identity"
            )
        status = self._get_json(
            client,
            "/app/link/api/status",
            purpose="receiver status",
            evidence_body=lambda body: {
                key: body[key]
                for key in _RECEIVER_STATUS_EVIDENCE_FIELDS
                if key in body
            },
        )
        self.evidence["receiver"]["status"] = {
            "http_status": status.status,
            "body": {
                key: status.body[key]
                for key in _RECEIVER_STATUS_EVIDENCE_FIELDS
                if key in status.body
            },
        }
        if status.status != 200:
            reason = status.body.get("reason_code", "unknown")
            raise SimulationFailure(
                f"receiver link status returned HTTP {status.status} ({reason})"
            )
        accepted_postures = set(self.evidence["receiver"]["accepted_postures"])
        observed_posture = status.body.get("posture")
        posture_compatible = (
            isinstance(observed_posture, str)
            and observed_posture in accepted_postures
        )
        identity_status_match = status.body.get("instance_id") == instance_id
        self.evidence["receiver"]["observed_posture"] = observed_posture
        self.evidence["receiver"]["posture_compatible"] = posture_compatible
        self.evidence["receiver"]["identity_status_match"] = identity_status_match
        if not identity_status_match:
            raise SimulationFailure(
                "receiver identity and link status name different journals"
            )
        if not posture_compatible:
            accepted = ", ".join(sorted(accepted_postures))
            raise SimulationFailure(
                f"receiver link posture is not compatible with the requested "
                f"{self.config.carrier} carrier (accepted: {accepted})"
            )
        local_identity: dict[str, str] | None = None
        if self.journal_root is not None:
            state_path, local_instance_id = _journal_state_identity(self.journal_root)
            if local_instance_id != instance_id:
                raise SimulationFailure(
                    "receiving journal root belongs to a different journal identity"
                )
            local_identity = {
                "path": str(self.journal_root),
                "state_path": state_path,
                "instance_id": local_instance_id,
            }
        prior = self.state.get("receiver_instance_id")
        if prior is not None and prior != instance_id:
            raise SimulationFailure(
                "state directory belongs to a different receiving journal"
            )
        self.state["receiver_instance_id"] = instance_id
        self._save_state()
        self.evidence["receiver"]["journal_root"] = local_identity
        self.evidence["receiver"]["expected_cid"] = self.expected_cid

    def _verify_local_bridge_status(self, client: BridgeHttpClient) -> None:
        response = self._get_json(
            client, "/_solstone/link/status", purpose="local bridge status"
        )
        self.evidence["bridge"]["local_status"] = {
            "http_status": response.status,
            "body": response.body,
        }
        if response.status != 200 or response.body.get("manager_alive") is not True:
            raise SimulationFailure(
                "simulator-owned link bridge did not report a live manager"
            )

    @staticmethod
    def _matched_files(
        item: dict[str, Any], segment: FixtureSegment
    ) -> list[dict[str, Any]] | None:
        raw_files = item.get("files")
        if not isinstance(raw_files, list):
            return None
        unmatched = list(raw_files)
        matched: list[dict[str, Any]] = []
        for expected in segment.files:
            match_index = None
            for index, candidate in enumerate(unmatched):
                if not isinstance(candidate, dict):
                    continue
                effective_name = candidate.get("submitted_name", candidate.get("name"))
                if (
                    effective_name == expected.submitted
                    and candidate.get("size") == expected.size
                    and candidate.get("sha256") == expected.sha256
                ):
                    match_index = index
                    break
            if match_index is None:
                return None
            candidate = unmatched.pop(match_index)
            assert isinstance(candidate, dict)
            matched.append(candidate)
        return matched

    def _find_listing_item(
        self,
        listing: dict[str, Any],
        segment: FixtureSegment,
        landed_segment: str | None,
    ) -> dict[str, Any] | None:
        candidates = []
        for item in listing.get("items", []):
            if not isinstance(item, dict) or self._matched_files(item, segment) is None:
                continue
            key = item.get("key")
            if not isinstance(key, str) or not _SEGMENT_KEY_RE.fullmatch(key):
                continue
            if landed_segment and item.get("key") != landed_segment:
                continue
            if not landed_segment and not (
                item.get("key") == segment.segment
                or item.get("original_key") == segment.segment
            ):
                continue
            candidates.append(item)
        if len(candidates) > 1:
            raise SimulationFailure(
                f"listing is ambiguous for fixture {segment.fixture_id}; {len(candidates)} matches"
            )
        return candidates[0] if candidates else None

    @staticmethod
    def _manifest_files_match(
        value: dict[str, Any], segment: FixtureSegment
    ) -> bool:
        files = value.get("files")
        if not isinstance(files, dict):
            return False
        return all(
            files.get(expected.submitted)
            == {"sha256": expected.sha256, "size": expected.size}
            for expected in segment.files
        )

    def _journal_candidate(
        self, segment: FixtureSegment, mapped_day: str, landed_segment: str
    ) -> tuple[
        Path,
        dict[str, Any],
        list[dict[str, Any]],
        dict[str, int],
        list[dict[str, Any]],
    ] | None:
        assert self.journal_root is not None
        if self.expected_cid is None:
            raise SimulationFailure(
                "white-box verification has no authenticated client CID"
            )
        if not _SEGMENT_KEY_RE.fullmatch(landed_segment):
            raise SimulationFailure("receiver returned an invalid landed segment key")
        day_root = self.journal_root / "chronicle" / mapped_day
        if not _plain_descendant(self.journal_root, day_root, directory=True):
            return None
        try:
            streams = tuple(day_root.iterdir())
        except OSError as error:
            raise SimulationFailure("receiving journal day cannot be inspected") from error
        candidates: list[
            tuple[
                Path,
                dict[str, Any],
                list[dict[str, Any]],
                dict[str, int],
                list[dict[str, Any]],
            ]
        ] = []
        for stream in streams:
            path = stream / landed_segment
            if not _plain_descendant(self.journal_root, path, directory=True):
                continue
            try:
                ingest = _bounded_json_file(
                    self.journal_root,
                    path / "ingest.json",
                    f"fixture {segment.fixture_id} ingest.json",
                )
            except SimulationFailure:
                continue
            if (
                ingest.get("schema_version") == 1
                and ingest.get("requested_segment") == segment.segment
                and self._manifest_files_match(ingest, segment)
            ):
                try:
                    events, recovery = _bounded_event_file(
                        self.journal_root,
                        path / "events.jsonl",
                        f"fixture {segment.fixture_id} events.jsonl",
                    )
                except SimulationFailure:
                    continue
                matching = self._matching_events(
                    events,
                    segment,
                    mapped_day,
                    landed_segment,
                    stream.name,
                    self.expected_cid,
                )
                if matching:
                    candidates.append((path, ingest, events, recovery, matching))
        if len(candidates) > 1:
            raise SimulationFailure(
                f"multiple journal directories satisfy custody for {segment.fixture_id}"
            )
        return candidates[0] if candidates else None

    @staticmethod
    def _event_files_match(raw: Any, segment: FixtureSegment) -> bool:
        if not isinstance(raw, list) or len(raw) != len(segment.files):
            return False
        by_submitted = {
            item.get("submitted"): item
            for item in raw
            if isinstance(item, dict) and isinstance(item.get("submitted"), str)
        }
        if len(by_submitted) != len(segment.files):
            return False
        for expected in segment.files:
            item = by_submitted.get(expected.submitted)
            if item is None:
                return False
            required = {
                **expected.metadata,
                "submitted": expected.submitted,
                "written": expected.submitted,
                "size": expected.size,
                "sha256": expected.sha256,
            }
            if any(item.get(key) != value for key, value in required.items()):
                return False
            if item.get("disposition") not in {"written", "already_held"}:
                return False
        return True

    def _matching_events(
        self,
        events: list[dict[str, Any]],
        segment: FixtureSegment,
        mapped_day: str,
        landed_segment: str,
        stream: str,
        expected_cid: str,
    ) -> list[dict[str, Any]]:
        expected_meta = self._envelope(segment, mapped_day)["meta"]
        return [
            event
            for event in events
            if event.get("record_type") == "device_ingest"
            and event.get("record_version") == 1
            and event.get("protocol_version") == 3
            and event.get("outcome") in {"accepted", "duplicate"}
            and event.get("cid", event.get("did")) == expected_cid
            and event.get("source") == segment.source
            and event.get("stream") == stream
            and event.get("day") == mapped_day
            and event.get("segment") == landed_segment
            and event.get("meta") == expected_meta
            and self._event_files_match(event.get("files"), segment)
        ]

    def _hash_receiver_file(
        self, segment_path: Path, physical_name: str, expected: FixtureFile
    ) -> None:
        label = f"receiver file for {expected.submitted}"
        if not _safe_content_name(physical_name):
            raise SimulationFailure(f"{label} has an invalid physical name")
        _confined_digest(
            segment_path,
            segment_path / physical_name,
            expected.size,
            expected.sha256,
            label,
        )

    def _custody_oracle(
        self,
        segment: FixtureSegment,
        mapped_day: str,
        landed_segment: str,
        listing_item: dict[str, Any],
    ) -> tuple[bool, Path | None, dict[str, Any] | None]:
        if self.journal_root is None:
            return True, None, None
        candidate = self._journal_candidate(segment, mapped_day, landed_segment)
        if candidate is None:
            return False, None, None
        path, ingest, events, event_recovery, matching_events = candidate
        stream = path.parent.name
        marker = _bounded_json_file(
            self.journal_root,
            path / "stream.json",
            f"fixture {segment.fixture_id} stream.json",
        )
        seq = marker.get("seq")
        previous = (marker.get("prev_day"), marker.get("prev_segment"))
        previous_ok = previous == (None, None) or (
            isinstance(previous[0], str)
            and re.fullmatch(r"[0-9]{8}", previous[0]) is not None
            and isinstance(previous[1], str)
            and _SEGMENT_KEY_RE.fullmatch(previous[1]) is not None
        )
        if (
            marker.get("stream") != stream
            or isinstance(seq, bool)
            or not isinstance(seq, int)
            or seq < 1
            or not previous_ok
        ):
            raise SimulationFailure(
                f"fixture {segment.fixture_id} stream marker is incoherent"
            )
        matched_files = self._matched_files(listing_item, segment)
        assert matched_files is not None
        latest_event_files = {
            item.get("submitted"): item
            for item in matching_events[-1]["files"]
            if isinstance(item, dict)
        }
        physical: list[dict[str, Any]] = []
        processed: list[dict[str, Any]] = []
        for expected, listed in zip(segment.files, matched_files, strict=True):
            status = listed.get("status")
            physical_name = listed.get("name")
            event_file = latest_event_files.get(expected.submitted)
            if (
                not _safe_content_name(physical_name)
                or not isinstance(event_file, dict)
                or physical_name != event_file.get("written")
            ):
                raise SimulationFailure(
                    f"fixture {segment.fixture_id} listing file is not bound to its device event"
                )
            if status == "present":
                self._hash_receiver_file(path, physical_name, expected)
                physical.append(
                    {"submitted": expected.submitted, "sha256_verified": True}
                )
            elif status == "processed":
                processing_expectation = next(
                    (
                        item
                        for item in segment.expectation.processing
                        if item.input == expected.submitted
                    ),
                    None,
                )
                if processing_expectation is None:
                    raise SimulationFailure(
                        f"fixture {segment.fixture_id} was reported processed without an oracle"
                    )
                ready, terminal = self._terminal_processing_oracle(
                    segment, path, processing_expectation
                )
                if not ready or terminal is None:
                    return False, None, None
                processed.append(terminal)
            else:
                raise SimulationFailure(
                    f"fixture {segment.fixture_id} listing cannot establish custody"
                )
        proof = {
            "segment_path": path.relative_to(self.journal_root).as_posix(),
            "ingest": {
                "schema_version": ingest["schema_version"],
                "requested_segment": ingest["requested_segment"],
                "files_verified": len(segment.files),
            },
            "stream": {
                "name": stream,
                "seq": seq,
                "prev_day": previous[0],
                "prev_segment": previous[1],
            },
            "device_event": {
                "cid": matching_events[-1].get(
                    "cid", matching_events[-1].get("did")
                ),
                "outcome": matching_events[-1]["outcome"],
                "files_verified": len(segment.files),
                "matched_index": events.index(matching_events[-1]),
                "recovery": event_recovery,
            },
            "physical_files": physical,
            "processed_files": processed,
        }
        return True, path, proof

    @staticmethod
    def _rfc3339(value: Any) -> bool:
        if not isinstance(value, str) or not value:
            return False
        try:
            parsed = datetime.fromisoformat(value.replace("Z", "+00:00"))
        except (ValueError, OverflowError):
            return False
        return parsed.tzinfo is not None and parsed.utcoffset() is not None

    def _processing_oracle(
        self,
        segment: FixtureSegment,
        path: Path,
        expectation: ProcessingExpectation,
    ) -> tuple[bool, dict[str, Any] | None]:
        assert self.journal_root is not None
        output = path / expectation.output
        if not _plain_descendant(self.journal_root, output, directory=False):
            return False, None
        rows, output_sha256 = _bounded_jsonl_data(
            self.journal_root,
            output,
            f"fixture {segment.fixture_id} {expectation.output}",
        )
        expected_file = next(
            item for item in segment.files if item.submitted == expectation.input
        )
        header = rows[0]
        record = header.get("_solstone_processing")
        if not isinstance(record, dict):
            raise SimulationFailure(
                f"fixture {segment.fixture_id} processing record is missing"
            )
        state = record.get("state")
        attempts = record.get("attempts", 0)
        retryable_failed = (
            state == "failed"
            and record.get("handler") == expectation.handler
            and expectation.handler == "describe"
            and record.get("reason_code") != "corrupt_input"
            and isinstance(attempts, int)
            and not isinstance(attempts, bool)
            and attempts < 3
        )
        if retryable_failed:
            return False, None
        input_size = record.get("input_size")
        if (
            header.get("raw") != expectation.input
            or record.get("schema") != "solstone.processing.v1"
            or state != "analyzed"
            or record.get("reason_code") != "ok"
            or record.get("handler") != expectation.handler
            or isinstance(input_size, bool)
            or not isinstance(input_size, int)
            or input_size != expected_file.size
            or not self._rfc3339(record.get("attempted_at"))
        ):
            raise SimulationFailure(
                f"fixture {segment.fixture_id} processing record is not a successful exact-input proof"
            )
        semantic = rows[1:]
        if expectation.handler == "transcribe":
            valid_rows = [row for row in semantic if _valid_audio_semantic_row(row)]
            identities = [row.get("sentence_id") for row in semantic]
            semantics_ok = (
                bool(semantic)
                and len(valid_rows) == len(semantic)
                and len(set(identities)) == len(identities)
                and any(bool(row["text"].strip()) for row in valid_rows)
            )
        else:
            valid_rows = [
                row for row in semantic if _valid_describe_semantic_row(row)
            ]
            identities = [row.get("frame_id") for row in semantic]
            semantics_ok = (
                bool(semantic)
                and len(valid_rows) == len(semantic)
                and len(set(identities)) == len(identities)
            )
        if not semantics_ok:
            raise SimulationFailure(
                f"fixture {segment.fixture_id} processing output has no valid semantic artifact"
            )
        return True, {
            "input": expectation.input,
            "output": expectation.output,
            "handler": expectation.handler,
            "input_size": expected_file.size,
            "attempted_at": record["attempted_at"],
            "state": state,
            "reason_code": record["reason_code"],
            "total_rows": len(semantic),
            "valid_semantic_rows": len(valid_rows),
            "output_sha256": output_sha256,
        }

    def _terminal_processing_oracle(
        self,
        segment: FixtureSegment,
        path: Path,
        expectation: ProcessingExpectation,
    ) -> tuple[bool, dict[str, Any] | None]:
        assert self.journal_root is not None
        output = path / expectation.output
        if not _plain_descendant(self.journal_root, output, directory=False):
            return False, None
        rows = _bounded_jsonl_file(
            self.journal_root,
            output,
            f"fixture {segment.fixture_id} {expectation.output}",
        )
        expected_file = next(
            item for item in segment.files if item.submitted == expectation.input
        )
        record = rows[0].get("_solstone_processing")
        input_size = record.get("input_size") if isinstance(record, dict) else None
        if (
            rows[0].get("raw") != expectation.input
            or not isinstance(record, dict)
            or record.get("schema") != "solstone.processing.v1"
            or record.get("state") not in {"analyzed", "empty"}
            or record.get("handler") != expectation.handler
            or isinstance(input_size, bool)
            or not isinstance(input_size, int)
            or input_size != expected_file.size
        ):
            raise SimulationFailure(
                f"fixture {segment.fixture_id} does not carry exact terminal processing proof"
            )
        return True, {
            "input": expectation.input,
            "output": expectation.output,
            "handler": expectation.handler,
            "input_size": expected_file.size,
            "state": record["state"],
        }

    def _white_box_oracles(
        self,
        segment: FixtureSegment,
        mapped_day: str,
        landed_segment: str,
        listing_item: dict[str, Any],
    ) -> tuple[bool, dict[str, Any] | None]:
        if self.profile.verification == "contract" and self.journal_root is None:
            return True, {"custody": None, "processing": []}
        ready, path, custody = self._custody_oracle(
            segment, mapped_day, landed_segment, listing_item
        )
        if not ready:
            return False, None
        processing: list[dict[str, Any]] = []
        if self.profile.verification == "processing":
            assert path is not None
            for expectation in segment.expectation.processing:
                processed, proof = self._processing_oracle(
                    segment, path, expectation
                )
                if not processed:
                    return False, None
                assert proof is not None
                processing.append(proof)
        return True, {"custody": custody, "processing": processing}

    def _reconcile(
        self,
        client: BridgeHttpClient,
        segment: FixtureSegment,
        mapped_day: str,
        landed_segment: str | None,
        wait: bool,
    ) -> tuple[dict[str, Any] | None, dict[str, Any] | None, bool]:
        deadline = time.monotonic() + (self.config.processing_timeout if wait else 0.0)
        while True:
            listing = self._listing(client, segment, mapped_day)
            item = self._find_listing_item(listing, segment, landed_segment)
            if item is not None:
                matched_files = self._matched_files(item, segment)
                assert matched_files is not None
                statuses = {entry.get("status") for entry in matched_files}
                statuses_ok = statuses and statuses.issubset(
                    set(segment.expectation.file_statuses)
                )
                outputs_ok, journal_oracles = self._white_box_oracles(
                    segment, mapped_day, str(item["key"]), item
                )
                if statuses_ok and outputs_ok:
                    return item, journal_oracles, True
            if time.monotonic() >= deadline:
                return item, None, False
            time.sleep(self.config.poll_interval)

    @staticmethod
    def _landed_segment(response: HttpResponse) -> str | None:
        status = response.body.get("status")
        if status == "duplicate":
            value = response.body.get("existing_segment")
        else:
            value = response.body.get("segment")
        return (
            value
            if isinstance(value, str) and _SEGMENT_KEY_RE.fullmatch(value)
            else None
        )

    def _validate_upload_response(
        self,
        response: HttpResponse,
        segment: FixtureSegment,
        envelope: dict[str, Any],
    ) -> str:
        status = response.body.get("status")
        if status not in {"ok", "collision", "duplicate"}:
            raise SimulationFailure(
                f"fixture {segment.fixture_id} returned an unknown upload status"
            )
        if response.body.get("meta") != envelope["meta"]:
            raise SimulationFailure(
                f"fixture {segment.fixture_id} response did not echo exact metadata"
            )
        raw_descriptors = response.body.get("file_descriptors")
        if not isinstance(raw_descriptors, list) or len(raw_descriptors) != len(
            segment.files
        ):
            raise SimulationFailure(
                f"fixture {segment.fixture_id} response file descriptors are incomplete"
            )
        by_submitted: dict[str, dict[str, Any]] = {}
        dispositions: list[str] = []
        for raw in raw_descriptors:
            if not isinstance(raw, dict):
                raise SimulationFailure(
                    f"fixture {segment.fixture_id} response descriptor is malformed"
                )
            submitted = raw.get("submitted")
            if not isinstance(submitted, str) or submitted in by_submitted:
                raise SimulationFailure(
                    f"fixture {segment.fixture_id} response descriptor names are invalid"
                )
            by_submitted[submitted] = raw
        for expected in segment.files:
            descriptor = by_submitted.get(expected.submitted)
            if descriptor is None:
                raise SimulationFailure(
                    f"fixture {segment.fixture_id} response omitted {expected.submitted}"
                )
            disposition = descriptor.get("disposition")
            if disposition not in {"written", "already_held"}:
                raise SimulationFailure(
                    f"fixture {segment.fixture_id} response disposition is invalid"
                )
            expected_descriptor = {
                **expected.metadata,
                "submitted": expected.submitted,
                "written": expected.submitted,
                "size": expected.size,
                "sha256": expected.sha256,
                "disposition": disposition,
            }
            if descriptor != expected_descriptor:
                raise SimulationFailure(
                    f"fixture {segment.fixture_id} response descriptor does not match {expected.submitted}"
                )
            dispositions.append(disposition)
        if status == "duplicate":
            if any(item != "already_held" for item in dispositions):
                raise SimulationFailure(
                    f"fixture {segment.fixture_id} duplicate response was not already held"
                )
            landed = response.body.get("existing_segment")
            if not isinstance(response.body.get("message"), str):
                raise SimulationFailure(
                    f"fixture {segment.fixture_id} duplicate response omitted its message"
                )
        else:
            if "written" not in dispositions:
                raise SimulationFailure(
                    f"fixture {segment.fixture_id} accepted response wrote no file"
                )
            landed = response.body.get("segment")
            if response.body.get("bytes") != sum(item.size for item in segment.files):
                raise SimulationFailure(
                    f"fixture {segment.fixture_id} response byte count is invalid"
                )
            if response.body.get("files") != [
                item.submitted for item in segment.files
            ]:
                raise SimulationFailure(
                    f"fixture {segment.fixture_id} response file names are invalid"
                )
        if not isinstance(landed, str) or not _SEGMENT_KEY_RE.fullmatch(landed):
            raise SimulationFailure(
                f"fixture {segment.fixture_id} response omitted the landed segment"
            )
        if status == "ok" and landed != segment.segment:
            raise SimulationFailure(
                f"fixture {segment.fixture_id} ok response changed the segment key"
            )
        if status == "collision" and (
            landed == segment.segment
            or response.body.get("segment_original") != segment.segment
        ):
            raise SimulationFailure(
                f"fixture {segment.fixture_id} collision response lost requested-key lineage"
            )
        return landed

    def _verify_duplicate(
        self,
        client: BridgeHttpClient,
        segment: FixtureSegment,
        envelope: dict[str, Any],
        landed_segment: str,
    ) -> dict[str, Any] | None:
        if not self.profile.verify_duplicate:
            return None
        with self._fixture_uploads(segment) as uploads:
            entry = self.state["segments"].setdefault(segment.fixture_id, {})
            entry.update(
                {
                    "phase": "sending",
                    "duplicate_attempts": int(entry.get("duplicate_attempts", 0))
                    + 1,
                }
            )
            self._save_state()
            self._record_upload_request(segment.fixture_id)
            try:
                duplicate = client.post_multipart(
                    "/app/devices/ingest", envelope, uploads
                )
            except HttpResponseError as error:
                self._record_upload_response(
                    segment.fixture_id, "duplicate", error=error
                )
                entry.update(
                    {
                        "phase": "contract_failed",
                        "contract_failure": {
                            "kind": "duplicate_response",
                            "response_error": error.receipt,
                        },
                    }
                )
                self._save_state()
                raise SimulationFailure(
                    f"fixture {segment.fixture_id} duplicate response violated the HTTP contract"
                ) from error
            except HttpRequestError:
                entry["phase"] = "uncertain"
                self._save_state()
                raise
        self._record_upload_response(segment.fixture_id, "duplicate", duplicate)
        try:
            if duplicate.status != 200 or duplicate.body.get("status") != "duplicate":
                raise SimulationFailure(
                    f"fixture {segment.fixture_id} duplicate replay was not idempotent"
                )
            duplicate_landed = self._validate_upload_response(
                duplicate, segment, envelope
            )
        except SimulationFailure as error:
            entry.update(
                {
                    "phase": "contract_failed",
                    "contract_failure": {
                        "kind": "duplicate_response",
                        "http_status": duplicate.status,
                        "body": duplicate.body,
                        "detail": str(error),
                    },
                }
            )
            self._save_state()
            raise
        if duplicate_landed != landed_segment:
            detail = (
                f"fixture {segment.fixture_id} duplicate resolved to another segment"
            )
            entry.update(
                {
                    "phase": "contract_failed",
                    "contract_failure": {
                        "kind": "duplicate_response",
                        "http_status": duplicate.status,
                        "body": duplicate.body,
                        "detail": detail,
                    },
                }
            )
            self._save_state()
            raise SimulationFailure(detail)
        return duplicate.body

    def _finish_segment(
        self,
        *,
        client: BridgeHttpClient,
        segment: FixtureSegment,
        mapped_day: str,
        envelope: dict[str, Any],
        item: dict[str, Any],
        journal_oracles: dict[str, Any] | None,
        upload_attempts: int,
        resumed: bool,
        response: HttpResponse | None,
    ) -> dict[str, Any]:
        landed_segment = str(item["key"])
        if not _SEGMENT_KEY_RE.fullmatch(landed_segment):
            raise SimulationFailure("listing returned an invalid landed segment key")
        entry = self.state["segments"].setdefault(segment.fixture_id, {})
        entry.update({"phase": "reconciled", "landed_segment": landed_segment})
        if response is not None:
            entry["last_response_status"] = response.status
        self._save_state()
        duplicate_from_state = (
            self.profile.verify_duplicate and entry.get("duplicate_proven") is True
        )
        duplicate = (
            None
            if duplicate_from_state
            else self._verify_duplicate(client, segment, envelope, landed_segment)
        )
        if duplicate is not None:
            entry["duplicate_proven"] = True
        entry["phase"] = "complete"
        self._save_state()
        result = {
            "fixture_id": segment.fixture_id,
            "mapped_day": mapped_day,
            "requested_segment": segment.segment,
            "landed_segment": landed_segment,
            "upload_attempts": upload_attempts,
            "duplicate_attempts": int(entry.get("duplicate_attempts", 0)),
            "request_count": self._segment_request_counts.get(
                segment.fixture_id, 0
            ),
            "lifetime_request_count": upload_attempts
            + int(entry.get("duplicate_attempts", 0)),
            "resumed": resumed,
            "response": response.body if response else None,
            "response_http_status": response.status if response else None,
            "listing": item,
            "journal_oracles": journal_oracles,
        }
        if duplicate is not None:
            result["duplicate_response"] = duplicate
        elif duplicate_from_state:
            result["duplicate_proven_from_state"] = True
        return result

    def _upload_one(
        self, client: BridgeHttpClient, segment: FixtureSegment
    ) -> dict[str, Any]:
        mapped_day = self.day_map[segment.day]
        prior = self.state["segments"].get(segment.fixture_id, {})
        envelope = self._envelope(segment, mapped_day)
        prior_phase = prior.get("phase")
        if prior_phase == "contract_failed":
            raise SimulationFailure(
                f"fixture {segment.fixture_id} has a persisted response-contract failure; "
                "use a fresh state directory after correcting the receiver"
            )
        landed_segment = prior.get("landed_segment")
        if landed_segment is not None and not (
            isinstance(landed_segment, str)
            and _SEGMENT_KEY_RE.fullmatch(landed_segment)
        ):
            raise SimulationFailure("existing state has an invalid landed segment key")
        prior_response: HttpResponse | None = None
        raw_accepted = prior.get("accepted_response")
        if raw_accepted is not None:
            if not isinstance(raw_accepted, dict):
                raise SimulationFailure(
                    f"fixture {segment.fixture_id} accepted state omitted its response receipt"
                )
            prior_response = HttpResponse(
                status=int(raw_accepted["http_status"]),
                body=raw_accepted["body"],
            )
            accepted_landed = self._validate_upload_response(
                prior_response, segment, envelope
            )
            if accepted_landed != landed_segment:
                raise SimulationFailure(
                    f"fixture {segment.fixture_id} accepted state changed its landed segment"
                )
        elif prior_phase == "accepted":
            raise SimulationFailure(
                f"fixture {segment.fixture_id} accepted state omitted its response receipt"
            )
        retry_after_uncertainty = prior_phase in {"sending", "uncertain"}
        resumable = prior_phase in {
            "sending",
            "uncertain",
            "accepted",
            "reconciled",
            "complete",
        }
        if prior and resumable:
            item, journal_oracles, ready = self._reconcile(
                client,
                segment,
                mapped_day,
                landed_segment if isinstance(landed_segment, str) else None,
                wait=isinstance(landed_segment, str),
            )
            if item is not None and not ready:
                item, journal_oracles, ready = self._reconcile(
                    client, segment, mapped_day, str(item["key"]), wait=True
                )
            if item is not None and ready:
                self._verify_fixture_bytes(segment)
                return self._finish_segment(
                    client=client,
                    segment=segment,
                    mapped_day=mapped_day,
                    envelope=envelope,
                    item=item,
                    journal_oracles=journal_oracles,
                    upload_attempts=int(prior.get("upload_attempts", 0)),
                    resumed=True,
                    response=prior_response,
                )
            if item is not None or prior_phase in {"accepted", "reconciled", "complete"}:
                raise SimulationFailure(
                    f"fixture {segment.fixture_id} has landed state, but its required "
                    "custody or processing evidence did not become ready"
                )
        last_error: str | None = None
        total_attempts = int(prior.get("upload_attempts", 0))
        attempts_this_run = 0
        while attempts_this_run < self.config.max_attempts:
            response: HttpResponse | None = None
            entry = self.state["segments"].setdefault(segment.fixture_id, {})
            try:
                with self._fixture_uploads(segment) as uploads:
                    attempts_this_run += 1
                    total_attempts += 1
                    entry.update(
                        {
                            "mapped_day": mapped_day,
                            "requested_segment": segment.segment,
                            "upload_attempts": total_attempts,
                            "phase": "sending",
                        }
                    )
                    self._save_state()
                    self._record_upload_request(segment.fixture_id)
                    response = client.post_multipart(
                        "/app/devices/ingest", envelope, uploads
                    )
            except HttpResponseError as caught:
                self._record_upload_response(
                    segment.fixture_id, "primary", error=caught
                )
                entry.update(
                    {
                        "phase": "contract_failed",
                        "contract_failure": {
                            "kind": "primary_response",
                            "response_error": caught.receipt,
                        },
                    }
                )
                self._save_state()
                try:
                    recovered, _, _ = self._reconcile(
                        client, segment, mapped_day, None, wait=False
                    )
                except (HttpRequestError, SimulationFailure):
                    recovered = None
                if recovered is not None:
                    entry["contract_failure"]["reconciled_segment"] = recovered.get(
                        "key"
                    )
                    self._save_state()
                raise SimulationFailure(
                    f"fixture {segment.fixture_id} upload response violated the HTTP contract"
                ) from caught
            except HttpRequestError as caught:
                last_error = str(caught)
                retry_after_uncertainty = True
                entry.update(
                    {
                        "phase": "uncertain",
                        "last_uncertainty": last_error,
                    }
                )
                self._save_state()
            else:
                self._record_upload_response(segment.fixture_id, "primary", response)
                response_status = response.body.get("status")
                if response.status == 200:
                    try:
                        recovery_duplicate = (
                            response_status == "duplicate" and retry_after_uncertainty
                        )
                        if (
                            response_status not in segment.expectation.upload_statuses
                            and not recovery_duplicate
                        ):
                            raise SimulationFailure(
                                f"fixture {segment.fixture_id} expected upload status "
                                f"{segment.expectation.upload_statuses}, got {response_status!r}"
                            )
                        landed_segment = self._validate_upload_response(
                            response, segment, envelope
                        )
                    except SimulationFailure as caught:
                        entry.update(
                            {
                                "phase": "contract_failed",
                                "contract_failure": {
                                    "kind": "primary_response",
                                    "http_status": response.status,
                                    "body": response.body,
                                    "detail": str(caught),
                                },
                            }
                        )
                        self._save_state()
                        raise
                    entry.update(
                        {
                            "phase": "accepted",
                            "landed_segment": landed_segment,
                            "last_response_status": response.status,
                            "accepted_response": {
                                "http_status": response.status,
                                "body": response.body,
                            },
                        }
                    )
                    entry.pop("contract_failure", None)
                    self._save_state()
                elif response.status >= 500:
                    last_error = (
                        f"HTTP {response.status} "
                        f"({response.body.get('reason_code', 'unknown')})"
                    )
                    retry_after_uncertainty = True
                    entry.update(
                        {
                            "phase": "uncertain",
                            "last_response_status": response.status,
                            "last_uncertainty": last_error,
                        }
                    )
                    self._save_state()
                else:
                    reason = response.body.get("reason_code", "unknown")
                    detail = (
                        f"fixture {segment.fixture_id} returned non-contract HTTP "
                        f"{response.status} ({reason})"
                    )
                    entry.update(
                        {
                            "phase": "contract_failed",
                            "last_response_status": response.status,
                            "contract_failure": {
                                "kind": "primary_response",
                                "http_status": response.status,
                                "body": response.body,
                                "detail": detail,
                            },
                        }
                    )
                    self._save_state()
                    raise SimulationFailure(
                        detail
                    )
            try:
                item, journal_oracles, ready = self._reconcile(
                    client,
                    segment,
                    mapped_day,
                    landed_segment,
                    wait=landed_segment is not None,
                )
            except HttpResponseError:
                raise
            except HttpRequestError as error:
                last_error = str(error)
                retry_after_uncertainty = True
                continue
            if item is not None:
                landed_segment = str(item["key"])
                if not ready:
                    item, journal_oracles, ready = self._reconcile(
                        client, segment, mapped_day, landed_segment, wait=True
                    )
                if not ready:
                    raise SimulationFailure(
                        f"fixture {segment.fixture_id} landed as {landed_segment}, but its "
                        "required custody or processing evidence did not become ready"
                    )
                return self._finish_segment(
                    client=client,
                    segment=segment,
                    mapped_day=mapped_day,
                    envelope=envelope,
                    item=item,
                    journal_oracles=journal_oracles,
                    upload_attempts=total_attempts,
                    resumed=False,
                    response=response,
                )
            if response is not None and response.status == 200:
                raise SimulationFailure(
                    f"fixture {segment.fixture_id} was accepted as {landed_segment}, but "
                    "the receiver did not attest it through the listing contract"
                )
            retry_after_uncertainty = True
        raise SimulationInconclusive(
            f"fixture {segment.fixture_id} did not reconcile after {attempts_this_run} "
            f"attempts in this invocation ({total_attempts} total); "
            f"last uncertainty: {last_error or 'no matching listing'}"
        )

    @staticmethod
    def _require_get_ok(response: HttpResponse, label: str) -> dict[str, Any]:
        if response.status != 200:
            reason = response.body.get("reason_code", "unknown")
            raise SimulationFailure(
                f"{label} returned HTTP {response.status} ({reason})"
            )
        return response.body

    def _contract_reads(self, client: BridgeHttpClient) -> None:
        by_id = {segment.fixture_id: segment for segment in self.segments}
        groups: dict[tuple[str, str], list[tuple[FixtureSegment, str]]] = {}
        for result in self.evidence["segments"]:
            segment = by_id[result["fixture_id"]]
            groups.setdefault((result["mapped_day"], segment.source), []).append(
                (segment, result["landed_segment"])
            )
        receipts: list[dict[str, Any]] = []
        self.evidence["contract_reads"] = receipts
        for (day, source), entries in sorted(groups.items()):
            receipt: dict[str, Any] = {
                "day": day,
                "source": source,
                "segments": None,
                "manifest_day": None,
                "manifest": None,
            }
            receipts.append(receipt)
            first = entries[0][0]
            listing = self._listing(client, first, day)
            receipt["segments"] = {"http_status": 200, "body": listing}
            query = {"source": source} if source else None
            day_response = self._get_json(
                client,
                f"/app/devices/ingest/manifest/{day}",
                query,
                purpose=f"day manifest {day}/{source}",
            )
            receipt["manifest_day"] = {
                "http_status": day_response.status,
                "body": day_response.body,
            }
            day_manifest = self._require_get_ok(
                day_response, f"ingest day manifest {day}/{source}"
            )
            if (
                day_manifest.get("version") != 1
                or day_manifest.get("day") != day
                or not isinstance(day_manifest.get("segments"), dict)
            ):
                raise SimulationFailure(
                    f"ingest day manifest {day}/{source} has the wrong shape"
                )
            listed_segments = day_manifest["segments"]
            for segment, landed in entries:
                item = self._find_listing_item(listing, segment, landed)
                if item is None:
                    raise SimulationFailure(
                        f"final listing omitted fixture {segment.fixture_id}"
                    )
                original = item.get("original_key")
                if original is not None and original != segment.segment:
                    raise SimulationFailure(
                        f"final listing lost requested-key lineage for {segment.fixture_id}"
                    )
                raw_day_entry = listed_segments.get(landed)
                if not isinstance(raw_day_entry, dict):
                    raise SimulationFailure(
                        f"day manifest omitted fixture {segment.fixture_id}"
                    )
                matched = self._matched_files(raw_day_entry, segment)
                if matched is None or not {
                    entry.get("status") for entry in matched
                }.issubset(set(segment.expectation.file_statuses)):
                    raise SimulationFailure(
                        f"day manifest files do not attest fixture {segment.fixture_id}"
                    )
            root_response = self._get_json(
                client,
                "/app/devices/ingest/manifest",
                query,
                purpose=f"root manifest {source}",
            )
            receipt["manifest"] = {
                "http_status": root_response.status,
                "body": root_response.body,
            }
            root_manifest = self._require_get_ok(
                root_response, f"ingest manifest {source}"
            )
            days = root_manifest.get("days")
            day_summary = days.get(day) if isinstance(days, dict) else None
            segment_count = (
                day_summary.get("segments")
                if isinstance(day_summary, dict)
                else None
            )
            distinct_landed = len({landed for _, landed in entries})
            if (
                isinstance(segment_count, bool)
                or not isinstance(segment_count, int)
                or segment_count < distinct_landed
            ):
                raise SimulationFailure(
                    f"ingest manifest {source} does not count mapped day {day}"
                )

    def _persist_evidence(self, phase: str) -> None:
        try:
            _atomic_json(self.config.evidence_path, self.evidence)
        except OSError as caught:
            raise SimulationFailure(
                f"{phase} evidence write failed at {self.config.evidence_path}: "
                f"{type(caught).__name__}"
            ) from caught

    def _write_evidence(self, outcome: RunOutcome, error: str | None) -> None:
        self.evidence["finished_at"] = _utc_now()
        self.evidence["result"] = outcome.value
        self.evidence["error"] = error
        self._persist_evidence(f"terminal {outcome.value}")

    def _run_with_client(self, client: BridgeHttpClient) -> RunOutcome:
        self._bind_receiver(client)
        for segment in self.segments:
            self.evidence["segments"].append(self._upload_one(client, segment))
        self._contract_reads(client)
        return RunOutcome.PASS

    def run(self) -> RunOutcome:
        self._persist_evidence("initial")
        bridge: LinkBridge | None = None
        outcome: RunOutcome | None = None
        error: str | None = None
        try:
            if self.config.bridge_url:
                base_url = self.config.bridge_url
            else:
                bridge = LinkBridge(
                    solstone_bin=self.config.solstone_bin,
                    pair_code=self.config.pair_code,
                    state_dir=self.config.state_dir,
                    carrier=self.config.carrier,
                    relay_url=self.config.relay_url,
                    convey_port=self.config.convey_port,
                    startup_timeout=self.config.request_timeout,
                )
                base_url = bridge.start()
                self.evidence["bridge"]["carrier_assurance"] = (
                    "native-direct-only"
                    if self.config.carrier == "direct"
                    else "native-relay-only"
                )
                bridge_provenance = bridge.provenance
                credentials = bridge_provenance.get("credentials")
                client_cid = (
                    credentials.get("client_cid")
                    if isinstance(credentials, dict)
                    else None
                )
                if not isinstance(client_cid, str):
                    raise LinkProcessError(
                        "native bridge did not expose its authenticated client CID"
                    )
                self.expected_cid = client_cid
                self.evidence["bridge"]["provenance"] = bridge_provenance
            client = BridgeHttpClient(base_url, timeout=self.config.request_timeout)
            outcome = self._run_with_client(client)
            if bridge is not None:
                self._verify_local_bridge_status(client)
        except (ManifestError, SimulationFailure, HttpResponseError) as caught:
            outcome = RunOutcome.FAIL
            error = str(caught)
        except LinkProcessError as caught:
            outcome = RunOutcome.BLOCKED
            error = str(caught)
        except (SimulationInconclusive, HttpRequestError) as caught:
            outcome = RunOutcome.INCONCLUSIVE
            error = str(caught)
        finally:
            if bridge:
                try:
                    if outcome is RunOutcome.PASS:
                        bridge.finish(
                            remove_credentials=not self.config.keep_credentials
                        )
                    else:
                        bridge.stop()
                except LinkProcessError as cleanup_error:
                    previous = f"; prior outcome: {error}" if error else ""
                    outcome = RunOutcome.BLOCKED
                    error = f"native bridge finalization failed: {cleanup_error}{previous}"
                provenance = getattr(bridge, "provenance", None)
                if provenance is not None:
                    self.evidence["bridge"]["provenance"] = provenance
        assert outcome is not None
        self._write_evidence(outcome, error)
        return outcome
