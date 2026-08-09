#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Build generated fixtures consumed by the Rust core workspace."""

from __future__ import annotations

import argparse
import base64
import copy
import hashlib
import importlib.metadata
import json
import logging
import math
import platform
import sqlite3
import sys
from collections.abc import Callable
from dataclasses import dataclass
from pathlib import Path
from typing import Any

import numpy as np

ROOT = Path(__file__).resolve().parent.parent
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))

from solstone.apps.speakers.encoder_config import (
    ACOUSTIC_HIGH,
    ACOUSTIC_MARGIN_MIN,
    ACOUSTIC_MEDIUM,
    CC_CONFIDENCE_GATE,
    CC_COVERAGE_GATE,
    CONFIRM_MIN_DURATION_S,
    CONFIRM_MIN_INTERVALS,
    CONFIRM_MIN_SEGMENTS,
    CONSOLIDATE_MERGE_THRESHOLD,
    CONSOLIDATE_MIN_INTERVALS,
    CONSOLIDATE_SUGGEST_MIN,
    DIARIZE_MIN_OVERLAP,
    ENCODER_ID,
    MERGE_THRESHOLD,
    NOISY_FLYWHEEL_OVERLAP_MAX,
    OVERLAP_DETECTOR_ID,
    OVERLAP_DETECTOR_SHA256,
    OWNER_BOOTSTRAP_EVIDENCE_TIER_STANDARD,
    OWNER_BOOTSTRAP_EVIDENCE_TIER_STRONG,
    OWNER_BOOTSTRAP_MIN_INTRA_COSINE_P25,
    OWNER_BOOTSTRAP_MIN_INTRA_COSINE_P25_STRONG,
    OWNER_BOOTSTRAP_MIN_MEDIAN_DURATION_S,
    OWNER_BOOTSTRAP_MIN_STMTS,
    OWNER_BOOTSTRAP_PROVISIONAL_GUARD_MIN_TAGS,
    OWNER_BOOTSTRAP_STRONG_EVIDENCE_MIN_STMTS,
    OWNER_MARGIN_MIN,
    OWNER_REBUILD_MAX_COHESION_DROP,
    OWNER_REBUILD_MIN_CENTROID_AGREEMENT,
    OWNER_REBUILD_MIN_CLUSTER_SIZE_RATIO,
    OWNER_REBUILD_SUPERSEDED_SCAN_DAYS,
    OWNER_THRESHOLD,
    SLOT_ACTIVE_MIN_SHARE,
    SOLO_CLUSTER_MIN_COSINE,
    SPEAKER_EVIDENCE_MULTI_MIN,
    SPEAKER_EVIDENCE_SINGLE_MAX,
    SPEAKER_EVIDENCE_VERSION,
    SPLIT_THRESHOLD,
    STABILITY_THRESHOLD,
    VP_DECAY_LAMBDA,
    VP_OUTLIER_MIN_SAMPLES,
    VP_OUTLIER_MIN_SIMILARITY,
)
from solstone.convey.contract.assemble import CALLOSUM_REGISTRY
from solstone.convey.provider_readiness import (
    is_blocking_reason,
    mapped_reason_codes,
)
from solstone.think import markdown as markdown_formatter
from solstone.think.cogitate_contract import (
    COGITATE_ACCESS_TIERS,
    COGITATE_DIAGNOSTIC_PREAMBLE,
    COGITATE_READ_TOOL_NAMES,
    COGITATE_RUNTIME_PREAMBLE,
    FUTURE_ACCESS_TIERS,
    TALENT_FINALIZATION_MODES,
    capabilities_for_access_tier,
)
from solstone.think.indexer.edges import EDGES_SCHEMA_VERSION, _ensure_edges_schema
from solstone.think.providers.shared import (
    _UNKNOWN_FINISH_REASON,
    is_non_retryable_generate_reason,
)
from tests.speaker_oracle.diarize import (
    AHC_LINKAGE,
    AHC_METRIC,
    FRAMES_PER_WINDOW,
    MAX_K,
    MIN_FRAME_CONFIDENCE,
    MIN_INTERVAL_S,
    SILHOUETTE_IMPROVEMENT,
    SINGLE_SPEAKER_CLASSES,
    _ahc,
    _assign_sentences,
    _find_intervals,
    _normalize_rows,
    _pick_k_silhouette,
    _silhouette,
    _wespeaker_features,
)
from tests.speaker_oracle.diarize import (
    SAMPLE_RATE as DIARIZE_SAMPLE_RATE,
)
from tests.speaker_oracle.diarize import (
    STRIDE_S as DIARIZE_STRIDE_S,
)
from tests.speaker_oracle.diarize import (
    WINDOW_S as DIARIZE_WINDOW_S,
)
from tests.speaker_oracle.embedder import _compute_wespeaker_features
from tests.speaker_oracle.overlap import (
    _DIARIZE_STRIDE_S as OVERLAP_DIARIZE_STRIDE_S,
)
from tests.speaker_oracle.overlap import (
    FRAMES_PER_WINDOW as OVERLAP_FRAMES_PER_WINDOW,
)
from tests.speaker_oracle.overlap import (
    OVERLAP_CLASSES,
    _speaker_window_stats,
    decide_speaker_evidence,
)
from tests.speaker_oracle.overlap import (
    STRIDE_S as OVERLAP_STRIDE_S,
)
from tests.speaker_oracle.overlap import (
    WINDOW_S as OVERLAP_WINDOW_S,
)

sys.path.insert(0, str(Path(__file__).resolve().parent))
import content_family_corpus  # noqa: E402  — sibling module, not a package
import entity_corpus  # noqa: E402  — sibling module, not a package
import install_status_corpus  # noqa: E402  — sibling module, not a package
import talent_projection_corpus  # noqa: E402  — sibling module, not a package

FIXTURE_DIR = ROOT / "core" / "fixtures"
CONTENT_FAMILIES_ARTIFACT_PATH = FIXTURE_DIR / "content_families.json"
TALENT_PROJECTIONS_ARTIFACT_PATH = FIXTURE_DIR / "talent_projections.json"
CALLOSUM_ARTIFACT_PATH = FIXTURE_DIR / "callosum_registry.json"
COGITATE_ARTIFACT_PATH = FIXTURE_DIR / "cogitate_contract.json"
GENERATE_ARTIFACT_PATH = FIXTURE_DIR / "generate_contract.json"

