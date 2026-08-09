# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Tests for unknown speaker discovery."""

from __future__ import annotations

import json
import os
import sys
import time
from functools import partial
from pathlib import Path

import numpy as np
import pytest

from solstone.apps.speakers import discovery as discovery_module
from solstone.apps.speakers.discovery import (
    DISCOVERY_CLUSTER_ALGORITHM,
    DISCOVERY_CLUSTER_DTYPE,
    DISCOVERY_CLUSTER_PAYLOAD_FORMAT,
    DISCOVERY_CLUSTER_REQUEST_SCHEMA,
    DISCOVERY_CLUSTER_RESPONSE_SCHEMA,
    MIN_CLUSTER_SIZE,
    MIN_SAMPLES,
    SpeakerDiscoveryKernelError,
    _discovery_cache_path,
    _discovery_resolved_path,
    create_discovery_cluster_temp_dir,
    discover_unknown_speakers,
    get_cluster_conversation_count,
    get_cluster_presence,
    identify_cluster,
    load_discovery_cache,
    resolve_statement_cluster,
    undo_identify_operation,
)
from solstone.apps.speakers.owner import OWNER_THRESHOLD
from solstone.apps.speakers.tests.conftest import journal_tree_hash
from solstone.observe.transcribe.speakers_analyze_adapter import (
    HelperInvocationResult,
    SpeakersAnalyzeBudget,
    invoke_speakers_analyze_helper,
)
from solstone.observe.transcribe.speakers_analyze_errors import SpeakerAnalyzeError


def _domain_tree_hash(journal: Path) -> dict[str, str]:
    return {
        path: digest
        for path, digest in journal_tree_hash(journal).items()
        if not path.endswith(".lock")
    }


def _make_speaker_embeddings(
    base_vector: list[float],
    count: int,
    noise_scale: float = 0.0,
) -> np.ndarray:
    """Create a cluster of similar embeddings around a base direction."""
    base = np.array(base_vector + [0.0] * (256 - len(base_vector)), dtype=np.float32)
    base = base / np.linalg.norm(base)
    rng = np.random.default_rng(42)
    noise = rng.normal(0, noise_scale, (count, 256)).astype(np.float32)
    embeddings = base + noise
    norms = np.linalg.norm(embeddings, axis=1, keepdims=True)
    return embeddings / norms


def _discovery_response(labels: list[int]) -> dict:
    return {
        "schema": DISCOVERY_CLUSTER_RESPONSE_SCHEMA,
        "labels": labels,
        "cluster_count": len({label for label in labels if label != -1}),
        "noise_count": sum(1 for label in labels if label == -1),
        "parameters": {
            "min_cluster_size": MIN_CLUSTER_SIZE,
            "min_samples": MIN_SAMPLES,
        },
        "algorithm": DISCOVERY_CLUSTER_ALGORITHM,
    }


def _matrix_from_discovery_request(stdin_text: str) -> np.ndarray:
    request = json.loads(stdin_text)
    assert request["schema"] == DISCOVERY_CLUSTER_REQUEST_SCHEMA
    assert request["payload_format"] == DISCOVERY_CLUSTER_PAYLOAD_FORMAT
    assert request["dtype"] == DISCOVERY_CLUSTER_DTYPE
    assert request["min_cluster_size"] == MIN_CLUSTER_SIZE
    assert request["min_samples"] == MIN_SAMPLES
    shape = request["shape"]
    assert isinstance(shape, list)
    assert len(shape) == 2
    rows, cols = int(shape[0]), int(shape[1])
    payload = np.fromfile(request["embeddings_f32le_path"], dtype="<f4")
    assert payload.size == rows * cols
    return payload.reshape((rows, cols))


def _group_identical_unit_rows(matrix: np.ndarray) -> list[int]:
    labels = [-1] * int(matrix.shape[0])
    groups: dict[tuple[float, ...], list[int]] = {}
    for index, row in enumerate(matrix):
        key = tuple(np.round(row.astype(np.float64), 6).tolist())
        groups.setdefault(key, []).append(index)

    next_label = 0
    for indices in groups.values():
        if len(indices) < MIN_CLUSTER_SIZE:
            continue
        for index in indices:
            labels[index] = next_label
        next_label += 1
    return labels


def _grouping_discovery_helper(
    _argv: list[str],
    stdin_text: str,
) -> HelperInvocationResult:
    matrix = _matrix_from_discovery_request(stdin_text)
    labels = _group_identical_unit_rows(matrix)
    return HelperInvocationResult(0, json.dumps(_discovery_response(labels)), "")


def _discover_with_grouping_double() -> dict:
    return discover_unknown_speakers(
        helper_locator=lambda: Path("/tmp/solstone-core-speakers-analyze"),
        helper_invoker=_grouping_discovery_helper,
    )


def _setup_owner_centroid(
    journal: Path,
    vector: list[float],
    entity_id: str = "owner_test",
) -> np.ndarray:
    """Create owner entity with centroid for testing."""
    base = np.array(vector + [0.0] * (256 - len(vector)), dtype=np.float32)
    centroid = base / np.linalg.norm(base)
    entity_dir = journal / "entities" / entity_id
    entity_dir.mkdir(parents=True, exist_ok=True)
    (entity_dir / "entity.json").write_text(
        json.dumps(
            {
                "id": entity_id,
                "name": "Owner Test",
                "type": "Person",
                "is_principal": True,
            }
        ),
        encoding="utf-8",
    )
    np.savez_compressed(
        entity_dir / "owner_centroid.npz",
        centroid=centroid,
        cluster_size=np.array(100, dtype=np.int32),
        threshold=np.array(OWNER_THRESHOLD, dtype=np.float32),
        last_refreshed_at=np.array("2026-01-01T00:00:00Z"),
    )
    return centroid


