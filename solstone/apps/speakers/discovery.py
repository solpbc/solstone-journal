# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Unknown speaker discovery - cluster unmatched embeddings to find recurring voices."""

from __future__ import annotations

import json
import logging
import os
import re
import shutil
import tempfile
import uuid
from collections import defaultdict
from collections.abc import Callable
from dataclasses import dataclass
from datetime import datetime
from pathlib import Path
from typing import TYPE_CHECKING, Any

from solstone.apps.speakers import native as native_speakers
from solstone.apps.speakers.attribution import (
    _load_setting_field,
    _speaker_encoder_identity,
    compute_segment_candidate_evidence_readonly,
)
from solstone.apps.speakers.audio import resolve_audio_url
from solstone.apps.speakers.issues import (
    invalid_embeddings_issue,
    owner_voice_unavailable_issue,
)
from solstone.think.entities.journal import get_journal_principal, load_journal_entity
from solstone.think.speakers_analyze_installation import (
    speakers_analyze_path_for_executable,
)
from solstone.think.utils import day_dirs, get_journal, segment_path

if TYPE_CHECKING:
    from solstone.observe.transcribe.speakers_analyze_adapter import (
        HelperInvocationResult,
    )
else:
    HelperInvocationResult = Any

logger = logging.getLogger(__name__)

MIN_CLUSTER_SIZE = 5
MIN_SAMPLES = 3
MIN_SEGMENT_DIVERSITY = 3
MAX_UNMATCHED_EMBEDDINGS = 10000
UNIT_NORM_TOLERANCE = 1.0e-3

DISCOVERY_CLUSTER_REQUEST_SCHEMA = "solstone-speaker-discovery-cluster-request-v1"
DISCOVERY_CLUSTER_RESPONSE_SCHEMA = "solstone-speaker-discovery-cluster-response-v1"
DISCOVERY_CLUSTER_COMMAND = "discovery-cluster"
DISCOVERY_CLUSTER_PAYLOAD_FORMAT = "raw-f32le-row-major-v1"
DISCOVERY_CLUSTER_DTYPE = "float32-le"
DISCOVERY_CLUSTER_ALGORITHM = "hdbscan-eom-euclidean-f64-prim-mst"
DISCOVERY_TEMP_PREFIX = "solstone-speakers-analyze-discovery-cluster-"
TEMP_ROOT = Path("/var/tmp")
TEMP_DIR_MODE = 0o700
TEMP_FILE_MODE = 0o600

DiscoveryHelperLocator = Callable[[], Path]
DiscoveryHelperInvoker = Callable[[list[str], str], HelperInvocationResult]
DiscoveryTempDirFactory = Callable[[], Path]
_REASON_RE = re.compile(r"^[a-z0-9][a-z0-9-]*$")


class SpeakerDiscoveryKernelError(RuntimeError):
    """Content-free attribution for failed native discovery clustering."""

    def __init__(
        self,
        *,
        stage: str,
        reason: str,
        native_exit_code: int | None = None,
    ) -> None:
        safe_reason = (
            reason if _REASON_RE.fullmatch(reason) else "invalid-helper-reason"
        )
        super().__init__(f"speaker discovery kernel failed: {stage}/{safe_reason}")
        self.stage = stage
        self.reason = safe_reason
        self.native_exit_code = native_exit_code

    def event_fields(self) -> dict[str, object]:
        fields: dict[str, object] = {
            "speaker_discovery_kernel_failure_stage": self.stage,
            "speaker_discovery_kernel_failure_reason": self.reason,
        }
        if self.native_exit_code is not None:
            fields["speaker_discovery_kernel_failure_native_exit_code"] = (
                self.native_exit_code
            )
        return fields


_KERNEL_FAILURE_HTTP_RESULTS: dict[str, tuple[int, bool]] = {
    "invoke": (503, True),
    "response": (500, False),
    "parse": (500, False),
    "request": (500, False),
    "payload": (500, False),
}
_KERNEL_FAILURE_HTTP_DEFAULT = (500, False)