_IMAGE_MIME_TYPES = frozenset({"image/png", "image/jpeg", "image/gif", "image/webp"})
_SESSION_LINE_LIMIT = 64 * 1024 * 1024
INSTALL_STATUS_ARTIFACT_PATH = FIXTURE_DIR / "install_status.json"
EDGE_SCHEMA_ARTIFACT_PATH = FIXTURE_DIR / "edge_schema.json"
MARKDOWN_CHUNKS_ARTIFACT_PATH = FIXTURE_DIR / "markdown_chunks.json"
SPEAKER_FILTERBANK_ARTIFACT_PATH = FIXTURE_DIR / "speaker_filterbank.json"
SPEAKER_STAGE_BOUNDARIES_ARTIFACT_PATH = FIXTURE_DIR / "speaker_stage_boundaries.json"
ENTITY_IDENTITY_ARTIFACT_PATH = FIXTURE_DIR / "entity_identity.json"
ENTITY_MATCHING_ARTIFACT_PATH = FIXTURE_DIR / "entity_matching.json"
ENTITY_RESOLUTION_MAP_DIVERGENCES_ARTIFACT_PATH = (
    FIXTURE_DIR / "entity_resolution_map_divergences.json"
)
ENTITY_STORE_ARTIFACT_PATH = FIXTURE_DIR / "entity_store.json"
ENTITY_LIFECYCLE_ARTIFACT_PATH = FIXTURE_DIR / "entity_lifecycle.json"
VOICEPRINT_OPERATIONS_ARTIFACT_PATH = FIXTURE_DIR / "voiceprint_operations.json"
OVERSIZED_SIZE_NORMALIZATION = "oversized_size"
OVERSIZED_SIZE_TOKEN = "normalizedsize"
# Filterbank rows are a scale/regime oracle for the production fbank stage. The
# tolerance leaves headroom under SILHOUETTE_IMPROVEMENT and the
# speaker-evidence thresholds while allowing native-library/architecture float noise.
FILTERBANK_VALUE_ABS_TOLERANCE = 1e-2
# Silhouette scores are compared with enough room for plausible float32
# matmul/BLAS reduction noise across architectures, but far below the
# SILHOUETTE_IMPROVEMENT decision margin. Selected k values and cluster
# labels are compared exactly, so score tolerance cannot hide a branch flip. If
# the arm64 leg proves 1e-6 too tight, that drift is a finding to report.
CLUSTER_SCORE_ABS_TOLERANCE = 1e-6
# generation_platform is diagnostic provenance. It is ignored so an arm64 check
# passes iff kaldi/source identity and floats agree.
SPEAKER_FIXTURE_IGNORED_JSON_POINTERS = ("/identity/generation_platform",)
FLOAT_ROW_DECIMALS = 3
SPEAKER_FILTERBANK_WAVEFORM_SEED = 20_260_724
SPEAKER_FILTERBANK_DURATION_S = 2.0
SPEAKER_FILTERBANK_NEAR_S = 0.60
SPEAKER_FILTERBANK_FADE_S = 0.20
SPEAKER_FILTERBANK_NEAR_AMPLITUDE = 1e-5
SPEAKER_FILTERBANK_BROADBAND_AMPLITUDE = 3e-2
SPEAKER_FILTERBANK_NEAR_ROWS = (5, 55)
SPEAKER_FILTERBANK_BROADBAND_ROWS = (100, 190)
K_DIVERGENCE_N = 32
K_DIVERGENCE_SEED = 0
# Found in prep by bounded seeded search over default_rng(seed).standard_normal.
CLUSTER_PERTURB_N = 12
CLUSTER_PERTURB_SEED = 15
CLUSTER_PERTURB_ROW = 6
CLUSTER_PERTURB_COL = 40
CLUSTER_PERTURB_EPSILON = 0.03


@dataclass(frozen=True)
class ArtifactDescriptor:
    build: Callable[[], Any]
    comparison: str = "exact"

    def render(self) -> str:
        value = self.build()
        if isinstance(value, str):
            return value
        return render_json(value)


def _package_version(name: str) -> str:
    try:
        return importlib.metadata.version(name)
    except importlib.metadata.PackageNotFoundError:
        return "not-installed"


def build_callosum_registry_fixture() -> dict[str, Any]:
    return {
        "fixture": "solstone-callosum-registry",
        "fixture_version": 1,
        "generated_by": "make core-fixtures",
        "registry": {
            tract: list(CALLOSUM_REGISTRY[tract]) for tract in sorted(CALLOSUM_REGISTRY)
        },
    }


def build_cogitate_contract_fixture() -> dict[str, Any]:
    runtime_preamble_bytes = COGITATE_RUNTIME_PREAMBLE.encode("utf-8")
    diagnostic_preamble_bytes = COGITATE_DIAGNOSTIC_PREAMBLE.encode("utf-8")
    return {
        "fixture": "solstone-cogitate-contract",
        "fixture_version": 1,
        "generated_by": "make core-fixtures",
        "access_tiers": list(COGITATE_ACCESS_TIERS),
        "capabilities": {
            tier: {
                "sol": capabilities_for_access_tier(tier).sol,
                "reads": capabilities_for_access_tier(tier).reads,
                "submit": capabilities_for_access_tier(tier).submit,
            }
            for tier in COGITATE_ACCESS_TIERS
        },
        "future_access_tiers": list(FUTURE_ACCESS_TIERS),
        "read_tools": list(COGITATE_READ_TOOL_NAMES),
        "finalization_modes": list(TALENT_FINALIZATION_MODES),
        "runtime_preamble": {
            "digest": hashlib.sha256(runtime_preamble_bytes).hexdigest(),
            "algorithm": "sha256",
            "encoding": "utf-8",
            "byte_length": len(runtime_preamble_bytes),
            "text": COGITATE_RUNTIME_PREAMBLE,
        },
        "diagnostic_preamble": {
            "digest": hashlib.sha256(diagnostic_preamble_bytes).hexdigest(),
            "algorithm": "sha256",
            "encoding": "utf-8",
            "byte_length": len(diagnostic_preamble_bytes),
            "text": COGITATE_DIAGNOSTIC_PREAMBLE,
        },
    }


def build_generate_contract_fixture() -> dict[str, Any]:
    """Build the cross-language generate boundary contract."""

    schemas = {
        "request": "solstone-generate-request-v2",
        "response": "solstone-generate-response-v2",
        "error": "solstone-generate-error-v2",
        "session_terminal": "solstone-generate-session-terminal-v2",
    }
    attestation_codes = {
        "attestation_not_yet_verified",
        "attestation_failed",
        "attestation_stale",
    }
    live_codes = mapped_reason_codes()
    reason_codes = [
        {
            "code": code,
            "blocking": True if code in attestation_codes else is_blocking_reason(code),
            "retryable": not is_non_retryable_generate_reason(code),
            "overrides_live_taxonomy": code in attestation_codes,
        }
        for code in sorted(live_codes | attestation_codes)
    ]
    request = {
        "schema": schemas["request"],
        "id": "fixture-request",
        "context": "fixture.generate",
        "contents": [{"type": "text", "text": "Reply with OK."}],
        "system_instruction": None,
        "temperature": 0.3,
        "max_output_tokens": 16384,
        "thinking_budget": None,
        "timeout_s": None,
        "json_output": False,
        "json_schema": None,
        "enforce_responsiveness": True,
        "attempt_index": 0,
        "exclusive_admission": False,
        "transport_retries": None,
    }
    generated = {
        "schema": schemas["response"],
        "id": request["id"],
        "outcome": "generated",
        "text": "OK",
        "model": "fixture-model",
        "usage": {"input_tokens": 2, "output_tokens": 1, "total_tokens": 3},
        "finish_reason": "stop",
        "thinking": None,
        "schema_validation": None,
        "input_budget": None,
        "request_budget": None,
        "inference": None,
    }
    typed_refusals = [
        (
            "attestation-not-verified",
            "AttestationNotVerifiedError",
            "attestation_not_yet_verified",
        ),
        ("attestation-failed", "AttestationFailedError", "attestation_failed"),
        ("attestation-stale", "AttestationStaleError", "attestation_stale"),
        (
            "no-engine-configured",
            "NoBrainConfiguredError",
            "thinking_engine_not_chosen",
        ),
        ("incomplete-json", "IncompleteJSONError", "incomplete_json_length"),
        ("incomplete-text", "IncompleteTextError", "incomplete_text_length"),
        (
            "provider-response-invalid",
            "ProviderResponseInvalidError",
            "provider_response_invalid",
        ),
        ("schema-validation-failed", "SchemaValidationError", None),
        ("non-responsive-output", "NonResponsiveOutputError", "non_responsive"),
    ]
    by_code = {entry["code"]: entry for entry in reason_codes}
    vectors: list[dict[str, Any]] = [
        {
            "id": "generated",
            "framing": "one_shot",
            "request": request,
            "response": generated,
            "exit_code": 0,
            "source": {"path": "generated"},
        }
    ]
    for reason, exception, reason_code in typed_refusals:
        classification = (
            by_code[reason_code]
            if reason_code is not None
            else {"blocking": True, "retryable": False}
        )
        vectors.append(
            {
                "id": f"refused-{reason}",
                "framing": "one_shot",
                "request": request,
                "response": {
                    "schema": schemas["response"],
                    "id": request["id"],
                    "outcome": "refused",
                    "reason": reason,
                    "reason_code": reason_code,
                    "retryable": classification["retryable"],
                    "blocking": classification["blocking"],
                    "reset_at_ms": None,
                    "provider": "none" if reason == "no-engine-configured" else "local",
                    "detail": f"fixture {reason}",
                },
                "exit_code": 0,
                "source": {
                    "path": "typed_exception",
                    "exception": exception,
                    "reason_code": reason_code,
                },
            }
        )
    vectors.extend(
        [
            {
                "id": "malformed-request",
                "framing": "protocol_error",
                "stdin": "{",
                "protocol_error": {
                    "schema": schemas["error"],
                    "id": None,
                    "reason": "malformed-request",
                    "detail": "stdin is not valid JSON",
                },
                "exit_code": 64,
                "source": {"path": "malformed_request"},
            },
            {
                "id": "unknown-refusal-reason",
                "framing": "one_shot",
                "response": {
                    "schema": schemas["response"],
                    "id": request["id"],
                    "outcome": "refused",
                    "reason": "future-reason",
                    "reason_code": None,
                    "retryable": False,
                    "blocking": True,
                    "reset_at_ms": None,
                    "provider": None,
                    "detail": "future",
                },
                "exit_code": 0,
                "source": {"path": "reader_tolerance"},
            },
            {
                "id": "unknown-reason-code",
                "framing": "one_shot",
                "response": {
                    "schema": schemas["response"],
                    "id": request["id"],
                    "outcome": "refused",
                    "reason": "provider-response-invalid",
                    "reason_code": "future_code",
                    "retryable": False,
                    "blocking": True,
                    "reset_at_ms": None,
                    "provider": None,
                    "detail": "future",
                },
                "exit_code": 0,
                "source": {"path": "reader_tolerance"},
            },
        ]
    )
    return {
        "fixture": "solstone-generate-contract",
        "fixture_version": 5,
        "generated_by": "make core-fixtures",
        "schema_identifiers": schemas,
        "request": {
            "fields": list(request),
            "required_fields": ["schema", "context", "contents"],
            "one_shot_optional_fields": ["id"],
            "session_required_fields": ["id"],
            "forbidden_fields": ["provider", "model"],
            "defaults": {
                key: value
                for key, value in request.items()
                if key not in {"schema", "id", "context", "contents"}
            },
            "content_parts": {
                "text": {"fields": ["type", "text"]},
                "image": {
                    "fields": ["type", "mime_type", "data"],
                    "mime_types": sorted(_IMAGE_MIME_TYPES),
                },
            },
        },
        "response": {
            "outcome_field": "outcome",
            # shared._UNKNOWN_FINISH_REASON: the value a generated response
            # carries when the provider reported no usable finish reason.
            # Distinct from the `unknown` REFUSAL reason despite the collision.
            "finish_reason_unknown": _UNKNOWN_FINISH_REASON,
            "outcomes": {
                "generated": {"fields": [*generated, "hints_applied"]},
                "refused": {
                    "fields": [
                        "schema",
                        "id",
                        "outcome",
                        "reason",
                        "reason_code",
                        "retryable",
                        "blocking",
                        "reset_at_ms",
                        "provider",
                        "detail",
                    ]
                },
            },
        },
        # Mirrors the native generate protocol-error response and
        # :_v2_internal_error, whose literals are not named constants.
        "protocol_error": {
            "fields": ["schema", "id", "reason", "detail"],
            "reasons": ["malformed-request", "internal-failure"],
        },
        "outcomes": ["generated", "refused"],
        "refusal_reasons": [reason for reason, _, _ in typed_refusals] + ["unknown"],
        "unknown_member": {
            "refusal_reason": "unknown",
            "retryable": False,
            "blocking": True,
        },
        "reason_codes": reason_codes,
        "exit_codes": {"response": 0, "malformed_request": 64, "internal_failure": 70},
        "framing": {
            "one_shot": {
                "selector": "--one-shot",
                "stdin": "json-eof",
                "id": "optional",
            },
            "session": {
                "selector": "--session",
                "stdin": "ndjson",
                "id": "required",
                "line_limit_bytes": _SESSION_LINE_LIMIT,
                "concurrency": {"flag": "--max-in-flight", "minimum": 1},
                "terminal": {
                    "schema": schemas["session_terminal"],
                    "fields": ["schema"],
                },
            },
        },
        "conformance_vectors": vectors,
    }