def _create_cluster_segments(
    env,
    embeddings: np.ndarray,
    *,
    audio_extension: str = ".flac",
) -> list[tuple[str, str, int]]:
    """Create four segments with one qualifying cluster and one filtered cluster."""
    segments = [
        ("20240101", "090000_300"),
        ("20240102", "090000_300"),
        ("20240103", "090000_300"),
        ("20240104", "090000_300"),
    ]
    alt_embeddings = _make_speaker_embeddings([0.0, 0.0, 1.0], embeddings.shape[0])
    results = []
    for idx, (day, segment_key) in enumerate(segments):
        segment_embeddings = embeddings
        if idx < 2:
            segment_embeddings = np.vstack([embeddings, alt_embeddings])
        env.create_segment(
            day,
            segment_key,
            ["audio"],
            embeddings=segment_embeddings,
            audio_extension=audio_extension,
        )
        results.append((day, segment_key, segment_embeddings.shape[0]))
    return results


def _all_sentence_labels(entity_id: str, count: int) -> list[dict]:
    """Build fully attributed labels for a segment."""
    return [
        {
            "sentence_id": idx,
            "speaker": entity_id,
            "confidence": "high",
            "method": "user_identified",
        }
        for idx in range(1, count + 1)
    ]


def _load_voiceprint_count(journal: Path, entity_id: str) -> int:
    """Return number of saved voiceprints for an entity."""
    path = journal / "entities" / entity_id / "voiceprints.npz"
    if not path.exists():
        return 0
    data = np.load(path, allow_pickle=False)
    return int(len(data["embeddings"]))


def _load_corrections_count(journal: Path, day: str, segment_key: str) -> int:
    """Return number of correction entries for a segment."""
    path = journal / day / "test" / segment_key / "talents" / "speaker_corrections.json"
    if not path.exists():
        return 0
    return len(json.loads(path.read_text(encoding="utf-8")).get("corrections", []))


def _corrections_path(journal: Path, day: str, segment_key: str) -> Path:
    return journal / day / "test" / segment_key / "talents" / "speaker_corrections.json"


def _speaker_corrections_for_segment(
    journal: Path,
    day: str,
    segment_key: str,
) -> list[dict]:
    path = _corrections_path(journal, day, segment_key)
    if not path.exists():
        return []
    return json.loads(path.read_text(encoding="utf-8")).get("corrections", [])


def _identify_ledger_lines_for_operation(
    journal: Path,
    operation_id: str,
) -> list[bytes]:
    path = journal / "speakers" / "identify-operations.jsonl"
    lines = []
    for line in path.read_bytes().splitlines():
        row = json.loads(line)
        if row.get("operation_id") == operation_id:
            lines.append(line)
    return lines


def _canonical_row_bytes(rows: list[dict]) -> list[bytes]:
    return [
        json.dumps(row, sort_keys=True, separators=(",", ":")).encode("utf-8")
        for row in rows
    ]


def _write_discovery_cache(env, cluster_id: int, records: list[dict]) -> None:
    awareness_dir = env.journal / "awareness"
    awareness_dir.mkdir(parents=True, exist_ok=True)
    cache_path = awareness_dir / "discovery_clusters.json"
    cache = {"version": "test", "clusters": {}}
    if cache_path.exists():
        cache = json.loads(cache_path.read_text(encoding="utf-8"))
    cache.setdefault("clusters", {})[str(cluster_id)] = records
    (awareness_dir / "discovery_clusters.json").write_text(
        json.dumps(cache, indent=2),
        encoding="utf-8",
    )


def _cluster_record(
    day: str,
    segment_key: str,
    *,
    stream: str = "test",
    source: str = "audio",
    sentence_id: int = 1,
) -> dict:
    return {
        "day": day,
        "stream": stream,
        "segment_key": segment_key,
        "source": source,
        "sentence_id": sentence_id,
    }


def _candidate_evidence(*items: tuple[str, list[str]]) -> dict:
    return {
        "owner_centroid_last_refreshed_at": None,
        "voiceprint_versions": {},
        "candidate_evidence": [
            {"entity_id": entity_id, "sources": sources} for entity_id, sources in items
        ],
    }


def _create_identify_cluster(
    env,
    cluster_id: int,
    segment_key: str,
    *,
    day: str = "20240101",
    sentence_count: int = 1,
) -> None:
    embeddings = _make_speaker_embeddings([1.0, 0.0], sentence_count)
    env.create_segment(day, segment_key, ["audio"], embeddings=embeddings)
    _write_discovery_cache(
        env,
        cluster_id,
        [
            _cluster_record(day, segment_key, sentence_id=sentence_id)
            for sentence_id in range(1, sentence_count + 1)
        ],
    )


def _update_entity(env, entity_id: str, **updates) -> None:
    entity_path = env.journal / "entities" / entity_id / "entity.json"
    entity = json.loads(entity_path.read_text(encoding="utf-8"))
    entity.update(updates)
    entity_path.write_text(json.dumps(entity), encoding="utf-8")


def _setup_mixed_person_entities(env) -> None:
    _setup_owner_centroid(env.journal, [0.0, 1.0], entity_id="owner_test")
    env.create_entity("Sarah Connor")
    env.create_entity("Sarah Lee")
    env.create_entity("Sarah Org")
    env.create_entity("Sarah Blocked")
    env.create_entity("Other Person")
    _update_entity(env, "sarah_org", type="Organization")
    _update_entity(env, "sarah_blocked", blocked=True)


def _setup_no_match_near_entities(env) -> None:
    _setup_owner_centroid(env.journal, [0.0, 1.0], entity_id="owner_test")
    _update_entity(env, "owner_test", name="Jnthn Smth Owner")
    env.create_entity("Jonathan Smith")
    env.create_entity("Jnthn Smth Org")
    env.create_entity("Jnthn Smth Blocked")
    _update_entity(env, "jnthn_smth_org", type="Organization")
    _update_entity(env, "jnthn_smth_blocked", blocked=True)