def discovery_kernel_failure_http_result(stage: str) -> tuple[int, bool]:
    """Return the public HTTP status and retryability for a kernel failure stage."""
    return _KERNEL_FAILURE_HTTP_RESULTS.get(stage, _KERNEL_FAILURE_HTTP_DEFAULT)


def create_discovery_cluster_temp_dir() -> Path:
    """Create a native discovery-cluster temp directory swept by the adapter."""
    prefix = f"{DISCOVERY_TEMP_PREFIX}{os.getpid()}-"
    path = Path(tempfile.mkdtemp(prefix=prefix, dir=TEMP_ROOT))
    path.chmod(TEMP_DIR_MODE)
    return path


def _invoke_discovery_helper(
    argv: list[str],
    stdin_text: str,
) -> HelperInvocationResult:
    from solstone.observe.transcribe.speakers_analyze_adapter import (
        SpeakersAnalyzeBudget,
        invoke_speakers_analyze_helper,
    )

    return invoke_speakers_analyze_helper(
        argv,
        stdin_text,
        Path("speaker-discovery-cluster"),
        budget=SpeakersAnalyzeBudget(
            timeout_s=180.0,
            stdout_limit_bytes=1024 * 1024,
            stderr_limit_bytes=64 * 1024,
            terminate_grace_s=5.0,
            kill_grace_s=5.0,
        ),
    )


def _write_embeddings_f32le(path: Path, embeddings_matrix: Any) -> None:
    import numpy as np

    payload = np.ascontiguousarray(embeddings_matrix, dtype="<f4")
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, TEMP_FILE_MODE)
    try:
        with os.fdopen(fd, "wb") as f:
            f.write(payload.tobytes(order="C"))
            f.flush()
            os.fsync(f.fileno())
    except Exception:
        path.unlink(missing_ok=True)
        raise


def _raise_for_discovery_returncode(completed: HelperInvocationResult) -> None:
    if completed.returncode == 0:
        return
    if completed.returncode < 0:
        reason = f"signal-{abs(completed.returncode)}"
    else:
        reason = f"exit-{completed.returncode}"
    raise SpeakerDiscoveryKernelError(
        stage="invoke",
        reason=reason,
        native_exit_code=completed.returncode,
    )


def _discovery_cluster_request(payload_path: Path, rows: int, cols: int) -> dict:
    return {
        "schema": DISCOVERY_CLUSTER_REQUEST_SCHEMA,
        "embeddings_f32le_path": str(payload_path),
        "payload_format": DISCOVERY_CLUSTER_PAYLOAD_FORMAT,
        "dtype": DISCOVERY_CLUSTER_DTYPE,
        "shape": [rows, cols],
        "min_cluster_size": MIN_CLUSTER_SIZE,
        "min_samples": MIN_SAMPLES,
    }


def _labels_from_discovery_response(stdout: str, *, rows: int) -> Any:
    import numpy as np

    try:
        response = json.loads(stdout)
    except json.JSONDecodeError:
        raise SpeakerDiscoveryKernelError(
            stage="response",
            reason="response-json-invalid",
        ) from None

    if not isinstance(response, dict) or (
        response.get("schema") != DISCOVERY_CLUSTER_RESPONSE_SCHEMA
    ):
        raise SpeakerDiscoveryKernelError(stage="response", reason="schema-mismatch")

    labels = response.get("labels")
    if not isinstance(labels, list) or any(type(label) is not int for label in labels):
        raise SpeakerDiscoveryKernelError(stage="response", reason="labels-invalid")

    if len(labels) != rows:
        raise SpeakerDiscoveryKernelError(
            stage="response",
            reason="label-count-mismatch",
        )

    parameters = response.get("parameters")
    if (
        not isinstance(parameters, dict)
        or parameters.get("min_cluster_size") != MIN_CLUSTER_SIZE
        or parameters.get("min_samples") != MIN_SAMPLES
    ):
        raise SpeakerDiscoveryKernelError(
            stage="response",
            reason="parameters-mismatch",
        )

    if response.get("algorithm") != DISCOVERY_CLUSTER_ALGORITHM:
        raise SpeakerDiscoveryKernelError(
            stage="response",
            reason="algorithm-mismatch",
        )

    noise_count = sum(1 for label in labels if int(label) == -1)
    cluster_count = len({int(label) for label in labels if int(label) != -1})
    if (
        type(response.get("noise_count")) is not int
        or type(response.get("cluster_count")) is not int
        or int(response["noise_count"]) != noise_count
        or int(response["cluster_count"]) != cluster_count
    ):
        raise SpeakerDiscoveryKernelError(stage="response", reason="count-mismatch")

    return np.asarray(labels, dtype=np.int64)