def build_edge_schema_fixture() -> dict[str, Any]:
    conn = sqlite3.connect(":memory:")

    def table_schema(table: str) -> dict[str, Any]:
        columns = [
            {"name": row[1], "type": row[2], "notnull": row[3], "pk": row[5]}
            for row in conn.execute(f"PRAGMA table_info({table})")
        ]
        indexes = []
        for row in conn.execute(f"PRAGMA index_list({table})"):
            index_name = row[1]
            indexes.append(
                {
                    "name": index_name,
                    "unique": row[2],
                    "origin": row[3],
                    "columns": [
                        column[2]
                        for column in conn.execute(f"PRAGMA index_info({index_name})")
                    ],
                }
            )
        indexes.sort(key=lambda index: index["name"])
        return {"columns": columns, "indexes": indexes}

    try:
        _ensure_edges_schema(conn)
        row = conn.execute("SELECT path, mtime FROM edge_files").fetchone()
        if row is None:
            raise RuntimeError("edge schema sentinel is missing")
        sentinel = {"path": row[0], "mtime": row[1]}
        return {
            "fixture": "solstone-edge-schema",
            "fixture_version": 1,
            "generated_by": "make core-fixtures",
            "schema_version": EDGES_SCHEMA_VERSION,
            "sentinel": sentinel,
            "tables": {
                "edge_files": table_schema("edge_files"),
                "edges": table_schema("edges"),
            },
        }
    finally:
        conn.close()


def render_peer_json(peer: dict[str, Any]) -> str:
    # Mirrors native link join peer.json formatting; keep this oracle in sync
    # because the Rust serializer is not imported by this Python fixture builder.
    return json.dumps(peer, indent=2) + "\n"


def build_link_join_observer_ascii_peer_json() -> str:
    return render_peer_json(
        {
            "label": "laptop",
            "paired_at": "1970-01-01T00:00:00Z",
            "instance_id": "receiver-instance",
            "home_label": "Home",
            "fingerprint": "sha256:abc",
            "local_endpoints": [],
            "role": "",
        }
    )


def build_link_join_peer_non_ascii_peer_json() -> str:
    return render_peer_json(
        {
            "label": "café",
            "paired_at": "1970-01-01T00:00:00Z",
            "instance_id": "receiver-instance",
            "home_label": "Hôme",
            "fingerprint": "sha256:abc",
            "local_endpoints": [
                {"endpoint": "réseau-local", "port": 7657, "scope": "lan"},
            ],
            "role": "peer",
        }
    )


def build_link_join_nested_endpoints_peer_json() -> str:
    return render_peer_json(
        {
            "label": "laptop",
            "paired_at": "1970-01-01T00:00:00Z",
            "instance_id": "receiver-instance",
            "home_label": "Home",
            "fingerprint": "sha256:abc",
            "local_endpoints": [
                {
                    "ip": "10.0.0.2",
                    "port": 7657,
                    "scope": "lan",
                    "meta": {
                        "first": "one",
                        "second": ["two", {"third": "three"}],
                    },
                },
            ],
            "role": "",
        }
    )


def link_join_fixture_dir() -> Path:
    return FIXTURE_DIR / "native-sol" / "link-join"