def _assert_no_identify_write_boundary(
    env,
    *,
    target_id: str,
    segment_key: str,
    before: dict[str, str],
    target_must_be_absent: bool = True,
) -> None:
    assert _domain_tree_hash(env.journal) == before
    target_dir = env.journal / "entities" / target_id
    if target_must_be_absent:
        assert not target_dir.exists()
    else:
        assert target_dir.exists()
        assert not (target_dir / "voiceprints.npz").exists()
    assert _speaker_labels_for_segment(env.journal, "20240101", segment_key) == []
    assert _load_corrections_count(env.journal, "20240101", segment_key) == 0
    assert not (env.journal / "speakers" / "identify-operations.jsonl").exists()
    assert not (env.journal / "speakers" / "keep-separate.jsonl").exists()
    assert not _discovery_resolved_path().exists()
    assert not (env.journal / "awareness" / "speaker_candidates.json").exists()


def _speaker_labels_for_segment(
    journal: Path, day: str, segment_key: str
) -> list[dict]:
    path = journal / day / "test" / segment_key / "talents" / "speaker_labels.json"
    if not path.exists():
        return []
    return json.loads(path.read_text(encoding="utf-8")).get("labels", [])


def _create_integer_labeled_segment(
    env,
    day: str,
    segment_key: str,
    embeddings: np.ndarray,
    *,
    cluster_label: int = 7,
) -> Path:
    seg_dir = env.create_segment(
        day,
        segment_key,
        ["audio"],
        embeddings=embeddings,
    )
    jsonl_path = seg_dir / "audio.jsonl"
    rows = jsonl_path.read_text(encoding="utf-8").splitlines()
    updated = [rows[0]]
    for row in rows[1:]:
        payload = json.loads(row)
        payload["speaker"] = cluster_label
        updated.append(json.dumps(payload))
    jsonl_path.write_text("\n".join(updated) + "\n", encoding="utf-8")
    return seg_dir


def test_load_discovery_cache_missing_is_read_only(tmp_path, monkeypatch):
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    import solstone.think.utils as think_utils

    think_utils._journal_path_cache = None

    assert load_discovery_cache() is None
    assert not (tmp_path / "awareness").exists()


def test_load_discovery_cache_non_dict_top_level_is_unavailable(tmp_path, monkeypatch):
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    import solstone.think.utils as think_utils

    think_utils._journal_path_cache = None
    awareness_dir = tmp_path / "awareness"
    awareness_dir.mkdir()
    (awareness_dir / "discovery_clusters.json").write_text("[]\n", encoding="utf-8")

    assert load_discovery_cache() is None


def test_resolve_statement_cluster_distinguishes_hit_miss_and_unavailable(
    speakers_env,
):
    env = speakers_env()

    assert resolve_statement_cluster(
        day="20240101",
        stream="test",
        segment_key="090000_300",
        source="audio",
        sentence_id=1,
    ) == {"status": "cache_unavailable", "cluster_id": None}

    _write_discovery_cache(
        env,
        8,
        [_cluster_record("20240101", "090000_300", source="audio", sentence_id=12)],
    )
    _write_discovery_cache(
        env,
        9,
        [_cluster_record("20240101", "090000_300", source="screen", sentence_id=12)],
    )
    _write_discovery_cache(
        env,
        3,
        [_cluster_record("20240101", "091000_300", source="audio", sentence_id=1)],
    )

    assert resolve_statement_cluster(
        day="20240101",
        stream="test",
        segment_key="090000_300",
        source="screen",
        sentence_id=12,
    ) == {"status": "hit", "cluster_id": 9}
    assert resolve_statement_cluster(
        day="20240101",
        stream="test",
        segment_key="090000_300",
        source="audio",
        sentence_id=12,
    ) == {"status": "hit", "cluster_id": 8}
    assert resolve_statement_cluster(
        day="20240101",
        stream="test",
        segment_key="090000_300",
        source="imported_audio",
        sentence_id=12,
    ) == {"status": "miss", "cluster_id": None}

    (_discovery_cache_path()).write_text("[]\n", encoding="utf-8")
    assert resolve_statement_cluster(
        day="20240101",
        stream="test",
        segment_key="090000_300",
        source="audio",
        sentence_id=12,
    ) == {"status": "cache_unavailable", "cluster_id": None}


def test_discover_no_owner_centroid(speakers_env):
    speakers_env()

    result = _discover_with_grouping_double()

    assert result == {
        "status": "degraded",
        "clusters": [],
        "issues": [
            {
                "reason_code": "speaker_discovery_owner_voice_unavailable",
                "message": "i need your voice set up before looking for new voices.",
                "count": 0,
            }
        ],
    }


def test_discover_no_unmatched(speakers_env):
    env = speakers_env()
    _setup_owner_centroid(env.journal, [0.0, 1.0])
    env.create_entity("Alice Test")
    embeddings = _make_speaker_embeddings([1.0, 0.0], 5)
    segments = _create_cluster_segments(env, embeddings)

    for day, segment_key, sentence_count in segments:
        env.create_speaker_labels(
            day,
            segment_key,
            _all_sentence_labels("alice_test", sentence_count),
        )

    result = _discover_with_grouping_double()

    assert result == {"status": "ok", "clusters": [], "issues": []}