def _cluster_discovery_embeddings_native(
    embeddings_matrix: Any,
    *,
    helper_locator: DiscoveryHelperLocator,
    helper_invoker: DiscoveryHelperInvoker,
    temp_dir_factory: DiscoveryTempDirFactory,
) -> Any:
    rows = int(embeddings_matrix.shape[0])
    cols = int(embeddings_matrix.shape[1])
    temp_dir: Path | None = None
    try:
        temp_dir = temp_dir_factory()
        payload_path = temp_dir / "embeddings.f32le"
        _write_embeddings_f32le(payload_path, embeddings_matrix)
        request = _discovery_cluster_request(payload_path, rows, cols)
        completed = helper_invoker(
            [str(helper_locator()), DISCOVERY_CLUSTER_COMMAND],
            json.dumps(request),
        )
        _raise_for_discovery_returncode(completed)
        return _labels_from_discovery_response(completed.stdout, rows=rows)
    except Exception as exc:
        from solstone.observe.transcribe.speakers_analyze_errors import (
            SpeakerAnalyzeError,
        )

        if not isinstance(exc, SpeakerAnalyzeError):
            raise
        raise SpeakerDiscoveryKernelError(
            stage=exc.stage,
            reason=exc.reason,
            native_exit_code=exc.native_exit_code,
        ) from exc
    finally:
        if temp_dir is not None:
            shutil.rmtree(temp_dir, ignore_errors=True)


def _routes_helpers():
    """Load speakers route helpers lazily to avoid import cycles."""
    from solstone.apps.speakers.routes import (
        _check_owner_contamination,
        _load_embeddings_file,
        _load_speaker_labels,
        _normalize_embedding,
        _scan_segment_embeddings,
    )

    return (
        _load_embeddings_file,
        _load_speaker_labels,
        _normalize_embedding,
        _scan_segment_embeddings,
        _check_owner_contamination,
    )


def _owner_helpers():
    """Load owner helpers lazily to avoid import cycles."""
    from solstone.apps.speakers.owner import load_owner_centroid

    return load_owner_centroid


def _discovery_cache_path(*, create: bool = False) -> Path:
    """Return the temporary cache path for discovery cluster assignments."""
    awareness_dir = Path(get_journal()) / "awareness"
    if create:
        awareness_dir.mkdir(parents=True, exist_ok=True)
    return awareness_dir / "discovery_clusters.json"


def _discovery_resolved_path(*, create: bool = False) -> Path:
    """Return the idempotency sentinel path for resolved discovery clusters."""
    return _discovery_cache_path(create=create).with_suffix(".resolved.json")


def load_discovery_cache() -> dict[str, Any] | None:
    """Return cached discovery cluster assignments, if present and valid."""
    path = _discovery_cache_path()
    if not path.exists():
        return None
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except (json.JSONDecodeError, OSError):
        return None
    if not isinstance(data, dict):
        return None
    clusters = data.get("clusters")
    return data if isinstance(clusters, dict) else None