def _markdown_fixture_cases() -> list[dict[str, Any]]:
    long_line = "z" * (markdown_formatter._MAX_LINE_CHARS + 1)
    non_ascii_under_line_bound = "é" * (markdown_formatter._MAX_LINE_CHARS - 1)
    non_ascii_over_line_bound = "é" * (markdown_formatter._MAX_LINE_CHARS + 1)
    non_ascii_chunk_body = "\n".join(["é" * 1300] * 3)
    oversized_line = "alpha " * 300
    oversized_body = "\n".join([oversized_line] * 3)
    return [
        {"id": "empty", "input": ""},
        {"id": "whitespace_only", "input": " \n\t\n"},
        {"id": "heading_only", "input": "# Heading\n"},
        {"id": "thematic_break_only", "input": "---\n"},
        {
            "id": "header_only_table",
            "input": "| Name | Value |\n| --- | --- |\n",
        },
        {
            "id": "nested_heading_context",
            "input": "# Root\n\n## Child\n\nalpha paragraph\n\n### Leaf\n\nbeta paragraph\n",
        },
        {
            "id": "ordinary_paragraphs",
            "input": "# Notes\n\nalpha paragraph\n\nbeta paragraph\n",
        },
        {
            "id": "ordinary_list",
            "input": "# Tasks\n\n- alpha item\n- beta item\n",
        },
        {
            "id": "intro_list",
            "input": "# Tasks\n\nintro alpha\n\n- alpha item\n- beta item\n",
        },
        {
            "id": "intro_table",
            "input": (
                "# Metrics\n\nintro alpha\n\n"
                "| Name | Value |\n| --- | --- |\n| alpha | one |\n| beta | two |\n"
            ),
        },
        {
            "id": "definition_2_of_4",
            "input": (
                "# Definitions\n\n"
                "- **alpha:** value one\n"
                "- ordinary note.\n"
                "- **beta:** value two\n"
                "- ordinary other.\n"
            ),
        },
        {
            "id": "definition_2_of_5",
            "input": (
                "# Boundary\n\n"
                "- **alpha:** value one\n"
                "- ordinary note.\n"
                "- **beta:** value two\n"
                "- ordinary other.\n"
                "- ordinary final.\n"
            ),
        },
        {
            "id": "definition_1_of_2",
            "input": "# Boundary\n\n- **alpha:** value one\n- ordinary note.\n",
        },
        {
            "id": "multi_row_table",
            "input": (
                "# Matrix\n\n"
                "| Name | Value |\n| --- | --- |\n"
                "| alpha | one |\n| beta | two |\n| gamma | three |\n"
            ),
        },
        {
            "id": "fenced_code_info",
            "input": "# Code\n\n```python\nprint('alpha')\n```\n",
        },
        {
            "id": "blockquote_multi_paragraph",
            "input": "# Quote\n\n> alpha quote\n>\n> beta quote\n",
        },
        {
            "id": "overlong_line",
            "input": f"# Long\n\n{long_line}\n\nkept alpha\n",
        },
        {
            "id": "oversized_chunk",
            "input": f"# Big\n\n{oversized_body}\n",
        },
        {
            "id": "loose_nested_list",
            "input": "# Nested\n\n- parent alpha\n\n  - child beta\n",
        },
        {
            "id": "two_paragraph_list_item",
            "input": "# Loose\n\n- first alpha\n\n  second beta\n",
        },
        {
            "id": "list_item_fenced_code",
            "input": "# Item Code\n\n- alpha before\n\n  ```python\n  print('beta')\n  ```\n",
        },
        {
            "id": "inline_link",
            "input": "# Link\n\n[alpha](https://example.com/path/to-beta?q=gamma)\n",
        },
        {
            "id": "inline_image",
            "input": '# Image\n\n![alt text](images/pic-alpha.png "title beta")\n',
        },
        {
            "id": "autolink",
            "input": "# Auto\n\n<https://example.com/path?q=gamma>\n",
        },
        {
            "id": "reference_link",
            "input": '# Reference\n\n[alpha][ref]\n\n[ref]: https://example.com/path "title beta"\n',
        },
        {
            "id": "inline_html",
            "input": "# Html\n\nalpha <span>beta</span> gamma\n",
        },
        {
            "id": "non_ascii_line_under_char_bound_over_byte_bound",
            "input": f"# Accent\n\n{non_ascii_under_line_bound}\n",
            "token_comparison": False,
            "token_comparison_reason": (
                "non-ASCII is outside the ASCII tokenizer-equivalence guarantee; "
                "compare only chunk count and warnings"
            ),
        },
        {
            "id": "non_ascii_line_over_char_bound",
            "input": f"# Accent\n\n{non_ascii_over_line_bound}\n\nkept ascii\n",
            "token_comparison": False,
            "token_comparison_reason": (
                "non-ASCII is outside the ASCII tokenizer-equivalence guarantee; "
                "compare only chunk count and warnings"
            ),
        },
        {
            "id": "non_ascii_chunk_under_char_bound_over_byte_bound",
            "input": f"# Accent\n\n{non_ascii_chunk_body}\n",
            "token_comparison": False,
            "token_comparison_reason": (
                "non-ASCII is outside the ASCII tokenizer-equivalence guarantee; "
                "compare only chunk count and warnings"
            ),
        },
    ]


class _WarningCapture(logging.Handler):
    def __init__(self) -> None:
        super().__init__(level=logging.WARNING)
        self.messages: list[str] = []

    def emit(self, record: logging.LogRecord) -> None:
        self.messages.append(record.getMessage())


def _format_markdown_with_warnings(text: str) -> tuple[list[dict[str, Any]], list[str]]:
    logger = logging.getLogger(markdown_formatter.__name__)
    handler = _WarningCapture()
    logger.addHandler(handler)
    try:
        chunks, _meta = markdown_formatter.format_markdown(text)
    finally:
        logger.removeHandler(handler)
    return chunks, handler.messages


def _fts5_tokens(chunks: list[str]) -> list[list[str]]:
    tokens: list[list[str]] = [[] for _chunk in chunks]
    conn = sqlite3.connect(":memory:")
    try:
        conn.execute("CREATE VIRTUAL TABLE chunks USING fts5(content)")
        conn.executemany(
            "INSERT INTO chunks(content) VALUES (?)",
            [(chunk,) for chunk in chunks],
        )
        conn.execute("CREATE VIRTUAL TABLE vocab USING fts5vocab(chunks, 'instance')")
        rows = conn.execute(
            "SELECT doc, offset, term FROM vocab ORDER BY doc, offset"
        ).fetchall()
    finally:
        conn.close()

    for doc, _offset, term in rows:
        tokens[int(doc) - 1].append(str(term))
    return tokens


def _normalize_oversized_size_tokens(tokens: list[str]) -> list[str]:
    normalized: list[str] = []
    i = 0
    while i < len(tokens):
        if i + 5 < len(tokens) and tokens[i : i + 5] == [
            "content",
            "too",
            "large",
            "to",
            "index",
        ]:
            normalized.extend(tokens[i : i + 5])
            j = i + 5
            while j < len(tokens) and tokens[j] != "chars":
                j += 1
            if j < len(tokens):
                normalized.append(OVERSIZED_SIZE_TOKEN)
                normalized.append("chars")
                i = j + 1
                continue
        normalized.append(tokens[i])
        i += 1
    return normalized


def _normalize_tokens(tokens: list[str], normalizations: list[str]) -> list[str]:
    if OVERSIZED_SIZE_NORMALIZATION in normalizations:
        tokens = _normalize_oversized_size_tokens(tokens)
    return tokens


def build_markdown_chunks_fixture() -> dict[str, Any]:
    cases = []
    for case in _markdown_fixture_cases():
        token_comparison = case.get("token_comparison", True)
        if token_comparison and not case["input"].isascii():
            raise RuntimeError(f"markdown fixture case is not ASCII-only: {case['id']}")
        chunks, warnings = _format_markdown_with_warnings(case["input"])
        entry = {
            "id": case["id"],
            "input": case["input"],
            "chunk_count": len(chunks),
            "warnings": warnings,
        }
        if token_comparison:
            rendered = [chunk["markdown"] for chunk in chunks]
            tokens_by_chunk = _fts5_tokens(rendered)
            chunk_entries = []
            for markdown, tokens in zip(rendered, tokens_by_chunk, strict=True):
                normalizations = (
                    [OVERSIZED_SIZE_NORMALIZATION]
                    if "[Content too large to index:" in markdown
                    else []
                )
                chunk_entry: dict[str, Any] = {
                    "markdown": markdown,
                    "tokens": _normalize_tokens(tokens, normalizations),
                }
                if normalizations:
                    chunk_entry["normalizations"] = normalizations
                chunk_entries.append(chunk_entry)
            entry["chunks"] = chunk_entries
        else:
            entry["token_comparison"] = False
            entry["token_comparison_reason"] = case["token_comparison_reason"]
        cases.append(entry)

    return {
        "fixture": "solstone-markdown-chunks",
        "fixture_version": 1,
        "generated_by": "make core-fixtures",
        "constraints": {
            "token_comparison_cases_ascii_only": True,
            "token_comparison_false_behavior": (
                "non-ASCII cases record only chunk_count and warnings because "
                "they are outside the ASCII tokenizer-equivalence guarantee"
            ),
            "max_line_chars": markdown_formatter._MAX_LINE_CHARS,
            "max_chunk_chars": markdown_formatter._MAX_CHUNK_CHARS,
            "normalizations": {
                OVERSIZED_SIZE_NORMALIZATION: (
                    "replace content-too-large size number tokens with "
                    f"{OVERSIZED_SIZE_TOKEN}"
                )
            },
            "tokenizer": (
                "sqlite fts5(content) with fts5vocab(chunks, 'instance') "
                "ordered by doc, offset"
            ),
        },
        "cases": cases,
    }