def test_discover_clusters_found(speakers_env):
    env = speakers_env()
    _setup_owner_centroid(env.journal, [0.0, 1.0])
    embeddings = _make_speaker_embeddings([1.0, 0.0], 5)
    _create_cluster_segments(env, embeddings)

    result = _discover_with_grouping_double()

    assert result["status"] == "ok"
    assert result["issues"] == []
    expected_samples = [
        {
            "day": day,
            "stream": "test",
            "segment_key": "090000_300",
            "source": "audio",
            "sentence_id": 5,
            "audio_url": f"/app/speakers/api/serve_audio/{day}/test/090000_300/audio.flac",
            "text": "This is sentence 5.",
        }
        for day in ("20240104", "20240103", "20240102")
    ]
    assert result["clusters"] == [
        {
            "cluster_id": 0,
            "size": 20,
            "segment_count": 4,
            "samples": expected_samples,
        }
    ]
    cluster = result["clusters"][0]
    cache_path = _discovery_cache_path()
    cache_bytes = cache_path.read_bytes()
    cache_payload = json.loads(cache_bytes)
    assert set(cache_payload) == {"version", "clusters"}
    assert cache_payload["clusters"] == {
        "0": [
            _cluster_record(day, "090000_300", sentence_id=sentence_id)
            for day in ("20240104", "20240103", "20240102", "20240101")
            for sentence_id in range(5, 0, -1)
        ]
    }
    assert cache_bytes == json.dumps(cache_payload, indent=2).encode("utf-8")
    assert b'"status"' not in cache_bytes
    assert b'"issues"' not in cache_bytes
    assert cluster["size"] == 20
    assert cluster["segment_count"] == 4
    assert len(cluster["samples"]) == 3


def test_discover_samples_use_registered_audio_extension(speakers_env):
    env = speakers_env()
    _setup_owner_centroid(env.journal, [0.0, 1.0])
    embeddings = _make_speaker_embeddings([1.0, 0.0], 5)
    _create_cluster_segments(env, embeddings, audio_extension=".m4a")

    result = _discover_with_grouping_double()

    samples = result["clusters"][0]["samples"]
    assert len(samples) == 3
    for sample in samples:
        assert sample["audio_url"] == (
            f"/app/speakers/api/serve_audio/{sample['day']}/"
            f"{sample['stream']}/{sample['segment_key']}/{sample['source']}.m4a"
        )


def test_discover_samples_allow_missing_audio(speakers_env):
    env = speakers_env()
    _setup_owner_centroid(env.journal, [0.0, 1.0])
    embeddings = _make_speaker_embeddings([1.0, 0.0], 5)
    segments = _create_cluster_segments(env, embeddings)
    for day, segment_key, _sentence_count in segments:
        (env.journal / "chronicle" / day / "test" / segment_key / "audio.flac").unlink()

    result = _discover_with_grouping_double()

    samples = result["clusters"][0]["samples"]
    assert len(samples) == 3
    assert all(sample["audio_url"] is None for sample in samples)


def test_discover_filters_attributed(speakers_env):
    env = speakers_env()
    _setup_owner_centroid(env.journal, [0.0, 1.0])
    env.create_entity("Alice Test")
    embeddings = _make_speaker_embeddings([1.0, 0.0], 5)
    segments = _create_cluster_segments(env, embeddings)

    for day, segment_key, sentence_count in segments[:3]:
        env.create_speaker_labels(
            day,
            segment_key,
            _all_sentence_labels("alice_test", sentence_count),
        )

    result = _discover_with_grouping_double()

    assert result == {"status": "ok", "clusters": [], "issues": []}


def test_discover_sample_shape_stays_scan_stable(speakers_env):
    env = speakers_env()
    _setup_owner_centroid(env.journal, [0.0, 1.0])
    embeddings = _make_speaker_embeddings([1.0, 0.0], 5)
    _create_cluster_segments(env, embeddings)

    result = _discover_with_grouping_double()

    sample = result["clusters"][0]["samples"][0]
    assert set(sample) == {
        "day",
        "stream",
        "segment_key",
        "source",
        "sentence_id",
        "audio_url",
        "text",
    }


def _seed_kernel_reaching_discovery(env) -> None:
    _setup_owner_centroid(env.journal, [0.0, 1.0])
    embeddings = _make_speaker_embeddings([1.0, 0.0], 5)
    _create_cluster_segments(env, embeddings)


def _seed_discovery_cache_files(env) -> tuple[Path, Path, bytes, bytes]:
    cache_path = _discovery_cache_path(create=True)
    resolved_path = _discovery_resolved_path(create=True)
    cache_path.write_bytes(b'{"version":"old","clusters":{}}\n')
    resolved_path.write_bytes(b'{"resolved":true}\n')
    return (
        cache_path,
        resolved_path,
        cache_path.read_bytes(),
        resolved_path.read_bytes(),
    )


def _assert_kernel_failure_preserves_cache(
    env,
    helper_invoker,
    *,
    reason: str,
) -> SpeakerDiscoveryKernelError:
    cache_path, resolved_path, cache_before, resolved_before = (
        _seed_discovery_cache_files(env)
    )
    tree_before = journal_tree_hash(env.journal)
    with pytest.raises(SpeakerDiscoveryKernelError) as exc:
        discover_unknown_speakers(
            helper_locator=lambda: Path("/tmp/solstone-core-speakers-analyze"),
            helper_invoker=helper_invoker,
        )

    assert exc.value.reason == reason
    assert cache_path.read_bytes() == cache_before
    assert resolved_path.read_bytes() == resolved_before
    assert journal_tree_hash(env.journal) == tree_before
    return exc.value


def _response_helper(response: dict | str) -> HelperInvocationResult:
    stdout = response if isinstance(response, str) else json.dumps(response)
    return HelperInvocationResult(0, stdout, "")


