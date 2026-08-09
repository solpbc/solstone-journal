# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Native transport for speaker-identity durable-state operations.

The Rust speaker-resolve commands own the durable writes. This module only
constructs their strict JSON requests and transports responses; it never reads
or writes journal artifacts itself.
"""

from __future__ import annotations

import json
import logging
import subprocess
import sys
from collections.abc import Callable, Mapping
from pathlib import Path
from typing import Any

from solstone.think import core_handshake
from solstone.think.indexer.native import runtime_has_solstone_core_wheel_coverage

logger = logging.getLogger(__name__)

EXIT_USAGE = 64
EXIT_UNAVAILABLE = 69
EXIT_TEMPFAIL = 75

UNSUPPORTED_HOST_MESSAGE = (
    "speaker identity requires a compatible solstone-core wheel. Supported wheel "
    "platforms: Linux x86_64 glibc 2.17+ (manylinux2014); Linux aarch64 glibc "
    "2.17+ (manylinux2014); macOS 14.0+ arm64. This host has no compatible "
    "solstone-core wheel. Use a supported host or install a compatible "
    "solstone-core wheel, then retry speaker identity."
)
HANDSHAKE_SKIP_MESSAGE = (
    "speaker identity requires solstone-core, but solstone-core distribution "
    "metadata is missing in this source checkout. Run make install to restore "
    "the journal-host environment, then retry speaker identity."
)
HANDSHAKE_FAIL_MESSAGE = "speaker identity cannot run native operations: {message}"
NATIVE_USAGE_MESSAGE = (
    "speaker identity usage error: native solstone-core speaker-resolve rejected "
    "the request with exit 64. This is a request-construction bug; update "
    "solstone or report the full speaker identity operation."
)
NATIVE_UNAVAILABLE_MESSAGE = (
    "speaker identity native operation exited 69 (unsupported input). Change the "
    "unsupported input and retry speaker identity."
)
NATIVE_TEMPFAIL_MESSAGE = (
    "speaker identity native operation exited 75 (temporary failure). Fix the "
    "reported cause and retry speaker identity."
)
NATIVE_LAUNCH_FAILED_MESSAGE = (
    "speaker identity failed to launch solstone-core speaker-resolve: {error}. "
    "Run make install in a source checkout or reinstall solstone-journal, then retry."
)
NATIVE_SIGNAL_MESSAGE = (
    "speaker identity native operation died from signal {signal_number} "
    "(returncode {returncode}); treating as temporary failure. Fix the cause and retry."
)
NATIVE_OTHER_NONZERO_MESSAGE = (
    "speaker identity native operation exited {returncode}. Fix the reported cause "
    "and retry speaker identity."
)
COMPOSED_COMMAND_WARNING = (
    "speaker identity warning: this operation combined multiple write operations "
    "and the native command did not complete. Some earlier operations may already "
    "have run. Fix the reported cause, then retry the whole speaker identity operation."
)

HandshakeChecker = Callable[[], core_handshake.CoreHandshakeResult]
HelperLocator = Callable[[], Path]
NativeRunner = Callable[..., subprocess.CompletedProcess[str]]


class NativeSpeakerResolveError(RuntimeError):
    """A native speaker-identity operation could not complete."""

    def __init__(
        self,
        message: str,
        *,
        reason: str,
        detail: str | None = None,
        exit_code: int | None = None,
    ) -> None:
        super().__init__(message if detail is None else f"{message}: {detail}")
        self.message = message
        self.reason = reason
        self.detail = detail
        self.exit_code = exit_code


def accumulate_voiceprints(
    journal_root: str | Path,
    *,
    day: str,
    stream: str,
    segment_key: str,
    source: str,
    now_ms: int,
    encoder: Mapping[str, Any],
    labels: list[dict[str, Any]],
    embeddings: list[dict[str, Any]],
    entity_ids: list[str],
    **transport_kwargs: Any,
) -> dict[str, Any]:
    """Accumulate already-screened voiceprints through the native owner."""
    return _run_speaker_resolve(
        "accumulate-voiceprints",
        {
            "schema": "solstone-speaker-resolve-accumulate-voiceprints-request-v1",
            "journal_root": str(journal_root),
            "segment": {
                "day": day,
                "stream": stream,
                "segment_key": segment_key,
                "source": source,
            },
            "now_ms": now_ms,
            "encoder": dict(encoder),
            "labels": labels,
            "embeddings": embeddings,
            "entity_ids": entity_ids,
        },
        **transport_kwargs,
    )


def write_owner_centroid(
    journal_root: str | Path,
    *,
    principal_entity_id: str,
    centroid: list[float],
    cluster_size: int,
    timestamp: str,
    evidence_tier: str,
    **transport_kwargs: Any,
) -> dict[str, Any]:
    """Persist a policy-approved owner centroid."""
    return _run_speaker_resolve(
        "write-owner-centroid",
        {
            "journal_root": str(journal_root),
            "principal_entity_id": principal_entity_id,
            "centroid": centroid,
            "cluster_size": cluster_size,
            "timestamp": timestamp,
            "evidence_tier": evidence_tier,
        },
        **transport_kwargs,
    )


def rebuild_owner_centroid(
    journal_root: str | Path,
    *,
    principal_entity_id: str,
    centroid: list[float],
    embeddings_count: int,
    timestamp: str,
    evidence_hash: str,
    evidence_intra_cosine_p25: float,
    evidence_tier: str,
    override: bool,
    **transport_kwargs: Any,
) -> dict[str, Any]:
    """Persist an owner centroid that Python has already quality-gated."""
    return _run_speaker_resolve(
        "rebuild-owner-centroid",
        {
            "journal_root": str(journal_root),
            "principal_entity_id": principal_entity_id,
            "centroid": centroid,
            "cluster_size": embeddings_count,
            "timestamp": timestamp,
            "evidence_hash": evidence_hash,
            "evidence_intra_cosine_p25": evidence_intra_cosine_p25,
            "evidence_tier": evidence_tier,
            "override": override,
        },
        **transport_kwargs,
    )


def write_owner_candidate(
    journal_root: str | Path,
    *,
    centroid: list[float],
    cluster_size: int,
    threshold: float,
    version: str,
    evidence_tier: str,
    **transport_kwargs: Any,
) -> dict[str, Any]:
    """Persist a policy-approved owner candidate."""
    return _run_speaker_resolve(
        "write-owner-candidate",
        {
            "journal_root": str(journal_root),
            "centroid": centroid,
            "cluster_size": cluster_size,
            "threshold": threshold,
            "version": version,
            "evidence_tier": evidence_tier,
        },
        **transport_kwargs,
    )


def clear_owner_candidate(
    journal_root: str | Path,
    **transport_kwargs: Any,
) -> dict[str, Any]:
    """Clear the persisted owner candidate if present."""
    return _run_speaker_resolve(
        "clear-owner-candidate",
        {
            "schema": "solstone-speaker-resolve-clear-owner-candidate-request-v1",
            "journal_root": str(journal_root),
        },
        **transport_kwargs,
    )


def identify(
    journal_root: str | Path,
    *,
    cluster_id: int,
    name: str | None,
    entity_id: str | None,
    resolve_only: bool,
    create_new: bool,
    entity_type: str,
    request_id: str,
    reviewed_near_match_entity_ids: list[str] | None,
    caller: str | None,
    actor: str | None,
    encoder: Mapping[str, Any],
    **transport_kwargs: Any,
) -> dict[str, Any]:
    """Run a native identify operation."""
    return _run_speaker_resolve(
        "identify",
        {
            "schema": "solstone-speaker-resolve-identify-request-v1",
            "journal_root": str(journal_root),
            "cluster_id": cluster_id,
            "name": name,
            "entity_id": entity_id,
            "resolve_only": resolve_only,
            "create_new": create_new,
            "entity_type": entity_type,
            "request_id": request_id,
            "reviewed_near_match_entity_ids": reviewed_near_match_entity_ids,
            "caller": caller,
            "actor": actor,
            "encoder": dict(encoder),
        },
        **transport_kwargs,
    )


def undo_identify(
    journal_root: str | Path,
    *,
    operation_id: str,
    encoder: Mapping[str, Any],
    **transport_kwargs: Any,
) -> dict[str, Any]:
    """Undo one native identify operation."""
    return _run_speaker_resolve(
        "undo-identify",
        {
            "schema": "solstone-speaker-resolve-undo-identify-request-v1",
            "journal_root": str(journal_root),
            "operation_id": operation_id,
            "encoder": dict(encoder),
        },
        **transport_kwargs,
    )


def bootstrap_voiceprints(
    journal_root: str | Path,
    *,
    encoder: Mapping[str, Any],
    added_at: int,
    dry_run: bool,
    **transport_kwargs: Any,
) -> dict[str, Any]:
    """Run native voiceprint bootstrap."""
    return _run_bootstrap(
        "bootstrap-voiceprints",
        "solstone-speaker-resolve-bootstrap-voiceprints-request-v1",
        journal_root,
        encoder=encoder,
        added_at=added_at,
        dry_run=dry_run,
        **transport_kwargs,
    )


def seed_from_imports(
    journal_root: str | Path,
    *,
    encoder: Mapping[str, Any],
    added_at: int,
    dry_run: bool,
    **transport_kwargs: Any,
) -> dict[str, Any]:
    """Run native import voiceprint seeding."""
    return _run_bootstrap(
        "seed-from-imports",
        "solstone-speaker-resolve-seed-from-imports-request-v1",
        journal_root,
        encoder=encoder,
        added_at=added_at,
        dry_run=dry_run,
        **transport_kwargs,
    )


def write_stub_labels(
    journal_root: str | Path,
    seg_dir: Path,
    reason: str,
    **transport_kwargs: Any,
) -> dict[str, Any]:
    """Write a skipped speaker-label payload for one segment."""
    return _run_segment_operation(
        "write-stub-labels",
        "solstone-speaker-resolve-write-stub-labels-request-v1",
        journal_root,
        seg_dir,
        {"reason": reason},
        **transport_kwargs,
    )


def write_full_labels(
    journal_root: str | Path,
    seg_dir: Path,
    labels: list[dict[str, Any]],
    metadata: dict[str, Any],
    **transport_kwargs: Any,
) -> dict[str, Any]:
    """Write a full speaker-label payload for one segment."""
    return _run_segment_operation(
        "write-full-labels",
        "solstone-speaker-resolve-write-full-labels-request-v1",
        journal_root,
        seg_dir,
        {"labels": labels, "metadata": metadata},
        **transport_kwargs,
    )


def patch_labels(
    journal_root: str | Path,
    seg_dir: Path,
    patches: Mapping[int, dict[str, Any]],
    *,
    allow_insert: bool,
    **transport_kwargs: Any,
) -> dict[str, Any]:
    """Apply native per-sentence speaker-label patches."""
    return _run_segment_operation(
        "patch-labels",
        "solstone-speaker-resolve-patch-labels-request-v1",
        journal_root,
        seg_dir,
        {
            "patches": {str(key): value for key, value in patches.items()},
            "allow_insert": allow_insert,
        },
        **transport_kwargs,
    )


def restore_label_rows(
    journal_root: str | Path,
    seg_dir: Path,
    restorations: list[dict[str, Any]],
    **transport_kwargs: Any,
) -> dict[str, Any]:
    """Compare-restore native speaker-label rows."""
    return _run_segment_operation(
        "restore-label-rows",
        "solstone-speaker-resolve-restore-label-rows-request-v1",
        journal_root,
        seg_dir,
        {"restorations": restorations},
        **transport_kwargs,
    )


def append_correction(
    journal_root: str | Path,
    seg_dir: Path,
    correction: dict[str, Any],
    **transport_kwargs: Any,
) -> dict[str, Any]:
    """Append one native speaker correction."""
    return _run_segment_operation(
        "append-correction",
        "solstone-speaker-resolve-append-correction-request-v1",
        journal_root,
        seg_dir,
        {"correction": correction},
        **transport_kwargs,
    )


def write_voiceprint(
    journal_root: str | Path,
    *,
    entity_id: str,
    embedding: list[float],
    metadata: dict[str, Any],
    encoder: Mapping[str, Any],
    **transport_kwargs: Any,
) -> dict[str, Any]:
    """Write one authenticated voiceprint without accumulation policy."""
    return _run_speaker_resolve(
        "write-voiceprint",
        {
            "schema": "solstone-speaker-resolve-write-voiceprint-request-v1",
            "journal_root": str(journal_root),
            "entity_id": entity_id,
            "embedding": embedding,
            "metadata": metadata,
            "encoder": dict(encoder),
        },
        **transport_kwargs,
    )


def remove_voiceprint(
    journal_root: str | Path,
    *,
    entity_id: str,
    key: dict[str, Any],
    encoder: Mapping[str, Any],
    **transport_kwargs: Any,
) -> dict[str, Any]:
    """Remove one direct voiceprint by its metadata key."""
    return _run_speaker_resolve(
        "remove-voiceprint",
        {
            "schema": "solstone-speaker-resolve-remove-voiceprint-request-v1",
            "journal_root": str(journal_root),
            "entity_id": entity_id,
            "key": key,
            "encoder": dict(encoder),
        },
        **transport_kwargs,
    )


def backfill_voiceprint_last_seen(
    journal_root: str | Path,
    *,
    entity_id: str,
    last_seen_ts: int,
    encoder: Mapping[str, Any],
    **transport_kwargs: Any,
) -> dict[str, Any]:
    """Raise every voiceprint row's last-seen timestamp through Rust."""
    return _run_speaker_resolve(
        "backfill-voiceprint-last-seen",
        {
            "schema": "solstone-speaker-resolve-backfill-voiceprint-last-seen-request-v1",
            "journal_root": str(journal_root),
            "entity_id": entity_id,
            "last_seen_ts": last_seen_ts,
            "encoder": dict(encoder),
        },
        **transport_kwargs,
    )