def _get_sentence_text(segment_dir: Path, source: str, sentence_id: int) -> str | None:
    """Return transcript text for a sentence ID from the source transcript."""
    jsonl_path = segment_dir / f"{source}.jsonl"
    if not jsonl_path.exists():
        return None
    try:
        lines = jsonl_path.read_text(encoding="utf-8").splitlines()
        if sentence_id < 1 or sentence_id >= len(lines):
            return None
        entry = json.loads(lines[sentence_id])
        return entry.get("text")
    except (json.JSONDecodeError, OSError, IndexError):
        return None


def _build_cluster_sample(record: dict) -> dict:
    day = record["day"]
    stream = record["stream"]
    segment_key = record["segment_key"]
    source = record["source"]
    sentence_id = record["sentence_id"]
    seg_dir = segment_path(day, segment_key, stream, create=False)
    return {
        **record,
        "audio_url": resolve_audio_url(day, stream, segment_key, source),
        "text": _get_sentence_text(seg_dir, source, sentence_id) or "",
    }


def _serialize_discovery_cluster(
    cluster_id: int,
    members: list[dict[str, Any]],
) -> dict[str, Any] | None:
    normalized_members = [
        normalized
        for member in members
        if (normalized := _normalized_cache_member(member)) is not None
    ]
    if not normalized_members:
        return None
    segment_keys = {
        (
            member["day"],
            member["stream"],
            member["segment_key"],
        )
        for member in normalized_members
    }
    samples: list[dict[str, Any]] = []
    seen_segments: set[tuple[str, str, str]] = set()
    for member in normalized_members:
        segment = (
            member["day"],
            member["stream"],
            member["segment_key"],
        )
        if segment in seen_segments:
            continue
        seen_segments.add(segment)
        samples.append(_build_cluster_sample(member))
        if len(samples) == 3:
            break
    if len(samples) < 3:
        for member in normalized_members:
            sample = _build_cluster_sample(member)
            if sample in samples:
                continue
            samples.append(sample)
            if len(samples) == 3:
                break
    return {
        "cluster_id": int(cluster_id),
        "size": len(normalized_members),
        "segment_count": len(segment_keys),
        "samples": samples,
    }


def _serialize_discovery_clusters(
    clusters: dict[str, Any],
) -> dict[str, Any]:
    from solstone.think.speaker_cluster_dismissals import (
        cluster_dismissal_suppressed,
    )

    rows: list[dict[str, Any]] = []
    for raw_cluster_id, members in clusters.items():
        try:
            cluster_id = int(raw_cluster_id)
        except (TypeError, ValueError):
            continue
        if not isinstance(members, list):
            continue
        normalized_members = [
            normalized
            for member in members
            if (normalized := _normalized_cache_member(member)) is not None
        ]
        if not normalized_members:
            continue
        if cluster_dismissal_suppressed(normalized_members):
            continue
        row = _serialize_discovery_cluster(cluster_id, normalized_members)
        if row is not None:
            rows.append(row)
    rows.sort(key=lambda cluster: (-int(cluster["size"]), int(cluster["cluster_id"])))
    return {"clusters": rows}


def read_discovery_cache_snapshot() -> dict[str, Any]:
    """Return visible discovery clusters from the current cache without scanning."""
    cache = load_discovery_cache()
    if cache is None:
        return {"status": "cache_unavailable", "clusters": []}
    return {"status": "ok", **_serialize_discovery_clusters(cache.get("clusters", {}))}


def _clear_discovery_cache() -> None:
    """Remove the cached discovery assignment file if present."""
    _discovery_cache_path().unlink(missing_ok=True)
    _discovery_resolved_path().unlink(missing_ok=True)


def _discovery_scan_result(
    clusters: list[dict[str, Any]],
    issues: list[dict[str, Any]],
) -> dict[str, Any]:
    return {
        "status": "degraded" if issues else "ok",
        "clusters": clusters,
        "issues": issues,
    }