@pytest.mark.parametrize(
    ("name", "helper_factory", "reason"),
    [
        (
            "nonzero",
            lambda rows: lambda _argv, _stdin: HelperInvocationResult(7, "", ""),
            "exit-7",
        ),
        (
            "malformed-json",
            lambda rows: lambda _argv, _stdin: _response_helper("{"),
            "response-json-invalid",
        ),
        (
            "schema",
            lambda rows: (
                lambda _argv, _stdin: _response_helper(
                    {
                        **_discovery_response([0] * rows),
                        "schema": "wrong",
                    }
                )
            ),
            "schema-mismatch",
        ),
        (
            "labels-invalid",
            lambda rows: (
                lambda _argv, _stdin: _response_helper(
                    {
                        **_discovery_response([0] * rows),
                        "labels": [True] * rows,
                    }
                )
            ),
            "labels-invalid",
        ),
        (
            "wrong-length",
            lambda rows: (
                lambda _argv, _stdin: _response_helper(
                    _discovery_response([0] * (rows - 1))
                )
            ),
            "label-count-mismatch",
        ),
        (
            "parameters",
            lambda rows: (
                lambda _argv, _stdin: _response_helper(
                    {
                        **_discovery_response([0] * rows),
                        "parameters": {
                            "min_cluster_size": MIN_CLUSTER_SIZE + 1,
                            "min_samples": MIN_SAMPLES,
                        },
                    }
                )
            ),
            "parameters-mismatch",
        ),
        (
            "algorithm",
            lambda rows: (
                lambda _argv, _stdin: _response_helper(
                    {
                        **_discovery_response([0] * rows),
                        "algorithm": "wrong",
                    }
                )
            ),
            "algorithm-mismatch",
        ),
        (
            "counts",
            lambda rows: (
                lambda _argv, _stdin: _response_helper(
                    {
                        **_discovery_response([0] * rows),
                        "cluster_count": 99,
                    }
                )
            ),
            "count-mismatch",
        ),
    ],
)
def test_discovery_kernel_failure_preserves_existing_cache(
    speakers_env,
    name,
    helper_factory,
    reason,
):
    env = speakers_env()
    _seed_kernel_reaching_discovery(env)

    def helper(argv: list[str], stdin_text: str) -> HelperInvocationResult:
        matrix = _matrix_from_discovery_request(stdin_text)
        return helper_factory(int(matrix.shape[0]))(argv, stdin_text)

    assert name
    _assert_kernel_failure_preserves_cache(env, helper, reason=reason)


def test_discovery_kernel_timeout_preserves_existing_cache(speakers_env):
    env = speakers_env()
    _seed_kernel_reaching_discovery(env)
    budget = SpeakersAnalyzeBudget(
        timeout_s=0.01,
        stdout_limit_bytes=1024,
        stderr_limit_bytes=1024,
        terminate_grace_s=0.05,
        kill_grace_s=0.5,
    )
    real_invoker = partial(
        invoke_speakers_analyze_helper,
        budget=budget,
        clock=time.monotonic,
    )

    def helper(_argv: list[str], stdin_text: str) -> HelperInvocationResult:
        return real_invoker(
            [sys.executable, "-c", "import time; time.sleep(10)"],
            stdin_text,
            Path("speaker-discovery-cluster"),
        )

    _assert_kernel_failure_preserves_cache(env, helper, reason="timeout")


@pytest.mark.parametrize(
    ("name", "helper_invoker", "reason", "exit_code"),
    [
        (
            "popen-oserror",
            lambda: partial(
                invoke_speakers_analyze_helper,
                budget=SpeakersAnalyzeBudget(timeout_s=1.0),
                popen_factory=lambda *_args, **_kwargs: (_ for _ in ()).throw(
                    OSError("boom")
                ),
                clock=time.monotonic,
            ),
            "oserror",
            None,
        ),
        (
            "timeout",
            lambda: partial(
                invoke_speakers_analyze_helper,
                budget=SpeakersAnalyzeBudget(
                    timeout_s=0.01,
                    terminate_grace_s=0.05,
                    kill_grace_s=0.5,
                ),
                clock=time.monotonic,
            ),
            "timeout",
            None,
        ),
        (
            "stdout-too-large",
            lambda: partial(
                invoke_speakers_analyze_helper,
                budget=SpeakersAnalyzeBudget(
                    timeout_s=1.0,
                    stdout_limit_bytes=64,
                    stderr_limit_bytes=64,
                    terminate_grace_s=0.05,
                    kill_grace_s=0.5,
                ),
                clock=time.monotonic,
            ),
            "stdout-too-large",
            None,
        ),
        (
            "stderr-too-large",
            lambda: partial(
                invoke_speakers_analyze_helper,
                budget=SpeakersAnalyzeBudget(
                    timeout_s=1.0,
                    stdout_limit_bytes=64,
                    stderr_limit_bytes=64,
                    terminate_grace_s=0.05,
                    kill_grace_s=0.5,
                ),
                clock=time.monotonic,
            ),
            "stderr-too-large",
            None,
        ),
        (
            "nonzero",
            lambda: None,
            "exit-9",
            9,
        ),
    ],
)
def test_speaker_analyze_errors_translate_totally_to_discovery_errors(
    speakers_env,
    name,
    helper_invoker,
    reason,
    exit_code,
):
    env = speakers_env()
    _seed_kernel_reaching_discovery(env)

    def helper(_argv: list[str], stdin_text: str) -> HelperInvocationResult:
        if exit_code is not None:
            return HelperInvocationResult(exit_code, "", "")
        invoker = helper_invoker()
        if name == "timeout":
            argv = [sys.executable, "-c", "import time; time.sleep(10)"]
        elif name == "stdout-too-large":
            argv = [
                sys.executable,
                "-c",
                "import sys, time; sys.stdout.write('x' * 2048); "
                "sys.stdout.flush(); time.sleep(10)",
            ]
        elif name == "stderr-too-large":
            argv = [
                sys.executable,
                "-c",
                "import sys, time; sys.stderr.write('x' * 2048); "
                "sys.stderr.flush(); time.sleep(10)",
            ]
        else:
            argv = [sys.executable, "-c", ""]
        return invoker(argv, stdin_text, Path("speaker-discovery-cluster"))

    error = _assert_kernel_failure_preserves_cache(env, helper, reason=reason)

    assert not isinstance(error, SpeakerAnalyzeError)
    assert all(
        not key.startswith("speaker_analysis_failure_") for key in error.event_fields()
    )