def wipe_speaker_artifacts(
    journal_root: str | Path,
    *,
    dry_run: bool,
    **transport_kwargs: Any,
) -> dict[str, Any]:
    """Remove durable speaker artifacts through the native owner."""
    return _run_speaker_resolve(
        "wipe-speaker-artifacts",
        {
            "schema": "solstone-speaker-resolve-wipe-speaker-artifacts-request-v1",
            "journal_root": str(journal_root),
            "dry_run": dry_run,
        },
        **transport_kwargs,
    )


def _run_bootstrap(
    verb: str,
    schema: str,
    journal_root: str | Path,
    *,
    encoder: Mapping[str, Any],
    added_at: int,
    dry_run: bool,
    **transport_kwargs: Any,
) -> dict[str, Any]:
    return _run_speaker_resolve(
        verb,
        {
            "schema": schema,
            "journal_root": str(journal_root),
            "encoder": dict(encoder),
            "added_at": added_at,
            "dry_run": dry_run,
        },
        **transport_kwargs,
    )


def _run_segment_operation(
    verb: str,
    schema: str,
    journal_root: str | Path,
    seg_dir: Path,
    fields: dict[str, Any],
    **transport_kwargs: Any,
) -> dict[str, Any]:
    request = {
        "schema": schema,
        "journal_root": str(journal_root),
        "segment": _segment_request(journal_root, seg_dir),
        **fields,
    }
    return _run_speaker_resolve(verb, request, **transport_kwargs)