def discover_unknown_speakers(
    *,
    helper_locator: DiscoveryHelperLocator = speakers_analyze_path_for_executable,
    helper_invoker: DiscoveryHelperInvoker = _invoke_discovery_helper,
    temp_dir_factory: DiscoveryTempDirFactory = create_discovery_cluster_temp_dir,
) -> dict[str, Any]:
    """Scan journal for recurring unknown speaker clusters."""
    import numpy as np

    load_owner_centroid = _owner_helpers()
    (
        load_embeddings_file,
        load_speaker_labels,
        normalize_embedding,
        scan_segment_embeddings,
        _,
    ) = _routes_helpers()

    centroid_data = load_owner_centroid()
    if centroid_data is None:
        return _discovery_scan_result([], [owner_voice_unavailable_issue()])

    owner_centroid = centroid_data.centroid
    owner_threshold = centroid_data.threshold
    embedding_chunks: list[np.ndarray] = []
    provenance: list[dict[str, Any]] = []
    issues: list[dict[str, Any]] = []
    dropped_count = 0

    for day in sorted(day_dirs().keys()):
        for segment in scan_segment_embeddings(day):
            stream = segment["stream"]
            seg_key = segment["key"]
            seg_dir = segment_path(day, seg_key, stream, create=False)

            labels_data = load_speaker_labels(seg_dir)
            attributed_sids: set[int] = set()
            if labels_data:
                for label in labels_data.get("labels", []):
                    sentence_id = label.get("sentence_id")
                    if label.get("speaker") is not None and sentence_id is not None:
                        attributed_sids.add(int(sentence_id))

            for source in segment["sources"]:
                emb_data = load_embeddings_file(seg_dir / f"{source}.npz")
                if emb_data is None:
                    continue

                embeddings, statement_ids, _ = emb_data
                if len(embeddings) == 0:
                    continue

                for emb, sid in zip(embeddings, statement_ids):
                    sid_int = int(sid)
                    if sid_int in attributed_sids:
                        continue

                    raw_embedding = np.asarray(emb, dtype=np.float64)
                    with np.errstate(invalid="ignore"):
                        raw_norm = float(np.linalg.norm(raw_embedding))
                    if (
                        not np.isfinite(raw_embedding).all()
                        or not np.isfinite(raw_norm)
                        or abs(raw_norm - 1.0) > UNIT_NORM_TOLERANCE
                    ):
                        dropped_count += 1
                        continue

                    normalized = normalize_embedding(emb)
                    if normalized is None:
                        dropped_count += 1
                        continue

                    score = float(np.dot(normalized, owner_centroid))
                    if score >= owner_threshold:
                        continue

                    embedding_chunks.append(normalized.reshape(1, -1))
                    provenance.append(
                        {
                            "day": day,
                            "stream": stream,
                            "segment_key": seg_key,
                            "source": source,
                            "sentence_id": sid_int,
                        }
                    )

    if dropped_count:
        logger.info(
            "speaker discovery admission filtered: dropped_invalid_embeddings=%d",
            dropped_count,
        )
        issues.append(invalid_embeddings_issue(dropped_count))

    if not embedding_chunks:
        _clear_discovery_cache()
        return _discovery_scan_result([], issues)

    embeddings_matrix = np.vstack(embedding_chunks)

    if len(embeddings_matrix) > MAX_UNMATCHED_EMBEDDINGS:
        rng = np.random.default_rng(42)
        indices = rng.choice(
            len(embeddings_matrix),
            MAX_UNMATCHED_EMBEDDINGS,
            replace=False,
        )
        indices.sort()
        embeddings_matrix = embeddings_matrix[indices]
        provenance = [provenance[int(i)] for i in indices]

    if len(embeddings_matrix) < MIN_CLUSTER_SIZE:
        _clear_discovery_cache()
        return _discovery_scan_result([], issues)

    labels = _cluster_discovery_embeddings_native(
        embeddings_matrix,
        helper_locator=helper_locator,
        helper_invoker=helper_invoker,
        temp_dir_factory=temp_dir_factory,
    )
    if np.all(labels == -1):
        _clear_discovery_cache()
        return _discovery_scan_result([], issues)

    cache_clusters: dict[str, list[dict[str, Any]]] = {}

    for cid in sorted(set(labels[labels != -1])):
        cluster_indices = np.flatnonzero(labels == int(cid))
        segment_set = {
            (
                provenance[int(idx)]["day"],
                provenance[int(idx)]["stream"],
                provenance[int(idx)]["segment_key"],
            )
            for idx in cluster_indices
        }
        if len(segment_set) < MIN_SEGMENT_DIVERSITY:
            continue

        cluster_embeddings = embeddings_matrix[cluster_indices]
        centroid = normalize_embedding(np.mean(cluster_embeddings, axis=0))
        if centroid is None:
            continue
        similarities = np.dot(cluster_embeddings, centroid)
        sorted_positions = np.argsort(similarities)[::-1]
        cache_clusters[str(int(cid))] = [
            provenance[int(cluster_indices[int(pos)])] for pos in sorted_positions
        ]

    if not cache_clusters:
        _clear_discovery_cache()
        return _discovery_scan_result([], issues)

    cache_path = _discovery_cache_path(create=True)
    tmp_path = cache_path.with_suffix(".tmp")
    with open(tmp_path, "w", encoding="utf-8") as f:
        json.dump(
            {
                "version": datetime.now().isoformat(),
                "clusters": cache_clusters,
            },
            f,
            indent=2,
        )
    tmp_path.rename(cache_path)

    return _discovery_scan_result(
        _serialize_discovery_clusters(cache_clusters)["clusters"],
        issues,
    )