def _diarize_constants() -> dict[str, Any]:
    return {
        "SAMPLE_RATE": DIARIZE_SAMPLE_RATE,
        "WINDOW_S": DIARIZE_WINDOW_S,
        "STRIDE_S": DIARIZE_STRIDE_S,
        "FRAMES_PER_WINDOW": FRAMES_PER_WINDOW,
        "SINGLE_SPEAKER_CLASSES": sorted(SINGLE_SPEAKER_CLASSES),
        "MIN_INTERVAL_S": MIN_INTERVAL_S,
        "MIN_FRAME_CONFIDENCE": MIN_FRAME_CONFIDENCE,
        "AHC_LINKAGE": AHC_LINKAGE,
        "AHC_METRIC": AHC_METRIC,
        "MAX_K": MAX_K,
        "SILHOUETTE_IMPROVEMENT": SILHOUETTE_IMPROVEMENT,
    }


def _encoder_config_constants() -> dict[str, Any]:
    return {
        "ENCODER_ID": ENCODER_ID,
        "OWNER_THRESHOLD": OWNER_THRESHOLD,
        "OWNER_MARGIN_MIN": OWNER_MARGIN_MIN,
        "ACOUSTIC_HIGH": ACOUSTIC_HIGH,
        "ACOUSTIC_MEDIUM": ACOUSTIC_MEDIUM,
        "ACOUSTIC_MARGIN_MIN": ACOUSTIC_MARGIN_MIN,
        "SOLO_CLUSTER_MIN_COSINE": SOLO_CLUSTER_MIN_COSINE,
        "VP_DECAY_LAMBDA": VP_DECAY_LAMBDA,
        "VP_OUTLIER_MIN_SIMILARITY": VP_OUTLIER_MIN_SIMILARITY,
        "VP_OUTLIER_MIN_SAMPLES": VP_OUTLIER_MIN_SAMPLES,
        "CC_COVERAGE_GATE": CC_COVERAGE_GATE,
        "CC_CONFIDENCE_GATE": CC_CONFIDENCE_GATE,
        "OWNER_BOOTSTRAP_MIN_STMTS": OWNER_BOOTSTRAP_MIN_STMTS,
        "OWNER_BOOTSTRAP_MIN_MEDIAN_DURATION_S": OWNER_BOOTSTRAP_MIN_MEDIAN_DURATION_S,
        "OWNER_BOOTSTRAP_MIN_INTRA_COSINE_P25": OWNER_BOOTSTRAP_MIN_INTRA_COSINE_P25,
        "OWNER_BOOTSTRAP_STRONG_EVIDENCE_MIN_STMTS": OWNER_BOOTSTRAP_STRONG_EVIDENCE_MIN_STMTS,
        "OWNER_BOOTSTRAP_MIN_INTRA_COSINE_P25_STRONG": OWNER_BOOTSTRAP_MIN_INTRA_COSINE_P25_STRONG,
        "OWNER_BOOTSTRAP_EVIDENCE_TIER_STANDARD": OWNER_BOOTSTRAP_EVIDENCE_TIER_STANDARD,
        "OWNER_BOOTSTRAP_EVIDENCE_TIER_STRONG": OWNER_BOOTSTRAP_EVIDENCE_TIER_STRONG,
        "OWNER_BOOTSTRAP_PROVISIONAL_GUARD_MIN_TAGS": OWNER_BOOTSTRAP_PROVISIONAL_GUARD_MIN_TAGS,
        "OWNER_REBUILD_MIN_CENTROID_AGREEMENT": OWNER_REBUILD_MIN_CENTROID_AGREEMENT,
        "OWNER_REBUILD_MIN_CLUSTER_SIZE_RATIO": OWNER_REBUILD_MIN_CLUSTER_SIZE_RATIO,
        "OWNER_REBUILD_MAX_COHESION_DROP": OWNER_REBUILD_MAX_COHESION_DROP,
        "OWNER_REBUILD_SUPERSEDED_SCAN_DAYS": OWNER_REBUILD_SUPERSEDED_SCAN_DAYS,
        "NOISY_FLYWHEEL_OVERLAP_MAX": NOISY_FLYWHEEL_OVERLAP_MAX,
        "SLOT_ACTIVE_MIN_SHARE": SLOT_ACTIVE_MIN_SHARE,
        "SPEAKER_EVIDENCE_MULTI_MIN": SPEAKER_EVIDENCE_MULTI_MIN,
        "SPEAKER_EVIDENCE_SINGLE_MAX": SPEAKER_EVIDENCE_SINGLE_MAX,
        "DIARIZE_MIN_OVERLAP": DIARIZE_MIN_OVERLAP,
        "SPEAKER_EVIDENCE_VERSION": SPEAKER_EVIDENCE_VERSION,
        "OVERLAP_DETECTOR_ID": OVERLAP_DETECTOR_ID,
        "OVERLAP_DETECTOR_SHA256": OVERLAP_DETECTOR_SHA256,
        "MERGE_THRESHOLD": MERGE_THRESHOLD,
        "SPLIT_THRESHOLD": SPLIT_THRESHOLD,
        "STABILITY_THRESHOLD": STABILITY_THRESHOLD,
        "CONSOLIDATE_MIN_INTERVALS": CONSOLIDATE_MIN_INTERVALS,
        "CONSOLIDATE_MERGE_THRESHOLD": CONSOLIDATE_MERGE_THRESHOLD,
        "CONSOLIDATE_SUGGEST_MIN": CONSOLIDATE_SUGGEST_MIN,
        "CONFIRM_MIN_SEGMENTS": CONFIRM_MIN_SEGMENTS,
        "CONFIRM_MIN_INTERVALS": CONFIRM_MIN_INTERVALS,
        "CONFIRM_MIN_DURATION_S": CONFIRM_MIN_DURATION_S,
    }


def _speaker_fixture_identity(fixture: str) -> dict[str, Any]:
    fixture_version = 2 if fixture == "solstone-speaker-filterbank" else 1
    return {
        "fixture": fixture,
        "fixture_version": fixture_version,
        "generated_by": "make core-fixtures",
        "kaldi_native_fbank_version": _package_version("kaldi-native-fbank"),
        "source_constants": {
            "diarize": _diarize_constants(),
            "encoder_config": _encoder_config_constants(),
            "overlap": {
                "WINDOW_S": OVERLAP_WINDOW_S,
                "STRIDE_S": OVERLAP_STRIDE_S,
                "_DIARIZE_STRIDE_S": OVERLAP_DIARIZE_STRIDE_S,
                "FRAMES_PER_WINDOW": OVERLAP_FRAMES_PER_WINDOW,
                "OVERLAP_CLASSES": list(OVERLAP_CLASSES),
            },
        },
        "generation_platform": {
            "system": platform.system(),
            "machine": platform.machine(),
        },
    }


def _speaker_filterbank_waveform() -> np.ndarray:
    total_samples = int(DIARIZE_SAMPLE_RATE * SPEAKER_FILTERBANK_DURATION_S)
    near_samples = int(DIARIZE_SAMPLE_RATE * SPEAKER_FILTERBANK_NEAR_S)
    fade_samples = int(DIARIZE_SAMPLE_RATE * SPEAKER_FILTERBANK_FADE_S)
    rng = np.random.default_rng(SPEAKER_FILTERBANK_WAVEFORM_SEED)
    near = (
        rng.standard_normal(total_samples).astype(np.float32)
        * SPEAKER_FILTERBANK_NEAR_AMPLITUDE
    )
    broadband = (
        rng.standard_normal(total_samples).astype(np.float32)
        * SPEAKER_FILTERBANK_BROADBAND_AMPLITUDE
    )
    envelope = np.zeros(total_samples, dtype=np.float32)
    fade_start = near_samples
    fade_end = near_samples + fade_samples
    fade = 0.5 - 0.5 * np.cos(np.linspace(0.0, math.pi, fade_samples, dtype=np.float32))
    envelope[fade_start:fade_end] = fade
    envelope[fade_end:] = 1.0
    return (near + broadband * envelope).astype(np.float32)