def test_discovery_admission_filter_drops_nonfinite_and_nonunit_rows(
    speakers_env,
    caplog,
):
    env = speakers_env()
    _setup_owner_centroid(env.journal, [0.0, 1.0])
    valid = _make_speaker_embeddings([1.0, 0.0], 5)
    invalid_inf = np.array([[np.inf] + [0.0] * 255], dtype=np.float32)
    invalid_nonunit = np.array([[2.0] + [0.0] * 255], dtype=np.float32)
    days = ["20240101", "20240102", "20240103", "20240104"]
    for idx, day in enumerate(days):
        embeddings = valid
        if idx == 0:
            embeddings = np.vstack([valid, invalid_inf, invalid_nonunit])
        env.create_segment(day, "090000_300", ["audio"], embeddings=embeddings)

    caplog.set_level("INFO")

    result = _discover_with_grouping_double()

    assert set(result) == {"status", "clusters", "issues"}
    assert result["status"] == "degraded"
    assert result["issues"] == [
        {
            "reason_code": "speaker_discovery_invalid_embeddings",
            "message": "i skipped some voice samples because they were not usable.",
            "count": 2,
        }
    ]
    assert result["clusters"][0]["size"] == 20
    assert "dropped_invalid_embeddings=2" in caplog.text
    cache = load_discovery_cache()
    assert cache is not None
    members = cache["clusters"][str(result["clusters"][0]["cluster_id"])]
    assert {
        (member["day"], member["sentence_id"])
        for member in members
        if member["day"] == "20240101"
    } == {("20240101", sid) for sid in range(1, 6)}


def test_discovery_admission_filter_all_rejected_reports_degraded_empty(
    speakers_env,
):
    env = speakers_env()
    _setup_owner_centroid(env.journal, [0.0, 1.0])
    invalid = np.tile(np.array([[2.0] + [0.0] * 255], dtype=np.float32), (5, 1))
    env.create_segment("20240101", "090000_300", ["audio"], embeddings=invalid)

    result = _discover_with_grouping_double()

    assert result == {
        "status": "degraded",
        "clusters": [],
        "issues": [
            {
                "reason_code": "speaker_discovery_invalid_embeddings",
                "message": "i skipped some voice samples because they were not usable.",
                "count": 5,
            }
        ],
    }
    assert load_discovery_cache() is None


def test_discovery_temp_dirs_are_reaped_by_speakers_analyze_sweeper(
    tmp_path,
    monkeypatch,
):
    from solstone.observe.transcribe import speakers_analyze_adapter

    monkeypatch.setattr(discovery_module, "TEMP_ROOT", tmp_path)
    monkeypatch.setattr(speakers_analyze_adapter, "TEMP_ROOT", tmp_path)
    temp_dir = create_discovery_cluster_temp_dir()
    assert temp_dir.name.startswith("solstone-speakers-analyze-discovery-cluster-")
    old = time.time() - 200
    os.utime(temp_dir, (old, old))

    assert (
        speakers_analyze_adapter.sweep_stale_speakers_analyze_dirs(max_age_seconds=100)
        == 1
    )
    assert not temp_dir.exists()


def test_cluster_presence_aggregates_persisted_evidence_and_ranks(speakers_env):
    env = speakers_env()
    env.create_entity(
        "Alice Co",
        voiceprints=[("20240101", "080000_300", "audio", 1)],
    )
    env.create_entity("Bob Co")
    env.create_entity("Carol Mention")
    env.create_entity("Dave Speaker")
    segments = [
        ("20240101", "091000_300", "Room A"),
        ("20240101", "091500_300", "Room A"),
        ("20240101", "092000_300", "Room B"),
    ]
    for day, segment_key, setting in segments:
        env.create_import_segment(
            day,
            segment_key,
            [("", "Unknown voice.")],
            stream="test",
            setting=setting,
        )
    env.create_speaker_labels(
        "20240101",
        "091000_300",
        [],
        metadata=_candidate_evidence(
            ("alice_co", ["screen"]),
            ("bob_co", ["meeting_day"]),
            ("carol_mention", ["setting"]),
            ("dave_speaker", ["speakers"]),
        ),
    )
    env.create_speaker_labels(
        "20240101",
        "091500_300",
        [],
        metadata=_candidate_evidence(
            ("alice_co", ["screen"]),
            ("bob_co", ["screen", "meeting_day"]),
            ("carol_mention", ["setting"]),
            ("dave_speaker", ["speakers"]),
        ),
    )
    env.create_speaker_labels(
        "20240101",
        "092000_300",
        [],
        metadata=_candidate_evidence(
            ("alice_co", ["meeting_day"]),
            ("bob_co", ["screen", "meeting_day"]),
            ("carol_mention", ["speakers"]),
        ),
    )
    _write_discovery_cache(
        env,
        7,
        [
            _cluster_record(day, segment_key, source="imported_audio")
            for day, segment_key, _setting in segments
        ],
    )

    presence = get_cluster_presence(7)

    assert presence is not None
    facts_without_samples = {
        key: value for key, value in presence["facts"].items() if key != "samples"
    }
    assert facts_without_samples == {
        "statement_count": 3,
        "segment_count": 3,
        "day_count": 1,
        "streams": ["test"],
        "conversation_count": 2,
    }
    assert [sample["setting"] for sample in presence["facts"]["samples"]] == [
        "Room A",
        "Room A",
        "Room B",
    ]
    assert presence["evidence_complete"] is True
    assert presence["evidence_gaps"] == []
    assert presence["candidates"]["co_presence"] == [
        {
            "entity_id": "bob_co",
            "name": "Bob Co",
            "has_voice": False,
            "screen_conversations": 2,
            "meeting_days": 1,
            "setting_conversations": 0,
            "speaker_conversations": 0,
        },
        {
            "entity_id": "alice_co",
            "name": "Alice Co",
            "has_voice": True,
            "screen_conversations": 1,
            "meeting_days": 1,
            "setting_conversations": 0,
            "speaker_conversations": 0,
        },
    ]
    assert presence["candidates"]["mention"] == [
        {
            "entity_id": "carol_mention",
            "name": "Carol Mention",
            "has_voice": False,
            "screen_conversations": 0,
            "meeting_days": 0,
            "setting_conversations": 1,
            "speaker_conversations": 1,
        },
        {
            "entity_id": "dave_speaker",
            "name": "Dave Speaker",
            "has_voice": False,
            "screen_conversations": 0,
            "meeting_days": 0,
            "setting_conversations": 0,
            "speaker_conversations": 1,
        },
    ]