def _segment_request(journal_root: str | Path, seg_dir: Path) -> dict[str, str]:
    segment_dir = Path(seg_dir)
    if segment_dir.name == "talents":
        segment_dir = segment_dir.parent
    chronicle = Path(journal_root) / "chronicle"
    try:
        day, stream, segment_key = segment_dir.relative_to(chronicle).parts
    except ValueError as exc:
        raise ValueError(
            f"segment path is outside journal chronicle: {seg_dir}"
        ) from exc
    return {"day": day, "stream": stream, "segment_key": segment_key}


def _run_speaker_resolve(
    verb: str,
    request: dict[str, Any],
    *,
    operation_count: int = 1,
    handshake_checker: HandshakeChecker = core_handshake.check_solstone_core_handshake,
    helper_locator: HelperLocator = core_handshake.helper_path_for_executable,
    native_runner: NativeRunner = subprocess.run,
    platform_covered: Callable[[], bool] = runtime_has_solstone_core_wheel_coverage,
) -> dict[str, Any]:
    if not platform_covered():
        raise _error(
            "unsupported-host", UNSUPPORTED_HOST_MESSAGE, core_handshake.EX_CONFIG
        )

    handshake = handshake_checker()
    if handshake.status == "skip":
        raise _error("handshake-skip", HANDSHAKE_SKIP_MESSAGE, core_handshake.EX_CONFIG)
    if handshake.status == "fail":
        raise _error(
            "handshake-fail",
            HANDSHAKE_FAIL_MESSAGE.format(
                message=handshake.message or "unknown reason"
            ),
            core_handshake.EX_CONFIG,
        )

    argv = [str(helper_locator()), "speaker-resolve", verb]
    try:
        completed = native_runner(
            argv,
            input=json.dumps(request, allow_nan=False),
            capture_output=True,
            text=True,
            check=False,
        )
    except OSError as exc:
        raise _error(
            "launch-failed",
            NATIVE_LAUNCH_FAILED_MESSAGE.format(error=exc),
            EXIT_TEMPFAIL,
        ) from exc

    if completed.returncode != 0:
        raise _native_failure(completed, operation_count)
    try:
        response = json.loads(completed.stdout)
    except json.JSONDecodeError as exc:
        raise _error(
            "invalid-response",
            "speaker identity native operation returned invalid JSON",
            EXIT_TEMPFAIL,
        ) from exc
    if not isinstance(response, dict):
        raise _error(
            "invalid-response",
            "speaker identity native operation returned a non-object JSON response",
            EXIT_TEMPFAIL,
        )
    return response