def _decimal_rows(matrix: np.ndarray) -> list[str]:
    return [
        " ".join(f"{float(value):.{FLOAT_ROW_DECIMALS}f}" for value in row)
        for row in matrix
    ]


def build_speaker_filterbank_fixture() -> dict[str, Any]:
    audio = _speaker_filterbank_waveform()
    diarize_features = _wespeaker_features(audio)
    main_features = _compute_wespeaker_features(audio, DIARIZE_SAMPLE_RATE)
    if not np.array_equal(diarize_features, main_features):
        raise RuntimeError("WeSpeaker filterbank call sites diverged")
    normalized = _normalize_rows(diarize_features)
    return {
        "identity": _speaker_fixture_identity("solstone-speaker-filterbank"),
        "comparison": {
            "mode": "float_rows",
            "filterbank_abs_tolerance": FILTERBANK_VALUE_ABS_TOLERANCE,
        },
        "waveform": {
            "seed": SPEAKER_FILTERBANK_WAVEFORM_SEED,
            "sample_rate": DIARIZE_SAMPLE_RATE,
            "duration_s": SPEAKER_FILTERBANK_DURATION_S,
            "near_silent_s": SPEAKER_FILTERBANK_NEAR_S,
            "fade_s": SPEAKER_FILTERBANK_FADE_S,
            "near_silent_amplitude": SPEAKER_FILTERBANK_NEAR_AMPLITUDE,
            "broadband_amplitude": SPEAKER_FILTERBANK_BROADBAND_AMPLITUDE,
            "near_silent_rows": list(SPEAKER_FILTERBANK_NEAR_ROWS),
            "broadband_rows": list(SPEAKER_FILTERBANK_BROADBAND_ROWS),
            "samples_f32_le_base64": base64.b64encode(
                audio.astype("<f4", copy=False).tobytes()
            ).decode("ascii"),
        },
        "call_site_agreement": {
            "array_equal": True,
            "shape": list(diarize_features.shape),
        },
        "matrices": {
            "filterbank_cmn": {
                "shape": list(diarize_features.shape),
                "encoding": f"space-separated fixed decimal rows, {FLOAT_ROW_DECIMALS} places",
                "rows": _decimal_rows(diarize_features),
            },
            "row_l2_normalized": {
                "shape": list(normalized.shape),
                "encoding": f"space-separated fixed decimal rows, {FLOAT_ROW_DECIMALS} places",
                "rows": _decimal_rows(normalized),
            },
        },
    }


def _pyannote_class_count() -> int:
    return max(max(SINGLE_SPEAKER_CLASSES), max(OVERLAP_CLASSES)) + 1


def _dominant_log_probs(classes: np.ndarray, *, seed: int) -> np.ndarray:
    rng = np.random.default_rng(seed)
    log_probs = rng.normal(
        loc=-3.0, scale=0.05, size=(len(classes), _pyannote_class_count())
    ).astype(np.float32)
    log_probs[np.arange(len(classes)), classes] = rng.normal(
        loc=2.0, scale=0.03, size=len(classes)
    ).astype(np.float32)
    return log_probs


def _interval_boundary_case(run_frames: int, *, seed: int) -> dict[str, Any]:
    speech_class = min(SINGLE_SPEAKER_CLASSES)
    classes = np.zeros(FRAMES_PER_WINDOW, dtype=np.int64)
    classes[:run_frames] = speech_class
    avg_log_probs = _dominant_log_probs(classes, seed=seed)
    intervals = _find_intervals(
        avg_log_probs,
        audio_len_samples=DIARIZE_WINDOW_S * DIARIZE_SAMPLE_RATE,
    )
    return {
        "run_frames": run_frames,
        "run_duration_s": run_frames * DIARIZE_WINDOW_S / FRAMES_PER_WINDOW,
        "intervals": [
            {"start_s": start, "end_s": end, "local_class": local_class}
            for start, end, local_class in intervals
        ],
    }


def _speaker_evidence_else_branch_band(
    overlap_fraction: float, decision: Any
) -> dict[str, Any]:
    preconditions = {
        "multi_window_fraction_below_multi_min": (
            decision.multi_window_fraction < SPEAKER_EVIDENCE_MULTI_MIN
        ),
        "multi_window_fraction_below_single_max": (
            decision.multi_window_fraction < SPEAKER_EVIDENCE_SINGLE_MAX
        ),
        "overlap_fraction_below_diarize_min_overlap": (
            overlap_fraction < DIARIZE_MIN_OVERLAP
        ),
        "mean_window_overlap_share_at_or_above_diarize_min_overlap": (
            decision.mean_window_overlap_share >= DIARIZE_MIN_OVERLAP
        ),
    }
    return {
        "multi_window_fraction": decision.multi_window_fraction,
        "speaker_evidence_multi_min": SPEAKER_EVIDENCE_MULTI_MIN,
        "speaker_evidence_single_max": SPEAKER_EVIDENCE_SINGLE_MAX,
        "overlap_fraction": overlap_fraction,
        "diarize_min_overlap": DIARIZE_MIN_OVERLAP,
        "mean_window_overlap_share": decision.mean_window_overlap_share,
        "preconditions": preconditions,
    }


def _speaker_evidence_cases() -> dict[str, Any]:
    single_class = min(SINGLE_SPEAKER_CLASSES)
    second_class = sorted(SINGLE_SPEAKER_CLASSES)[1]
    overlap_class = min(OVERLAP_CLASSES)
    speech_frames = OVERLAP_FRAMES_PER_WINDOW
    else_overlap_frames = math.ceil(DIARIZE_MIN_OVERLAP * speech_frames)
    cases = {
        "none": {
            "overlap_fraction": 0.0,
            "class_windows": [np.zeros(OVERLAP_FRAMES_PER_WINDOW, dtype=np.int64)],
        },
        "single": {
            "overlap_fraction": 0.0,
            "class_windows": [
                np.full(OVERLAP_FRAMES_PER_WINDOW, single_class, dtype=np.int64)
            ],
        },
        "multi_by_active_slots": {
            "overlap_fraction": 0.0,
            "class_windows": [
                np.concatenate(
                    [
                        np.full(
                            OVERLAP_FRAMES_PER_WINDOW // 2,
                            single_class,
                            dtype=np.int64,
                        ),
                        np.full(
                            OVERLAP_FRAMES_PER_WINDOW - OVERLAP_FRAMES_PER_WINDOW // 2,
                            second_class,
                            dtype=np.int64,
                        ),
                    ]
                )
            ],
        },
        "else_branch_overlap_ambiguity": {
            "overlap_fraction": max(
                0.0, DIARIZE_MIN_OVERLAP - min(DIARIZE_MIN_OVERLAP / 2, 0.001)
            ),
            "class_windows": [
                np.concatenate(
                    [
                        np.full(
                            speech_frames - else_overlap_frames,
                            single_class,
                            dtype=np.int64,
                        ),
                        np.full(
                            else_overlap_frames,
                            overlap_class,
                            dtype=np.int64,
                        ),
                    ]
                )
            ],
        },
    }
    results = {}
    for name, case in cases.items():
        window_stats = []
        payload_windows = []
        for idx, classes in enumerate(case["class_windows"]):
            stats = _speaker_window_stats(
                _dominant_log_probs(classes, seed=300 + idx + len(results) * 10)
            )
            window_stats.append(stats)
            payload_windows.append(
                {
                    "speech_frames": stats.speech_frames,
                    "active_slot_count": stats.active_slot_count,
                    "overlap_frames": stats.overlap_frames,
                }
            )
        decision = decide_speaker_evidence(case["overlap_fraction"], window_stats)
        result = {
            "overlap_fraction": case["overlap_fraction"],
            "windows": payload_windows,
            "decision": {
                "speaker_evidence": decision.speaker_evidence,
                "multi_window_fraction": decision.multi_window_fraction,
                "mean_window_overlap_share": decision.mean_window_overlap_share,
            },
        }
        if name == "else_branch_overlap_ambiguity":
            branch_band = _speaker_evidence_else_branch_band(
                case["overlap_fraction"], decision
            )
            if not all(branch_band["preconditions"].values()):
                raise RuntimeError(
                    "speaker evidence ambiguity case no longer lands in the else branch"
                )
            result["else_branch_band"] = branch_band
        results[name] = result
    return results