def test_cluster_presence_conversation_grouping_setting_vs_no_setting(speakers_env):
    env = speakers_env()
    for segment_key in ("093000_300", "093500_300"):
        env.create_import_segment(
            "20240101",
            segment_key,
            [("", "Unknown voice.")],
            stream="test",
            setting="Shared Room",
        )
        env.create_speaker_labels(
            "20240101",
            segment_key,
            [],
            metadata=_candidate_evidence(),
        )
    shared_setting_records = [
        _cluster_record("20240101", "093000_300", source="imported_audio"),
        _cluster_record("20240101", "093500_300", source="imported_audio"),
    ]
    _write_discovery_cache(
        env,
        8,
        shared_setting_records,
    )

    for segment_key in ("094000_300", "094500_300"):
        env.create_segment("20240101", segment_key, ["audio"])
        env.create_speaker_labels(
            "20240101",
            segment_key,
            [],
            metadata=_candidate_evidence(),
        )
    no_setting_records = [
        _cluster_record("20240101", "094000_300"),
        _cluster_record("20240101", "094500_300"),
    ]
    _write_discovery_cache(
        env,
        9,
        no_setting_records,
    )

    shared_setting = get_cluster_presence(8)
    no_setting = get_cluster_presence(9)

    assert shared_setting is not None
    assert no_setting is not None
    assert get_cluster_conversation_count(shared_setting_records) == 1
    assert get_cluster_conversation_count(no_setting_records) == 2
    assert shared_setting["facts"]["conversation_count"] == 1
    assert no_setting["facts"]["conversation_count"] == 2


def test_cluster_presence_readonly_fallback_uses_legacy_sources_without_writes(
    speakers_env,
):
    env = speakers_env()
    env.create_entity("Alice Test")
    env.create_entity("Bob Test")
    env.create_entity("Carol Test")
    env.create_import_segment(
        "20240101",
        "100000_300",
        [("", "Unknown voice.")],
        stream="test",
        setting="Meeting with Alice Test",
    )
    embedding_path = (
        env.journal
        / "chronicle"
        / "20240101"
        / "test"
        / "100000_300"
        / "imported_audio.npz"
    )
    embedding_path.unlink()
    env.create_screen_json("20240101", "100000_300", ["Bob Test"], stream="test")
    env.create_speakers_json("20240101", "100000_300", ["Carol Test"])
    env.create_speaker_labels(
        "20240101",
        "100000_300",
        [],
        metadata={"owner_centroid_last_refreshed_at": None, "voiceprint_versions": {}},
    )
    _write_discovery_cache(
        env,
        10,
        [_cluster_record("20240101", "100000_300", source="imported_audio")],
    )
    before = _domain_tree_hash(env.journal)

    presence = get_cluster_presence(10)

    assert _domain_tree_hash(env.journal) == before
    assert presence is not None
    assert presence["evidence_complete"] is True
    assert {cand["entity_id"] for cand in presence["candidates"]["co_presence"]} == {
        "bob_test"
    }
    assert {cand["entity_id"] for cand in presence["candidates"]["mention"]} == {
        "alice_test",
        "carol_test",
    }


def test_cluster_presence_legacy_fallback_reports_speakers_gap_without_writes(
    speakers_env,
):
    env = speakers_env()
    env.create_entity("Alice Test")
    env.create_import_segment(
        "20240101",
        "100500_300",
        [("", "Unknown voice.")],
        stream="test",
        setting="Meeting with Alice Test",
    )
    env.create_speakers_json(
        "20240101",
        "100500_300",
        [],
        raw=json.dumps([5]),
    )
    env.create_speaker_labels(
        "20240101",
        "100500_300",
        [],
        metadata={"owner_centroid_last_refreshed_at": None, "voiceprint_versions": {}},
    )
    _write_discovery_cache(
        env,
        14,
        [_cluster_record("20240101", "100500_300", source="imported_audio")],
    )
    before = _domain_tree_hash(env.journal)

    presence = get_cluster_presence(14)

    assert _domain_tree_hash(env.journal) == before
    assert presence is not None
    assert presence["evidence_complete"] is False
    assert presence["evidence_gaps"] == [
        {
            "day": "20240101",
            "stream": "test",
            "segment_key": "100500_300",
            "source": "speakers",
            "reason": "wrong_shape",
        }
    ]
    assert presence["candidates"]["mention"] == [
        {
            "entity_id": "alice_test",
            "name": "Alice Test",
            "has_voice": False,
            "screen_conversations": 0,
            "meeting_days": 0,
            "setting_conversations": 1,
            "speaker_conversations": 0,
        }
    ]