def _native_failure(
    completed: subprocess.CompletedProcess[str],
    operation_count: int,
) -> NativeSpeakerResolveError:
    returncode = completed.returncode
    if returncode < 0:
        _emit_composed_warning(operation_count)
        return _error(
            "signal",
            NATIVE_SIGNAL_MESSAGE.format(
                signal_number=abs(returncode),
                returncode=returncode,
            ),
            EXIT_TEMPFAIL,
            _error_detail(completed.stderr),
        )
    if returncode == EXIT_USAGE:
        return _error(
            "usage", NATIVE_USAGE_MESSAGE, returncode, _error_detail(completed.stderr)
        )
    if returncode == EXIT_UNAVAILABLE:
        _emit_composed_warning(operation_count)
        return _error(
            "unavailable",
            NATIVE_UNAVAILABLE_MESSAGE,
            returncode,
            _error_detail(completed.stderr),
        )
    if returncode == EXIT_TEMPFAIL:
        _emit_composed_warning(operation_count)
        return _error(
            "tempfail",
            NATIVE_TEMPFAIL_MESSAGE,
            returncode,
            _error_detail(completed.stderr),
        )
    _emit_composed_warning(operation_count)
    return _error(
        "native-nonzero",
        NATIVE_OTHER_NONZERO_MESSAGE.format(returncode=returncode),
        returncode,
        _error_detail(completed.stderr),
    )


def _error_detail(stderr: str) -> str | None:
    for line in reversed(stderr.splitlines()):
        try:
            response = json.loads(line)
        except json.JSONDecodeError:
            continue
        if isinstance(response, dict) and isinstance(response.get("detail"), str):
            return response["detail"]
    return stderr.strip() or None


def _error(
    reason: str,
    message: str,
    exit_code: int,
    detail: str | None = None,
) -> NativeSpeakerResolveError:
    return NativeSpeakerResolveError(
        message,
        reason=reason,
        detail=detail,
        exit_code=exit_code,
    )


def _emit_composed_warning(operation_count: int) -> None:
    if operation_count > 1:
        print(COMPOSED_COMMAND_WARNING, file=sys.stderr)
        logger.warning(COMPOSED_COMMAND_WARNING)