def _conversation_key(
    day: str,
    stream: str,
    segment_key: str,
    setting: str | None,
) -> tuple:
    if setting:
        return (day, stream, setting)
    return (day, stream, "__segment__", segment_key)


@dataclass(frozen=True)
class _ClusterConversationContext:
    distinct_segments: tuple[tuple[str, str, str], ...]
    first_record_by_segment: dict[tuple[str, str, str], dict[str, Any]]
    segment_settings: dict[tuple[str, str, str], str | None]
    conversation_keys: dict[tuple[str, str, str], tuple]
    conversation_count: int


def _normalized_cache_member(member: Any) -> dict[str, Any] | None:
    if not isinstance(member, dict):
        return None
    normalized: dict[str, Any] = {}
    for field in ("day", "stream", "segment_key", "source"):
        value = member.get(field)
        if not isinstance(value, str) or not value:
            return None
        normalized[field] = value
    try:
        normalized["sentence_id"] = int(member["sentence_id"])
    except (KeyError, TypeError, ValueError):
        return None
    return normalized


def _cluster_conversation_context(
    members: list[dict[str, Any]],
) -> _ClusterConversationContext:
    distinct_segments: list[tuple[str, str, str]] = []
    first_record_by_segment: dict[tuple[str, str, str], dict[str, Any]] = {}
    for member in members:
        normalized = _normalized_cache_member(member)
        if normalized is None:
            continue
        segment = (
            normalized["day"],
            normalized["stream"],
            normalized["segment_key"],
        )
        if segment in first_record_by_segment:
            continue
        first_record_by_segment[segment] = normalized
        distinct_segments.append(segment)

    segment_settings: dict[tuple[str, str, str], str | None] = {}
    conversation_keys: dict[tuple[str, str, str], tuple] = {}
    for day, stream, segment_key in distinct_segments:
        seg_dir = segment_path(day, segment_key, stream, create=False)
        setting = _load_setting_field(seg_dir)
        segment = (day, stream, segment_key)
        segment_settings[segment] = setting
        conversation_keys[segment] = _conversation_key(
            day,
            stream,
            segment_key,
            setting,
        )

    conversations = set(conversation_keys.values())
    return _ClusterConversationContext(
        distinct_segments=tuple(distinct_segments),
        first_record_by_segment=first_record_by_segment,
        segment_settings=segment_settings,
        conversation_keys=conversation_keys,
        conversation_count=len(conversations),
    )