def test_cluster_presence_stale_resolution_gap_keeps_siblings(speakers_env):
    from solstone.think.entities import (
        ResolutionOrigin,
        ResolutionScope,
        load_all_journal_entities,
        record_ambiguity_choice,
        record_entity_resolution,
    )

    env = speakers_env()
    env.create_entity("Alice Test")
    env.create_entity("Sarah Connor")
    env.create_entity("Sarah Lee")
    entities = list(load_all_journal_entities().values())
    scope = ResolutionScope.journal()
    origin = ResolutionOrigin(lane="test", field="candidate_name")
    record_entity_resolution("Sarah", entities, scope=scope, origin=origin)
    record_ambiguity_choice("Sarah", "sarah_connor", entities, scope=scope)
    sarah_path = env.journal / "entities" / "sarah_connor" / "entity.json"
    sarah = json.loads(sarah_path.read_text(encoding="utf-8"))
    sarah["blocked"] = True
    sarah_path.write_text(json.dumps(sarah), encoding="utf-8")

    env.create_import_segment(
        "20240101",
        "101000_300",
        [("", "Unknown voice.")],
        stream="test",
        setting="Meeting with Alice Test",
    )
    env.create_screen_json("20240101", "101000_300", ["Sarah"], stream="test")
    _write_discovery_cache(
        env,
        11,
        [_cluster_record("20240101", "101000_300", source="imported_audio")],
    )

    presence = get_cluster_presence(11)

    assert presence is not None
    assert presence["evidence_complete"] is False
    assert presence["evidence_gaps"] == [
        {
            "day": "20240101",
            "stream": "test",
            "segment_key": "101000_300",
            "source": "resolution",
            "reason": "stale_resolution",
        }
    ]
    assert presence["candidates"]["mention"] == [
        {
            "entity_id": "alice_test",
            "name": "Alice Test",
            "has_voice": False,
            "screen_conversations": 0,
            "meeting_days": 0,
            "setting_conversations": 1,
            "speaker_conversations": 0,
        }
    ]


def test_cluster_presence_excludes_principal_blocked_and_missing_entities(
    speakers_env,
):
    env = speakers_env()
    env.create_entity("Owner Test", is_principal=True)
    env.create_entity("Alice Test")
    blocked_dir = env.create_entity("Blocked Test")
    blocked_path = blocked_dir / "entity.json"
    blocked = json.loads(blocked_path.read_text(encoding="utf-8"))
    blocked["blocked"] = True
    blocked_path.write_text(json.dumps(blocked), encoding="utf-8")
    env.create_segment("20240101", "102000_300", ["audio"])
    env.create_speaker_labels(
        "20240101",
        "102000_300",
        [],
        metadata=_candidate_evidence(
            ("owner_test", ["screen"]),
            ("alice_test", ["screen"]),
            ("blocked_test", ["screen"]),
            ("missing_test", ["screen"]),
        ),
    )
    _write_discovery_cache(env, 12, [_cluster_record("20240101", "102000_300")])

    presence = get_cluster_presence(12)

    assert presence is not None
    assert presence["candidates"]["co_presence"] == [
        {
            "entity_id": "alice_test",
            "name": "Alice Test",
            "has_voice": False,
            "screen_conversations": 1,
            "meeting_days": 0,
            "setting_conversations": 0,
            "speaker_conversations": 0,
        }
    ]
    assert presence["candidates"]["mention"] == []


def test_cluster_presence_empty_evidence_and_unknown_cluster(speakers_env):
    env = speakers_env()
    env.create_segment("20240101", "103000_300", ["audio"])
    env.create_speaker_labels(
        "20240101",
        "103000_300",
        [],
        metadata=_candidate_evidence(),
    )
    _write_discovery_cache(env, 13, [_cluster_record("20240101", "103000_300")])

    presence = get_cluster_presence(13)

    assert presence is not None
    assert presence["facts"]["statement_count"] == 1
    assert presence["candidates"] == {"co_presence": [], "mention": []}
    assert get_cluster_presence(999) is None


def test_identify_cluster_forwards_native_request() -> None:
    from unittest.mock import patch

    from solstone.think.utils import get_journal

    expected = {"status": "identified", "operation_id": "idop_test"}
    with patch.object(
        discovery_module.native_speakers, "identify", return_value=expected
    ) as identify:
        result = identify_cluster(
            12,
            name="Alice Test",
            entity_id="alice_test",
            create_new=True,
            request_id="request-12",
            reviewed_near_match_entity_ids=["bob_test"],
        )

    assert result is expected
    args, kwargs = identify.call_args
    assert args == (get_journal(),)
    assert kwargs["cluster_id"] == 12
    assert kwargs["name"] == "Alice Test"
    assert kwargs["entity_id"] == "alice_test"
    assert kwargs["create_new"] is True
    assert kwargs["request_id"] == "request-12"
    assert kwargs["reviewed_near_match_entity_ids"] == ["bob_test"]
    assert kwargs["encoder"] == discovery_module._speaker_encoder_identity()


def test_undo_identify_operation_forwards_native_request() -> None:
    from unittest.mock import patch

    from solstone.think.utils import get_journal

    expected = {"status": "undone", "operation_id": "idop_test"}
    with patch.object(
        discovery_module.native_speakers, "undo_identify", return_value=expected
    ) as undo:
        result = undo_identify_operation("idop_test")

    assert result is expected
    undo.assert_called_once_with(
        get_journal(),
        operation_id="idop_test",
        encoder=discovery_module._speaker_encoder_identity(),
    )
