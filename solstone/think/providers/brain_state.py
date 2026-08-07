# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Native transport for active-brain state.

``solstone-core brain`` owns persisted records, fingerprint computation, and
write fencing.  This module preserves the Python-facing types and call shapes
while forwarding every stateful operation to that native owner.
"""

from __future__ import annotations

import hashlib
import json
import subprocess
import sys
import threading
from collections.abc import Callable, Mapping
from dataclasses import dataclass
from datetime import datetime, timedelta, timezone
from pathlib import Path
from queue import Empty, Queue
from typing import Any, Literal, NotRequired, TextIO, TypedDict, cast

from solstone.think import core_handshake
from solstone.think.journal_io.lease import probe_file_lease_held
from solstone.think.models import DEFAULT_MODEL_BY_PROVIDER, LOCAL_MODEL
from solstone.think.providers.local_endpoint import (
    confidential_provenance_block,
    normalize_local_endpoint_url,
)
from solstone.think.providers.runtime_health import RuntimePhase
from solstone.think.utils import get_journal

_LOCAL_CONTRACT_PATH = Path(__file__).parents[3] / "core/fixtures/local_contract.json"
# This bounds a response after the caller has supplied a complete request; it
# is deliberately independent from the child's 90-second input deadline.
_SESSION_RESPONSE_TIMEOUT_SECONDS = 10.0


def _local_contract() -> dict[str, Any]:
    with _LOCAL_CONTRACT_PATH.open(encoding="utf-8") as handle:
        contract = json.load(handle)
    brain_state = contract.get("brain_state")
    if not isinstance(brain_state, dict):
        raise RuntimeError("local_contract.json has no brain_state object")
    return brain_state


_BRAIN = _local_contract()

SCHEMA_VERSION = cast(int, _BRAIN["schema_version"])
FINGERPRINT_SCHEMA_VERSION = cast(int, _BRAIN["fingerprint_schema_version"])
BRAIN_FILE_MODE = int(cast(str, _BRAIN["file_mode_octal"]), 8)
FINGERPRINT_KEY_BYTES = cast(int, _BRAIN["fingerprint_key_bytes"])
CHECKING_TTL = timedelta(seconds=cast(int, _BRAIN["checking_ttl_seconds"]))
DEFAULT_READY_EVIDENCE_TTL = timedelta(
    seconds=cast(int, _BRAIN["default_ready_evidence_ttl_seconds"])
)

CLOUD_BYO_PROVIDERS = frozenset(cast(list[str], _BRAIN["cloud_byo_providers"]))
PROVIDER_ENV_BY_NAME = dict(cast(dict[str, str], _BRAIN["provider_env_by_name"]))
COMPONENT_ORDER = tuple(cast(list[str], _BRAIN["component_order"]))
LANE_COMPONENTS = {
    lane: tuple(components)
    for lane, components in cast(dict[str, list[str]], _BRAIN["lane_components"]).items()
}

BrainAggregateState = Literal[*cast(list[str], _BRAIN["aggregate_states"])]
BrainComponentStatus = Literal[*cast(list[str], _BRAIN["component_statuses"])]
BrainLaneId = Literal[*cast(list[str], _BRAIN["lanes"])]
BrainInspectionStatus = Literal[*cast(list[str], _BRAIN["inspection_statuses"])]
BrainReasonCode = Literal[*cast(list[str], _BRAIN["reason_codes"])]
BrainRuntimeFailureComponent = Literal[*cast(list[str], _BRAIN["runtime_failure_components"])]
BrainRuntimeFailureRejectedReason = Literal[
    *cast(list[str], _BRAIN["runtime_failure_rejected_reasons"])
]
BrainPrerequisiteRenewalStatus = Literal[
    *cast(list[str], _BRAIN["prerequisite_renewal_statuses"])
]
BrainDiagnosticValue = str | int | float | bool

BRAIN_AGGREGATE_STATES = frozenset(cast(list[str], _BRAIN["aggregate_states"]))
BRAIN_COMPONENT_STATUSES = frozenset(cast(list[str], _BRAIN["component_statuses"]))
BRAIN_LANES = frozenset(cast(list[str], _BRAIN["lanes"]))
BRAIN_REASON_CODES = frozenset(cast(list[str], _BRAIN["reason_codes"]))
BRAIN_EVIDENCE_REASON_CODES = {
    name: frozenset(reasons)
    for name, reasons in cast(dict[str, list[str]], _BRAIN["evidence_reason_codes"]).items()
}
BRAIN_PROJECTION_ONLY_REASON_CODES = frozenset(
    cast(list[str], _BRAIN["projection_only_reason_codes"])
)
BRAIN_REASON_TO_AGGREGATE = dict(cast(dict[str, str], _BRAIN["reason_to_aggregate"]))
RUNTIME_FAILURE_AGGREGATES = frozenset(
    cast(list[str], _BRAIN["runtime_failure_aggregates"])
)
RUNTIME_PHASE_TO_REASON = dict(cast(dict[str, str | None], _BRAIN["runtime_phase_to_reason"]))
RUNTIME_REASON_TO_BRAIN_REASON = dict(
    cast(dict[str, str], _BRAIN["runtime_reason_to_brain_reason"])
)
RUNTIME_PHASES = frozenset(cast(list[str], _BRAIN["runtime_phases"]))
RUNTIME_REASON_CODES = frozenset(cast(list[str], _BRAIN["runtime_reason_codes"]))
INCOHERENT_RUNTIME_PHASE_REASON_CODES = frozenset(
    tuple(pair)
    for pair in cast(list[list[str]], _BRAIN["incoherent_runtime_phase_reason_codes"])
)
RUNTIME_TRANSITION_PHASES = frozenset(
    cast(list[str], _BRAIN["runtime_transition_phases"])
)
CONFIG_DIAGNOSTIC_FIELDS = frozenset(
    cast(list[str], _BRAIN["config_diagnostic_fields"])
)
DIAGNOSTIC_METADATA_SCHEMAS = {
    reason: {field: frozenset(values) for field, values in fields.items()}
    for reason, fields in cast(
        dict[str, dict[str, list[str]]], _BRAIN["diagnostic_metadata_schemas"]
    ).items()
}
_RECORD_FIELDS = cast(dict[str, list[str]], _BRAIN["record_fields"])
BRAIN_TOP_LEVEL_FIELDS = set(_RECORD_FIELDS["top_level"])
BRAIN_CHECKING_FIELDS = set(_RECORD_FIELDS["checking"])
BRAIN_EVIDENCE_FIELDS = set(_RECORD_FIELDS["evidence"])
BRAIN_EVIDENCE_COMPONENT_FIELDS = set(_RECORD_FIELDS["evidence_component"])
BRAIN_RUNTIME_FAILURE_MARKER_FIELDS = set(_RECORD_FIELDS["runtime_failure_marker"])


class BrainEvidenceComponent(TypedDict):
    status: BrainComponentStatus
    observed_at: str
    reason_code: NotRequired[BrainReasonCode | None]
    expires_at: NotRequired[str | None]
    diagnostic: NotRequired[dict[str, BrainDiagnosticValue]]


class BrainEvidenceRecord(TypedDict):
    configuration: BrainEvidenceComponent | None
    lane_prerequisites: BrainEvidenceComponent | None
    generate: BrainEvidenceComponent | None
    cogitate: BrainEvidenceComponent | None


class BrainCheckingRecord(TypedDict):
    run_id: str
    started_at: str
    expires_at: str
    fingerprint_sha256: str | None
    checking_revision: int
    runtime_failure_marker_seen: str | None


class BrainRuntimeFailureMarker(TypedDict):
    marker_id: str
    revision: int
    recorded_at: str
    reason_code: BrainReasonCode


class BrainStateRecord(TypedDict):
    schema_version: int
    revision: int
    aggregate_state: BrainAggregateState
    reason_code: BrainReasonCode | None
    active_lane: BrainLaneId
    active_provider: str | None
    active_model: str | None
    fingerprint_sha256: str | None
    checking: BrainCheckingRecord | None
    evidence: BrainEvidenceRecord
    runtime_failure_marker: BrainRuntimeFailureMarker | None
    diagnostic: dict[str, BrainDiagnosticValue]
    updated_at: str


class BrainProjection(TypedDict):
    aggregate_state: BrainAggregateState
    reason_code: BrainReasonCode | None
    active_lane: BrainLaneId | None
    active_provider: str | None
    active_model: str | None
    fingerprint_sha256: str | None
    runtime_transition_in_progress: bool


class BrainStateInspection(TypedDict):
    status: BrainInspectionStatus
    path: str
    record: BrainStateRecord | None
    projection: BrainProjection
    reason_code: BrainReasonCode | None
    error: str | None


class BrainFingerprintResult(TypedDict):
    ok: bool
    fingerprint_sha256: str | None
    active_lane: BrainLaneId | None
    active_provider: str | None
    active_model: str | None
    reason_code: BrainReasonCode | None
    diagnostic: dict[str, BrainDiagnosticValue]
    bundled_runtime_fingerprint_sha256: NotRequired[str | None]


class BrainProbeOutcome(TypedDict):
    configuration: BrainEvidenceComponent | None
    lane_prerequisites: BrainEvidenceComponent | None
    generate: BrainEvidenceComponent | None
    cogitate: BrainEvidenceComponent | None


class BrainRuntimeFailureResult(TypedDict):
    accepted: bool
    record: BrainStateRecord | None
    rejected_reason: BrainRuntimeFailureRejectedReason | None
    error: str | None


class BrainPrerequisiteRenewalBeginResult(TypedDict):
    status: BrainPrerequisiteRenewalStatus
    permit: NotRequired["BrainRefreshPermit"]
    reason: NotRequired[str]


class BrainStateValidationError(ValueError):
    """Retained public validation exception for input/transport validation failures."""

    def __init__(self, path: str, reason: str):
        self.path = path
        self.reason = reason
        super().__init__(f"{path}: {reason}")


class BrainStateConflictError(RuntimeError):
    """Raised when the native writer refuses a stale session permit."""


class BrainStateExpectedFingerprintStaleError(BrainStateConflictError):
    """Raised when a native refresh precondition reports stale identity."""


class _NativeBrainTransportError(RuntimeError):
    pass


def _journal_root(journal_path: str | Path | None) -> Path:
    return Path(journal_path) if journal_path is not None else Path(get_journal())


def brain_state_path(*, journal_path: str | Path | None = None) -> Path:
    return _journal_root(journal_path) / _BRAIN["paths"]["record"]


def brain_fingerprint_key_path(*, journal_path: str | Path | None = None) -> Path:
    return _journal_root(journal_path) / _BRAIN["paths"]["fingerprint_key"]


def brain_refresh_lease_path(*, journal_path: str | Path | None = None) -> Path:
    return _journal_root(journal_path) / _BRAIN["paths"]["refresh_lease"]


def probe_brain_refresh_lease_held(*, journal_path: str | Path | None = None) -> bool:
    return probe_file_lease_held(brain_refresh_lease_path(journal_path=journal_path))


def _utc(now: datetime) -> datetime:
    if now.tzinfo is None or now.utcoffset() is None:
        raise ValueError("brain state timestamps require timezone-aware datetimes")
    return now.astimezone(timezone.utc)


def _bundled_runtime_fingerprint_sha() -> str:
    if sys.platform == "darwin":
        from solstone.think.providers import mlx_install

        target = mlx_install.target_fingerprint()
    else:
        from solstone.think.providers import local_install

        target = local_install.target_fingerprint(LOCAL_MODEL)
    from solstone.think.providers.install_state import (
        canonical_fingerprint,
        fingerprint_sha256,
    )

    return fingerprint_sha256(canonical_fingerprint(target))


def _bundled_runtime_fingerprint(
    provided: str | None = None,
) -> str | None:
    if provided is not None:
        return provided
    try:
        return _bundled_runtime_fingerprint_sha()
    except Exception:
        return None


HandshakeChecker = Callable[[], core_handshake.CoreHandshakeResult]
HelperLocator = Callable[[], Path]
NativeRunner = Callable[..., subprocess.CompletedProcess[str]]
PopenFactory = Callable[..., subprocess.Popen[str]]


def _native_binary(
    *,
    handshake_checker: HandshakeChecker | None = None,
    helper_locator: HelperLocator | None = None,
) -> Path:
    handshake_checker = handshake_checker or core_handshake.check_solstone_core_handshake
    helper_locator = helper_locator or core_handshake.helper_path_for_executable
    handshake = handshake_checker()
    if handshake.status != "ok":
        detail = handshake.message or "unknown solstone-core handshake failure"
        raise _NativeBrainTransportError(f"brain state requires solstone-core: {detail}")
    return helper_locator()


def _native_error(prefix: str, returncode: int | None, stderr: str) -> _NativeBrainTransportError:
    detail = stderr.strip() or "no diagnostic output"
    return _NativeBrainTransportError(f"{prefix} failed (exit {returncode}): {detail}")


def _run_native(
    args: list[str],
    *,
    request: Mapping[str, Any] | None = None,
    handshake_checker: HandshakeChecker | None = None,
    helper_locator: HelperLocator | None = None,
    native_runner: NativeRunner | None = None,
) -> dict[str, Any]:
    binary = _native_binary(
        handshake_checker=handshake_checker,
        helper_locator=helper_locator,
    )
    native_runner = native_runner or subprocess.run
    try:
        completed = native_runner(
            [str(binary), "brain", *args],
            input=(json.dumps(request, allow_nan=False) if request is not None else None),
            capture_output=True,
            text=True,
            timeout=_SESSION_RESPONSE_TIMEOUT_SECONDS,
            check=False,
        )
    except (OSError, subprocess.TimeoutExpired) as exc:
        raise _NativeBrainTransportError(f"brain native command could not run: {exc}") from exc
    if completed.returncode != 0:
        raise _native_error("brain native command", completed.returncode, completed.stderr)
    try:
        response = json.loads(completed.stdout)
    except json.JSONDecodeError as exc:
        raise _NativeBrainTransportError("brain native command returned invalid JSON") from exc
    if not isinstance(response, dict):
        raise _NativeBrainTransportError("brain native command returned a non-object response")
    return response


def _read_line(stream: TextIO, *, context: str) -> str:
    result: Queue[str | BaseException] = Queue(maxsize=1)

    def read() -> None:
        try:
            result.put(stream.readline())
        except BaseException as exc:  # pragma: no cover - pipe failures are platform-specific
            result.put(exc)

    thread = threading.Thread(target=read, daemon=True)
    thread.start()
    try:
        value = result.get(timeout=_SESSION_RESPONSE_TIMEOUT_SECONDS)
    except Empty as exc:
        raise _NativeBrainTransportError(f"brain session timed out waiting for {context}") from exc
    if isinstance(value, BaseException):
        raise _NativeBrainTransportError(f"brain session could not read {context}: {value}")
    if not value:
        raise _NativeBrainTransportError(f"brain session ended before {context}")
    return value


@dataclass
class _StderrDrain:
    stream: TextIO
    _chunks: list[str]
    _thread: threading.Thread

    @classmethod
    def start(cls, stream: TextIO) -> "_StderrDrain":
        chunks: list[str] = []
        thread = threading.Thread(target=lambda: chunks.append(stream.read()), daemon=True)
        thread.start()
        return cls(stream, chunks, thread)

    def text(self) -> str:
        self._thread.join(timeout=1)
        return "".join(self._chunks)


@dataclass
class _BrainSessionClient:
    process: subprocess.Popen[str]
    stderr: _StderrDrain
    result_schema: str
    ready_schema: str
    probe_schema: str
    abandon_schema: str
    terminal_schema: str
    journal_path: str | Path | None
    _closed: bool = False

    @classmethod
    def start(
        cls,
        verb: str,
        *,
        argv: list[str],
        schemas: tuple[str, str, str, str, str],
        journal_path: str | Path | None,
        stale_on_dataerr: bool = False,
        handshake_checker: HandshakeChecker | None = None,
        helper_locator: HelperLocator | None = None,
        popen_factory: PopenFactory | None = None,
    ) -> "_BrainSessionClient | dict[str, Any]":
        binary = _native_binary(
            handshake_checker=handshake_checker,
            helper_locator=helper_locator,
        )
        popen_factory = popen_factory or subprocess.Popen
        try:
            process = popen_factory(
                [str(binary), "brain", verb, "--session", *argv],
                stdin=subprocess.PIPE,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                text=True,
            )
        except OSError as exc:
            raise _NativeBrainTransportError(f"brain {verb} session could not start: {exc}") from exc
        assert process.stdout is not None and process.stderr is not None
        stderr = _StderrDrain.start(process.stderr)
        try:
            line = _read_line(process.stdout, context="the initial ready/result record")
        except _NativeBrainTransportError:
            try:
                returncode = process.wait(timeout=_SESSION_RESPONSE_TIMEOUT_SECONDS)
            except subprocess.TimeoutExpired:
                process.kill()
                returncode = process.wait(timeout=_SESSION_RESPONSE_TIMEOUT_SECONDS)
            error = _native_error(f"brain {verb} session", returncode, stderr.text())
            if stale_on_dataerr and returncode == 65:
                raise BrainStateExpectedFingerprintStaleError(str(error)) from error
            if returncode == 64:
                raise ValueError(str(error)) from error
            raise error
        try:
            record = json.loads(line)
        except json.JSONDecodeError as exc:
            process.terminate()
            process.wait(timeout=_SESSION_RESPONSE_TIMEOUT_SECONDS)
            raise _NativeBrainTransportError("brain session returned invalid initial JSON") from exc
        if not isinstance(record, dict):
            process.terminate()
            process.wait(timeout=_SESSION_RESPONSE_TIMEOUT_SECONDS)
            raise _NativeBrainTransportError("brain session returned a non-object initial record")
        result_schema, ready_schema, probe_schema, abandon_schema, terminal_schema = schemas
        if record.get("schema") == ready_schema:
            return cls(
                process,
                stderr,
                result_schema,
                ready_schema,
                probe_schema,
                abandon_schema,
                terminal_schema,
                journal_path,
            )
        if record.get("schema") == result_schema:
            returncode = process.wait(timeout=_SESSION_RESPONSE_TIMEOUT_SECONDS)
            if returncode != 0:
                raise _native_error(f"brain {verb} session", returncode, stderr.text())
            return record
        process.terminate()
        process.wait(timeout=_SESSION_RESPONSE_TIMEOUT_SECONDS)
        raise _NativeBrainTransportError("brain session returned an unknown initial schema")

    @property
    def owned(self) -> bool:
        return not self._closed and self.process.poll() is None

    def _send(self, record: Mapping[str, Any]) -> None:
        if not self.owned or self.process.stdin is None:
            raise BrainStateConflictError("brain refresh permit is no longer owned")
        try:
            self.process.stdin.write(json.dumps(record, allow_nan=False) + "\n")
            self.process.stdin.flush()
        except (BrokenPipeError, OSError) as exc:
            raise _NativeBrainTransportError("brain session pipe closed while writing") from exc

    def _complete(self, request: Mapping[str, Any]) -> dict[str, Any]:
        self._send(request)
        self._send({"schema": self.terminal_schema})
        assert self.process.stdin is not None
        self.process.stdin.close()
        try:
            line = _read_line(self.process.stdout, context="the terminal result record")  # type: ignore[arg-type]
            record = json.loads(line)
        except (json.JSONDecodeError, _NativeBrainTransportError) as exc:
            self.release()
            raise BrainStateConflictError(f"brain session did not return a valid result: {exc}") from exc
        if not isinstance(record, dict) or record.get("schema") != self.result_schema:
            self.release()
            raise BrainStateConflictError("brain session returned an unexpected result schema")
        self._closed = True
        try:
            returncode = self.process.wait(timeout=_SESSION_RESPONSE_TIMEOUT_SECONDS)
        except subprocess.TimeoutExpired as exc:
            self.process.kill()
            raise _NativeBrainTransportError("brain session did not exit after its result") from exc
        if returncode != 0:
            raise BrainStateConflictError(
                str(_native_error("brain session", returncode, self.stderr.text()))
            )
        return record

    def finish(self, outcome: Mapping[str, Any]) -> dict[str, Any]:
        return self._complete({"schema": self.probe_schema, "outcome": dict(outcome)})

    def finish_prerequisite(self, component: Mapping[str, Any]) -> dict[str, Any]:
        return self._complete(
            {"schema": self.probe_schema, "lane_prerequisites": dict(component)}
        )

    def abandon(
        self,
        reason_code: str,
        diagnostic: Mapping[str, BrainDiagnosticValue] | None,
    ) -> dict[str, Any]:
        return self._complete(
            {
                "schema": self.abandon_schema,
                "reason_code": reason_code,
                "diagnostic": dict(diagnostic or {}),
            }
        )

    def release(self) -> None:
        if self._closed:
            return
        self._closed = True
        if self.process.stdin is not None:
            try:
                self.process.stdin.close()
            except OSError:
                pass
        if self.process.poll() is None:
            self.process.terminate()
            try:
                self.process.wait(timeout=_SESSION_RESPONSE_TIMEOUT_SECONDS)
            except subprocess.TimeoutExpired:
                self.process.kill()
                self.process.wait(timeout=_SESSION_RESPONSE_TIMEOUT_SECONDS)


@dataclass
class BrainRefreshPermit:
    """A Python wrapper around the native child holding the refresh lease."""

    _session: _BrainSessionClient

    @property
    def owned(self) -> bool:
        return self._session.owned

    def release(self) -> None:
        self._session.release()


_REFRESH_SCHEMAS = (
    "solstone.brain.refresh.result.v1",
    "solstone.brain.refresh.ready.v1",
    "solstone.brain.refresh.probe.v1",
    "solstone.brain.refresh.abandon.v1",
    "solstone.brain.refresh.terminal.v1",
)
_PREREQUISITE_RENEWAL_SCHEMAS = (
    "solstone.brain.prerequisite_renewal.result.v1",
    "solstone.brain.prerequisite_renewal.ready.v1",
    "solstone.brain.prerequisite_renewal.probe.v1",
    "solstone.brain.prerequisite_renewal.abandon.v1",
    "solstone.brain.prerequisite_renewal.terminal.v1",
)


def _journal_args(journal_path: str | Path | None) -> list[str]:
    return ["--journal", str(journal_path)] if journal_path is not None else []


def _sha256(value: str, field: str) -> str:
    if len(value) != 64 or not value.isascii() or not all(char in "0123456789abcdefABCDEF" for char in value):
        raise BrainStateValidationError(field, "expected SHA-256 hex string")
    return value


def begin_brain_refresh(
    now: datetime,
    *,
    run_id: str | None = None,
    expected_active_fingerprint_sha256: str | None = None,
    expect_active_fingerprint_absent: bool = False,
    journal_path: str | Path | None = None,
) -> BrainRefreshPermit | None:
    _utc(now)
    if expected_active_fingerprint_sha256 is not None:
        try:
            _sha256(expected_active_fingerprint_sha256, "expected_active_fingerprint_sha256")
        except BrainStateValidationError as exc:
            raise BrainStateExpectedFingerprintStaleError(str(exc)) from exc
    if expected_active_fingerprint_sha256 is not None and expect_active_fingerprint_absent:
        raise ValueError("expected active fingerprint and expected absence are mutually exclusive")
    argv = _journal_args(journal_path)
    if run_id is not None:
        argv.extend(["--run-id", run_id])
    if expected_active_fingerprint_sha256 is not None:
        argv.extend(["--expect-fingerprint", expected_active_fingerprint_sha256])
    elif expect_active_fingerprint_absent:
        argv.append("--expect-absent")
    bundled = _bundled_runtime_fingerprint()
    if bundled is not None:
        argv.extend(["--bundled-runtime-fingerprint", bundled])
    started = _BrainSessionClient.start(
        "refresh",
        argv=argv,
        schemas=_REFRESH_SCHEMAS,
        journal_path=journal_path,
        stale_on_dataerr=True,
    )
    return BrainRefreshPermit(started) if isinstance(started, _BrainSessionClient) else None


def begin_brain_prerequisite_renewal(
    now: datetime,
    *,
    expected_fingerprint_sha256: str | None = None,
    run_id: str | None = None,
    journal_path: str | Path | None = None,
) -> BrainPrerequisiteRenewalBeginResult:
    _utc(now)
    argv = _journal_args(journal_path)
    if run_id is not None:
        argv.extend(["--run-id", run_id])
    if expected_fingerprint_sha256 is not None:
        argv.extend(["--expect-fingerprint", expected_fingerprint_sha256])
    bundled = _bundled_runtime_fingerprint()
    if bundled is not None:
        argv.extend(["--bundled-runtime-fingerprint", bundled])
    started = _BrainSessionClient.start(
        "prerequisite-renewal",
        argv=argv,
        schemas=_PREREQUISITE_RENEWAL_SCHEMAS,
        journal_path=journal_path,
    )
    if isinstance(started, _BrainSessionClient):
        return {"status": "started", "permit": BrainRefreshPermit(started)}
    status = started.get("status")
    reason = started.get("reason")
    if status not in {"busy", "unsafe"} or not isinstance(reason, str):
        raise _NativeBrainTransportError("native prerequisite renewal returned an invalid result")
    return {"status": cast(BrainPrerequisiteRenewalStatus, status), "reason": reason}


def _finished_record(permit: BrainRefreshPermit) -> BrainStateRecord:
    inspection = inspect_brain_state(datetime.now(timezone.utc), journal_path=permit._session.journal_path)
    record = inspection["record"]
    if record is None:
        raise BrainStateConflictError("native brain session completed without a record")
    return record


def finish_brain_refresh(
    permit: BrainRefreshPermit,
    outcome: BrainProbeOutcome,
    now: datetime,
    *,
    journal_path: str | Path | None = None,
) -> BrainStateRecord:
    _utc(now)
    permit._session.finish(outcome)
    return _finished_record(permit)


def abandon_brain_refresh(
    permit: BrainRefreshPermit,
    reason_code: BrainReasonCode,
    now: datetime,
    *,
    diagnostic: Mapping[str, BrainDiagnosticValue] | None = None,
    journal_path: str | Path | None = None,
) -> BrainStateRecord:
    _utc(now)
    permit._session.abandon(reason_code, diagnostic)
    return _finished_record(permit)


def finish_brain_prerequisite_renewal(
    permit: BrainRefreshPermit,
    lane_prerequisites: Mapping[str, Any],
    now: datetime,
    *,
    journal_path: str | Path | None = None,
) -> BrainStateRecord:
    _utc(now)
    permit._session.finish_prerequisite(lane_prerequisites)
    return _finished_record(permit)


def abandon_brain_prerequisite_renewal(
    permit: BrainRefreshPermit,
    reason_code: BrainReasonCode,
    now: datetime,
    *,
    diagnostic: Mapping[str, BrainDiagnosticValue] | None = None,
    journal_path: str | Path | None = None,
) -> BrainStateRecord:
    _utc(now)
    permit._session.abandon(reason_code, diagnostic)
    return _finished_record(permit)


def record_brain_runtime_failure(
    reason_code: BrainReasonCode,
    now: datetime,
    *,
    expected_fingerprint_sha256: str,
    component: BrainRuntimeFailureComponent,
    diagnostic: Mapping[str, BrainDiagnosticValue] | None = None,
    journal_path: str | Path | None = None,
) -> BrainRuntimeFailureResult:
    try:
        _utc(now)
    except ValueError as exc:
        return {
            "accepted": False,
            "record": None,
            "rejected_reason": "state_unavailable",
            "error": str(exc),
        }
    response = _run_native(
        ["record-runtime-failure", *_journal_args(journal_path)],
        request={
            "reason_code": reason_code,
            "component": component,
            "expected_fingerprint_sha256": expected_fingerprint_sha256,
            "diagnostic": dict(diagnostic or {}),
            "bundled_runtime_fingerprint_sha256": _bundled_runtime_fingerprint(),
        },
    )
    return cast(BrainRuntimeFailureResult, response)


def inspect_brain_state(
    now: datetime,
    *,
    journal_path: str | Path | None = None,
    config: Mapping[str, Any] | None = None,
) -> BrainStateInspection:
    _utc(now)
    # Native inspection intentionally owns wall-clock projection and config reads.
    # Retain these arguments for API compatibility; kept callers do not assert a
    # real transport result against injected values.
    del config
    bundled = _bundled_runtime_fingerprint()
    argv = ["inspect", *_journal_args(journal_path)]
    if bundled is not None:
        argv.extend(["--bundled-runtime-fingerprint", bundled])
    return cast(BrainStateInspection, _run_native(argv))


def read_active_brain_fingerprint_sha256(
    *, journal_path: str | Path | None = None
) -> str | None:
    inspection = inspect_brain_state(datetime.now(timezone.utc), journal_path=journal_path)
    fingerprint = inspection.get("active_fingerprint")
    if not isinstance(fingerprint, Mapping) or not fingerprint.get("ok"):
        return None
    value = fingerprint.get("fingerprint_sha256")
    return value if isinstance(value, str) else None


def build_active_brain_fingerprint(
    config: Mapping[str, Any],
    *,
    hmac_key: bytes,
    bundled_runtime_fingerprint_sha256: str | None = None,
) -> BrainFingerprintResult:
    response = _run_native(
        ["fingerprint"],
        request={
            "config": dict(config),
            "hmac_key_hex": hmac_key.hex(),
            "bundled_runtime_fingerprint_sha256": _bundled_runtime_fingerprint(
                bundled_runtime_fingerprint_sha256
            ),
        },
    )
    return cast(BrainFingerprintResult, response)


def _active_config(config: Mapping[str, Any]) -> tuple[str, str]:
    providers = config.get("providers")
    active: Any = providers.get("active", {}) if isinstance(providers, Mapping) else {}
    if not isinstance(active, Mapping):
        return "none", ""
    provider = active.get("provider")
    if not isinstance(provider, str) or not provider.strip():
        return "none", ""
    provider = provider.strip()
    if provider == "none":
        return "none", ""
    model = active.get("model")
    return provider, model.strip() if isinstance(model, str) and model.strip() else DEFAULT_MODEL_BY_PROVIDER.get(provider, "")


def _local_config(config: Mapping[str, Any]) -> Mapping[str, Any]:
    providers = config.get("providers")
    if not isinstance(providers, Mapping):
        return {}
    local = providers.get("local", {})
    return local if isinstance(local, Mapping) else {}


def _local_endpoint_from_config(
    config: Mapping[str, Any],
) -> tuple[Literal["missing", "partial", "complete"], str, str, str | None]:
    local = _local_config(config)
    endpoint_url = str(local.get("endpoint_url") or "").strip()
    served_model_id = str(local.get("served_model_id") or "").strip()
    if endpoint_url and served_model_id:
        credential = local.get("credential")
        return (
            "complete",
            normalize_local_endpoint_url(endpoint_url),
            served_model_id,
            str(credential) if credential is not None else None,
        )
    if endpoint_url or served_model_id:
        return "partial", "", "", None
    return "missing", "", "", None


def _spp_provenance_matches(config: Mapping[str, Any]) -> bool:
    endpoint_state, base_url, served_model_id, credential = _local_endpoint_from_config(
        config
    )
    if endpoint_state != "complete" or credential is None:
        return False
    block = confidential_provenance_block(dict(config))
    if not isinstance(block, Mapping):
        return False
    block_url = block.get("endpoint_url")
    block_model = block.get("served_model_id")
    block_fingerprint = block.get("credential_fingerprint_sha256")
    if not isinstance(block_url, str):
        return False
    if not isinstance(block_model, str) or block_model != served_model_id:
        return False
    if not isinstance(block_fingerprint, str):
        return False
    credential_fingerprint = hashlib.sha256(credential.encode("utf-8")).hexdigest()
    return (
        normalize_local_endpoint_url(block_url) == base_url
        and block_fingerprint == credential_fingerprint
    )


def derive_active_brain_lane(config: Mapping[str, Any]) -> BrainLaneId | None:
    provider, _model = _active_config(config)
    if provider == "none":
        return "none"
    if provider in CLOUD_BYO_PROVIDERS:
        return "byo-cloud"
    if provider != "local":
        return None
    endpoint_state, _, _, _ = _local_endpoint_from_config(config)
    if endpoint_state == "missing":
        return "bundled"
    if endpoint_state == "partial":
        return None
    if _spp_provenance_matches(config):
        return "spp"
    return None if confidential_provenance_block(dict(config)) is not None else "byo-endpoint"


def runtime_phase_reason(phase: RuntimePhase) -> BrainReasonCode | None:
    return cast(BrainReasonCode | None, RUNTIME_PHASE_TO_REASON[phase])


__all__ = [
    "BRAIN_AGGREGATE_STATES",
    "BRAIN_CHECKING_FIELDS",
    "BRAIN_COMPONENT_STATUSES",
    "BRAIN_EVIDENCE_COMPONENT_FIELDS",
    "BRAIN_EVIDENCE_FIELDS",
    "BRAIN_EVIDENCE_REASON_CODES",
    "BRAIN_FILE_MODE",
    "BRAIN_LANES",
    "BRAIN_PROJECTION_ONLY_REASON_CODES",
    "BRAIN_REASON_CODES",
    "BRAIN_REASON_TO_AGGREGATE",
    "BRAIN_RUNTIME_FAILURE_MARKER_FIELDS",
    "BRAIN_TOP_LEVEL_FIELDS",
    "BrainAggregateState",
    "BrainComponentStatus",
    "BrainDiagnosticValue",
    "BrainEvidenceComponent",
    "BrainEvidenceRecord",
    "BrainFingerprintResult",
    "BrainInspectionStatus",
    "BrainLaneId",
    "BrainPrerequisiteRenewalBeginResult",
    "BrainPrerequisiteRenewalStatus",
    "BrainProbeOutcome",
    "BrainProjection",
    "BrainReasonCode",
    "BrainRefreshPermit",
    "BrainRuntimeFailureComponent",
    "BrainRuntimeFailureMarker",
    "BrainRuntimeFailureRejectedReason",
    "BrainRuntimeFailureResult",
    "BrainStateConflictError",
    "BrainStateExpectedFingerprintStaleError",
    "BrainStateInspection",
    "BrainStateRecord",
    "BrainStateValidationError",
    "CHECKING_TTL",
    "CLOUD_BYO_PROVIDERS",
    "COMPONENT_ORDER",
    "CONFIG_DIAGNOSTIC_FIELDS",
    "DEFAULT_READY_EVIDENCE_TTL",
    "DIAGNOSTIC_METADATA_SCHEMAS",
    "FINGERPRINT_KEY_BYTES",
    "FINGERPRINT_SCHEMA_VERSION",
    "INCOHERENT_RUNTIME_PHASE_REASON_CODES",
    "LANE_COMPONENTS",
    "PROVIDER_ENV_BY_NAME",
    "RUNTIME_FAILURE_AGGREGATES",
    "RUNTIME_PHASE_TO_REASON",
    "RUNTIME_REASON_CODES",
    "RUNTIME_REASON_TO_BRAIN_REASON",
    "RUNTIME_TRANSITION_PHASES",
    "SCHEMA_VERSION",
    "abandon_brain_prerequisite_renewal",
    "abandon_brain_refresh",
    "begin_brain_prerequisite_renewal",
    "begin_brain_refresh",
    "brain_state_path",
    "build_active_brain_fingerprint",
    "derive_active_brain_lane",
    "finish_brain_prerequisite_renewal",
    "finish_brain_refresh",
    "inspect_brain_state",
    "probe_brain_refresh_lease_held",
    "read_active_brain_fingerprint_sha256",
    "record_brain_runtime_failure",
    "runtime_phase_reason",
]