def get_cluster_conversation_count(members: list[dict[str, Any]]) -> int:
    """Return distinct conversation count for valid discovery-cache members."""
    return _cluster_conversation_context(members).conversation_count


def resolve_statement_cluster(
    *,
    day: str,
    stream: str,
    segment_key: str,
    source: str,
    sentence_id: int,
) -> dict[str, Any]:
    """Resolve one statement identity to a discovery cluster in the current cache."""
    cache = load_discovery_cache()
    if cache is None:
        return {"status": "cache_unavailable", "cluster_id": None}

    clusters = cache.get("clusters", {})
    eligible: list[tuple[int, list[dict[str, Any]]]] = []
    for raw_cluster_id, members in clusters.items():
        try:
            cluster_id = int(raw_cluster_id)
        except (TypeError, ValueError):
            continue
        if isinstance(members, list):
            eligible.append((cluster_id, members))

    for cluster_id, members in sorted(eligible, key=lambda item: item[0]):
        for member in members:
            normalized = _normalized_cache_member(member)
            if normalized is None:
                continue
            if (
                normalized["day"] == day
                and normalized["stream"] == stream
                and normalized["segment_key"] == segment_key
                and normalized["source"] == source
                and normalized["sentence_id"] == sentence_id
            ):
                return {"status": "hit", "cluster_id": cluster_id}

    return {"status": "miss", "cluster_id": None}


def _voiceprints_exist(entity_id: str) -> bool:
    return (Path(get_journal()) / "entities" / entity_id / "voiceprints.npz").exists()


def _presence_candidate(
    entity_id: str,
    buckets: dict[str, set],
) -> dict[str, Any] | None:
    entity = load_journal_entity(entity_id)
    if entity is None or entity.get("blocked"):
        return None
    return {
        "entity_id": entity_id,
        "name": entity["name"],
        "has_voice": _voiceprints_exist(entity_id),
        "screen_conversations": len(buckets["screen"]),
        "meeting_days": len(buckets["meeting_day"]),
        "setting_conversations": len(buckets["setting"]),
        "speaker_conversations": len(buckets["speakers"]),
    }