def _cluster_case(rows: np.ndarray) -> dict[str, Any]:
    normalized = _normalize_rows(rows.astype(np.float32))
    curve = []
    for k in range(2, min(MAX_K, len(rows) - 1) + 1):
        labels = _ahc(normalized, k).astype(np.int32)
        curve.append(
            {
                "k": k,
                "silhouette": _silhouette(normalized, labels),
                "labels": labels.tolist(),
            }
        )
    selected_k = _pick_k_silhouette(normalized, MAX_K)
    plain_argmax_k = max(curve, key=lambda row: row["silhouette"])["k"]
    return {
        "selected_k": selected_k,
        "plain_argmax_k": plain_argmax_k,
        "curve": curve,
    }


def _seeded_rows(seed: int, n: int) -> np.ndarray:
    return np.random.default_rng(seed).standard_normal((n, 64)).astype(np.float32)


def build_speaker_stage_boundaries_fixture() -> dict[str, Any]:
    kept = _interval_boundary_case(30, seed=201)
    dropped = _interval_boundary_case(29, seed=202)
    assigned = _assign_sentences(
        [
            {"start": 0.1, "end": 0.3, "text": "inside"},
            {"start": 0.7, "end": 0.8, "text": "outside"},
        ],
        [
            (
                kept["intervals"][0]["start_s"],
                kept["intervals"][0]["end_s"],
                kept["intervals"][0]["local_class"],
            )
        ],
        np.array([0], dtype=np.int32),
    )

    divergence_rows = _seeded_rows(K_DIVERGENCE_SEED, K_DIVERGENCE_N)
    divergence = _cluster_case(divergence_rows)
    perturb_base_rows = _seeded_rows(CLUSTER_PERTURB_SEED, CLUSTER_PERTURB_N)
    perturb_changed_rows = perturb_base_rows.copy()
    perturb_changed_rows[CLUSTER_PERTURB_ROW, CLUSTER_PERTURB_COL] += (
        CLUSTER_PERTURB_EPSILON
    )
    perturb_base = _cluster_case(perturb_base_rows)
    perturb_changed = _cluster_case(perturb_changed_rows)
    if perturb_base["selected_k"] == perturb_changed["selected_k"]:
        raise RuntimeError("pinned clustering perturbation no longer flips selected k")

    return {
        "identity": _speaker_fixture_identity("solstone-speaker-stage-boundaries"),
        "comparison": {
            "mode": "mixed_exact_and_float",
            "cluster_score_abs_tolerance": CLUSTER_SCORE_ABS_TOLERANCE,
        },
        "interval_boundary": {
            "kept_at_30_frames": kept,
            "dropped_at_29_frames": dropped,
            "min_interval_s": MIN_INTERVAL_S,
            "assigned_sentences": assigned,
        },
        "speaker_evidence": _speaker_evidence_cases(),
        "k_selection_divergence": {
            "seed": K_DIVERGENCE_SEED,
            "n": K_DIVERGENCE_N,
            "case": divergence,
        },
        "clustering_input_perturbation": {
            "seed": CLUSTER_PERTURB_SEED,
            "n": CLUSTER_PERTURB_N,
            "row": CLUSTER_PERTURB_ROW,
            "col": CLUSTER_PERTURB_COL,
            "epsilon": CLUSTER_PERTURB_EPSILON,
            "base": perturb_base,
            "perturbed": perturb_changed,
        },
    }


def render_json(payload: dict[str, Any]) -> str:
    return json.dumps(payload, indent=2, sort_keys=True) + "\n"


def _relative_artifact_path(path: Path) -> str:
    return str(path.relative_to(ROOT))


def _json_pointer(parts: tuple[str, ...]) -> str:
    return "/" + "/".join(parts)


def _get_json_path(payload: Any, pointer: str) -> Any:
    current = payload
    for part in pointer.strip("/").split("/"):
        if isinstance(current, list):
            current = current[int(part)]
        else:
            current = current[part]
    return current


def _remove_json_path(payload: Any, pointer: str) -> None:
    parts = pointer.strip("/").split("/")
    current = payload
    for part in parts[:-1]:
        if isinstance(current, list):
            current = current[int(part)]
        else:
            current = current[part]
    leaf = parts[-1]
    if isinstance(current, list):
        del current[int(leaf)]
    else:
        current.pop(leaf, None)


def _compare_exact_artifact(path: Path, current: str, expected: str) -> list[str]:
    if current == expected:
        return []
    return [_relative_artifact_path(path)]


def _json_load_for_compare(
    path: Path, label: str, content: str
) -> tuple[Any, list[str]]:
    try:
        return json.loads(content), []
    except json.JSONDecodeError as exc:
        return None, [
            f"{_relative_artifact_path(path)}: {label} JSON parse failed: {exc}"
        ]


def _format_float_drift(
    path: Path,
    location: str,
    expected: float,
    actual: float,
    tolerance: float,
) -> str:
    diff = abs(actual - expected)
    return (
        f"{_relative_artifact_path(path)}: {location} drifted; "
        f"expected={expected:.9g} actual={actual:.9g} "
        f"abs_diff={diff:.9g} tolerance={tolerance:.9g}"
    )


def _format_float_value_failure(
    path: Path,
    location: str,
    expected: Any,
    actual: Any,
    tolerance: float,
    reason: str,
) -> str:
    return (
        f"{_relative_artifact_path(path)}: {location} drifted; "
        f"expected={expected!r} actual={actual!r} "
        f"abs_diff={reason} tolerance={tolerance:.9g}"
    )


def _compare_float_value(
    path: Path,
    location: str,
    current: Any,
    expected: Any,
    tolerance: float,
) -> list[str]:
    try:
        expected_value = float(expected)
        actual = float(current)
    except (TypeError, ValueError):
        return [
            _format_float_value_failure(
                path, location, expected, current, tolerance, "unparseable"
            )
        ]
    if not math.isfinite(expected_value) or not math.isfinite(actual):
        return [
            _format_float_value_failure(
                path, location, expected, current, tolerance, "non-finite"
            )
        ]
    if abs(actual - expected_value) > tolerance:
        return [_format_float_drift(path, location, expected_value, actual, tolerance)]
    return []


def _compare_decimal_row_matrix(
    path: Path,
    current: Any,
    expected: Any,
    pointer: str,
    tolerance: float,
) -> list[str]:
    current_rows = _get_json_path(current, pointer)
    expected_rows = _get_json_path(expected, pointer)
    if not isinstance(current_rows, list) or not isinstance(expected_rows, list):
        return [f"{_relative_artifact_path(path)}: {pointer} is not a row list"]
    if len(current_rows) != len(expected_rows):
        return [
            f"{_relative_artifact_path(path)}: {pointer} row count drifted; "
            f"expected={len(expected_rows)} actual={len(current_rows)}"
        ]
    failures: list[str] = []
    for row_idx, (current_row, expected_row) in enumerate(
        zip(current_rows, expected_rows, strict=True)
    ):
        current_values = str(current_row).split()
        expected_values = str(expected_row).split()
        if len(current_values) != len(expected_values):
            failures.append(
                f"{_relative_artifact_path(path)}: {pointer}[{row_idx}] column count drifted; "
                f"expected={len(expected_values)} actual={len(current_values)}"
            )
            continue
        for col_idx, (current_text, expected_text) in enumerate(
            zip(current_values, expected_values, strict=True)
        ):
            failures.extend(
                _compare_float_value(
                    path,
                    f"{pointer}[{row_idx}][{col_idx}]",
                    current_text,
                    expected_text,
                    tolerance,
                )
            )
    return failures