def get_cluster_presence(cluster_id: int) -> dict[str, Any] | None:
    """Return read-only co-presence evidence for a discovered cluster."""
    cache = load_discovery_cache()
    if cache is None:
        return None
    members = cache.get("clusters", {}).get(str(cluster_id))
    if not isinstance(members, list) or not members:
        return None

    _, load_speaker_labels, _, _, _ = _routes_helpers()
    conversation_context = _cluster_conversation_context(members)
    distinct_segments = list(conversation_context.distinct_segments)
    first_record_by_segment = conversation_context.first_record_by_segment
    segment_settings = conversation_context.segment_settings
    conversation_keys = conversation_context.conversation_keys

    samples: list[dict[str, Any]] = []
    for segment in distinct_segments[:3]:
        sample = _build_cluster_sample(first_record_by_segment[segment])
        sample["setting"] = segment_settings[segment]
        samples.append(sample)

    entity_buckets: dict[str, dict[str, set]] = defaultdict(
        lambda: {
            "screen": set(),
            "meeting_day": set(),
            "setting": set(),
            "speakers": set(),
        }
    )
    evidence_gaps: list[dict[str, Any]] = []

    for day, stream, segment_key in distinct_segments:
        seg_dir = segment_path(day, segment_key, stream, create=False)
        labels = load_speaker_labels(seg_dir)
        if isinstance(labels, dict) and "candidate_evidence" in labels:
            evidence = labels.get("candidate_evidence") or []
            seg_gaps = labels.get("candidate_evidence_gaps") or []
        else:
            evidence, seg_gaps = compute_segment_candidate_evidence_readonly(
                day,
                stream,
                segment_key,
            )

        for gap in seg_gaps:
            if isinstance(gap, dict):
                evidence_gaps.append(
                    {"day": day, "stream": stream, "segment_key": segment_key, **gap}
                )

        conversation_key = conversation_keys[(day, stream, segment_key)]
        for item in evidence:
            if not isinstance(item, dict):
                continue
            entity_id = item.get("entity_id")
            sources = item.get("sources") or []
            if not entity_id or not isinstance(sources, list):
                continue
            buckets = entity_buckets[str(entity_id)]
            for source in sources:
                if source == "screen":
                    buckets["screen"].add(conversation_key)
                elif source == "meeting_day":
                    buckets["meeting_day"].add(day)
                elif source == "setting":
                    buckets["setting"].add(conversation_key)
                elif source == "speakers":
                    buckets["speakers"].add(conversation_key)

    principal = get_journal_principal()
    principal_id = principal.get("id") if isinstance(principal, dict) else None
    candidates: list[dict[str, Any]] = []
    for entity_id, buckets in entity_buckets.items():
        if entity_id == principal_id:
            continue
        candidate = _presence_candidate(entity_id, buckets)
        if candidate is not None:
            candidates.append(candidate)

    co_presence = [
        candidate
        for candidate in candidates
        if candidate["screen_conversations"] > 0 or candidate["meeting_days"] > 0
    ]
    co_presence.sort(
        key=lambda candidate: (
            -candidate["screen_conversations"],
            -candidate["meeting_days"],
            candidate["name"],
            candidate["entity_id"],
        )
    )

    mention = [
        candidate
        for candidate in candidates
        if candidate not in co_presence
        and (
            candidate["setting_conversations"] > 0
            or candidate["speaker_conversations"] > 0
        )
    ]
    mention.sort(
        key=lambda candidate: (
            -candidate["setting_conversations"],
            -candidate["speaker_conversations"],
            candidate["name"],
            candidate["entity_id"],
        )
    )

    days = {day for day, _stream, _segment_key in distinct_segments}
    streams = {stream for _day, stream, _segment_key in distinct_segments}

    return {
        "cluster_id": cluster_id,
        "facts": {
            "statement_count": len(members),
            "segment_count": len(distinct_segments),
            "day_count": len(days),
            "streams": sorted(streams),
            "conversation_count": conversation_context.conversation_count,
            "samples": samples,
        },
        "evidence_complete": len(evidence_gaps) == 0,
        "evidence_gaps": evidence_gaps,
        "candidates": {
            "co_presence": co_presence,
            "mention": mention,
        },
    }


def identify_cluster(
    cluster_id: int,
    name: str | None = None,
    entity_id: str | None = None,
    *,
    resolve_only: bool = False,
    create_new: bool = False,
    entity_type: str = "Person",
    request_id: str | None = None,
    reviewed_near_match_entity_ids: list[str] | None = None,
) -> dict[str, Any]:
    """Identify a discovered unknown speaker cluster through the native owner."""
    return native_speakers.identify(
        get_journal(),
        cluster_id=cluster_id,
        name=name,
        entity_id=entity_id,
        resolve_only=resolve_only,
        create_new=create_new,
        entity_type=entity_type,
        request_id=request_id or f"server:{uuid.uuid4().hex}",
        reviewed_near_match_entity_ids=reviewed_near_match_entity_ids,
        caller=None,
        actor=None,
        encoder=_speaker_encoder_identity(),
    )


def undo_identify_operation(operation_id: str) -> dict[str, Any]:
    """Undo one speaker identify operation through the native owner."""
    return native_speakers.undo_identify(
        get_journal(),
        operation_id=operation_id,
        encoder=_speaker_encoder_identity(),
    )