def _compare_speaker_filterbank(path: Path, current: str, expected: str) -> list[str]:
    current_obj, errors = _json_load_for_compare(path, "current", current)
    expected_obj, expected_errors = _json_load_for_compare(path, "expected", expected)
    if errors or expected_errors:
        return errors + expected_errors

    matrix_pointers = (
        "/matrices/filterbank_cmn/rows",
        "/matrices/row_l2_normalized/rows",
    )
    current_exact = copy.deepcopy(current_obj)
    expected_exact = copy.deepcopy(expected_obj)
    for pointer in SPEAKER_FIXTURE_IGNORED_JSON_POINTERS + matrix_pointers:
        _remove_json_path(current_exact, pointer)
        _remove_json_path(expected_exact, pointer)

    failures: list[str] = []
    if current_exact != expected_exact:
        failures.append(
            f"{_relative_artifact_path(path)}: non-float content drifted outside tolerance-managed paths"
        )
    for pointer in matrix_pointers:
        failures.extend(
            _compare_decimal_row_matrix(
                path,
                current_obj,
                expected_obj,
                pointer,
                FILTERBANK_VALUE_ABS_TOLERANCE,
            )
        )
    return failures


def _compare_stage_value(
    path: Path,
    current: Any,
    expected: Any,
    ignored_json_pointers: frozenset[str],
    parts: tuple[str, ...] = (),
) -> list[str]:
    pointer = _json_pointer(parts) if parts else "/"
    if pointer in ignored_json_pointers:
        return []
    if parts and parts[-1] == "silhouette":
        return _compare_float_value(
            path,
            pointer,
            current,
            expected,
            CLUSTER_SCORE_ABS_TOLERANCE,
        )
    if isinstance(expected, dict):
        if not isinstance(current, dict):
            return [
                f"{_relative_artifact_path(path)}: {pointer} type drifted; expected=dict actual={type(current).__name__}"
            ]
        if set(current) != set(expected):
            return [
                f"{_relative_artifact_path(path)}: {pointer} keys drifted; "
                f"expected={sorted(expected)} actual={sorted(current)}"
            ]
        failures: list[str] = []
        for key in sorted(expected):
            failures.extend(
                _compare_stage_value(
                    path,
                    current[key],
                    expected[key],
                    ignored_json_pointers,
                    parts + (key,),
                )
            )
        return failures
    if isinstance(expected, list):
        if not isinstance(current, list):
            return [
                f"{_relative_artifact_path(path)}: {pointer} type drifted; expected=list actual={type(current).__name__}"
            ]
        if len(current) != len(expected):
            return [
                f"{_relative_artifact_path(path)}: {pointer} length drifted; "
                f"expected={len(expected)} actual={len(current)}"
            ]
        failures = []
        for idx, (current_item, expected_item) in enumerate(
            zip(current, expected, strict=True)
        ):
            failures.extend(
                _compare_stage_value(
                    path,
                    current_item,
                    expected_item,
                    ignored_json_pointers,
                    parts + (str(idx),),
                )
            )
        return failures
    if current != expected:
        return [
            f"{_relative_artifact_path(path)}: {pointer} drifted; "
            f"expected={expected!r} actual={current!r}"
        ]
    return []


def _compare_speaker_stage_boundaries(
    path: Path, current: str, expected: str
) -> list[str]:
    current_obj, errors = _json_load_for_compare(path, "current", current)
    expected_obj, expected_errors = _json_load_for_compare(path, "expected", expected)
    if errors or expected_errors:
        return errors + expected_errors
    return _compare_stage_value(
        path,
        current_obj,
        expected_obj,
        frozenset(SPEAKER_FIXTURE_IGNORED_JSON_POINTERS),
    )


def compare_artifact(
    path: Path, descriptor: ArtifactDescriptor, current: str, expected: str
) -> list[str]:
    if descriptor.comparison == "exact":
        return _compare_exact_artifact(path, current, expected)
    if descriptor.comparison == "speaker_filterbank":
        return _compare_speaker_filterbank(path, current, expected)
    if descriptor.comparison == "speaker_stage_boundaries":
        return _compare_speaker_stage_boundaries(path, current, expected)
    raise ValueError(f"unknown comparison mode: {descriptor.comparison}")


def expected_outputs() -> dict[Path, ArtifactDescriptor]:
    return {
        CALLOSUM_ARTIFACT_PATH: ArtifactDescriptor(
            build_callosum_registry_fixture,
        ),
        COGITATE_ARTIFACT_PATH: ArtifactDescriptor(
            build_cogitate_contract_fixture,
        ),
        GENERATE_ARTIFACT_PATH: ArtifactDescriptor(
            build_generate_contract_fixture,
        ),
        INSTALL_STATUS_ARTIFACT_PATH: ArtifactDescriptor(
            install_status_corpus.build_install_status_fixture,
        ),
        EDGE_SCHEMA_ARTIFACT_PATH: ArtifactDescriptor(
            build_edge_schema_fixture,
        ),
        MARKDOWN_CHUNKS_ARTIFACT_PATH: ArtifactDescriptor(
            build_markdown_chunks_fixture,
        ),
        CONTENT_FAMILIES_ARTIFACT_PATH: ArtifactDescriptor(
            content_family_corpus.build_content_families_fixture,
        ),
        TALENT_PROJECTIONS_ARTIFACT_PATH: ArtifactDescriptor(
            talent_projection_corpus.build_talent_projection_fixture,
        ),
        ENTITY_IDENTITY_ARTIFACT_PATH: ArtifactDescriptor(
            entity_corpus.build_entity_identity_fixture,
        ),
        ENTITY_MATCHING_ARTIFACT_PATH: ArtifactDescriptor(
            entity_corpus.build_entity_matching_fixture,
        ),
        ENTITY_RESOLUTION_MAP_DIVERGENCES_ARTIFACT_PATH: ArtifactDescriptor(
            entity_corpus.build_entity_resolution_map_divergences_fixture,
        ),
        ENTITY_STORE_ARTIFACT_PATH: ArtifactDescriptor(
            entity_corpus.build_entity_store_fixture,
        ),
        ENTITY_LIFECYCLE_ARTIFACT_PATH: ArtifactDescriptor(
            entity_corpus.build_entity_lifecycle_fixture,
        ),
        VOICEPRINT_OPERATIONS_ARTIFACT_PATH: ArtifactDescriptor(
            entity_corpus.build_voiceprint_operations_fixture,
        ),
        SPEAKER_FILTERBANK_ARTIFACT_PATH: ArtifactDescriptor(
            build_speaker_filterbank_fixture,
            comparison="speaker_filterbank",
        ),
        SPEAKER_STAGE_BOUNDARIES_ARTIFACT_PATH: ArtifactDescriptor(
            build_speaker_stage_boundaries_fixture,
            comparison="speaker_stage_boundaries",
        ),
        link_join_fixture_dir() / "observer_ascii_peer.json": ArtifactDescriptor(
            build_link_join_observer_ascii_peer_json,
        ),
        link_join_fixture_dir() / "peer_non_ascii_peer.json": ArtifactDescriptor(
            build_link_join_peer_non_ascii_peer_json,
        ),
        link_join_fixture_dir() / "nested_endpoints_peer.json": ArtifactDescriptor(
            build_link_join_nested_endpoints_peer_json,
        ),
    }


def write_outputs() -> None:
    FIXTURE_DIR.mkdir(parents=True, exist_ok=True)
    for path, descriptor in expected_outputs().items():
        content = descriptor.render()
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(content, encoding="utf-8")
        print(f"wrote {path.relative_to(ROOT)}")


def check_outputs() -> int:
    stale: list[str] = []
    drift: list[str] = []
    for path, descriptor in expected_outputs().items():
        expected = descriptor.render()
        try:
            current = path.read_text(encoding="utf-8")
        except FileNotFoundError:
            current = ""
        errors = compare_artifact(path, descriptor, current, expected)
        if not errors:
            continue
        if descriptor.comparison == "exact":
            stale.extend(errors)
        else:
            drift.extend(errors)

    if stale:
        paths = ", ".join(stale)
        print(
            f"Core generated fixtures are stale: {paths}. Run: make core-fixtures",
            file=sys.stderr,
        )
    if drift:
        print("Core generated float fixtures drifted:", file=sys.stderr)
        for error in drift:
            print(f"  {error}", file=sys.stderr)
    if stale or drift:
        return 1
    return 0


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--check",
        action="store_true",
        help="Check generated fixtures without writing files.",
    )
    args = parser.parse_args(argv)

    if args.check:
        return check_outputs()
    write_outputs()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
