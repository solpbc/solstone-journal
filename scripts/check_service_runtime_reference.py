#!/usr/bin/env python3
"""Verify the immutable read-only service-runtime evidence fixture."""

from __future__ import annotations

import base64
import copy
import hashlib
import importlib.util
import re
import subprocess
from pathlib import Path
from types import ModuleType

ROOT = Path(__file__).resolve().parents[1]
FIXTURE = ROOT / "core/fixtures/service_runtime_reference.json"
CAPTURE = ROOT / "scripts/capture_service_runtime_reference.py"
FIXTURE_SHA256 = "8a0335d0db6e459da6d44c1f6337f429753964c0eec1c5af89c4a5838f54fd27"
CAPTURE_SHA256 = "fc1d264d56fdb2eeb056d867de872117d7aa7deca863eb5833c947a72c07b425"
CAPTURE_COMMIT = "0add95fca75aa0c67c827b06f7f4dda1ef668fe2"
CAPTURE_BLOB = "12f5b2248f5dce2e2f7aa9d9750c2760934469e5"


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def load_capture() -> ModuleType:
    spec = importlib.util.spec_from_file_location("_service_runtime_capture", CAPTURE)
    if spec is None or spec.loader is None:
        raise RuntimeError("service-runtime-reference: cannot load capture tool")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def git(*args: str) -> str:
    result = subprocess.run(
        ["git", *args],
        cwd=ROOT,
        stdin=subprocess.DEVNULL,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
        timeout=10,
    )
    if result.returncode != 0:
        raise RuntimeError(
            f"service-runtime-reference: git {' '.join(args)} failed: "
            f"{result.stderr.decode('utf-8', 'replace').strip()}"
        )
    return result.stdout.decode("ascii", "strict").strip()


def verify_provenance() -> None:
    capture_bytes = CAPTURE.read_bytes()
    if sha256(capture_bytes) != CAPTURE_SHA256:
        raise RuntimeError("service-runtime-reference: capture tool digest mismatch")
    committed_blob = git(
        "rev-parse", f"{CAPTURE_COMMIT}:scripts/capture_service_runtime_reference.py"
    )
    if committed_blob != CAPTURE_BLOB:
        raise RuntimeError("service-runtime-reference: capture commit/blob mismatch")
    committed_bytes = subprocess.run(
        [
            "git",
            "show",
            f"{CAPTURE_COMMIT}:scripts/capture_service_runtime_reference.py",
        ],
        cwd=ROOT,
        stdin=subprocess.DEVNULL,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=True,
        timeout=10,
    ).stdout
    if committed_bytes != capture_bytes or sha256(committed_bytes) != CAPTURE_SHA256:
        raise RuntimeError(
            "service-runtime-reference: working capture differs from producer"
        )


def verify_privacy(value: dict[str, object]) -> None:
    patterns = (
        re.compile(rb"/Users/[^/\s]+"),
        re.compile(rb"@"),
        re.compile(rb"(?:token|secret|password|authorization)", re.IGNORECASE),
    )
    for capture in value["captures"]:  # type: ignore[index]
        for row in capture["rows"]:
            for stream in ("stdout_b64", "stderr_b64"):
                raw = base64.b64decode(row["result"][stream], validate=True)
                if any(pattern.search(raw) for pattern in patterns):
                    raise RuntimeError(
                        "service-runtime-reference: retained stream failed privacy scan"
                    )


def verify_semantics(module: ModuleType, fixture_bytes: bytes) -> None:
    value = module.load_json_bytes(fixture_bytes)
    if set(value) != {
        "captures",
        "derivation_report",
        "product_ground",
        "schema",
        "tool_sha256",
    }:
        raise RuntimeError("service-runtime-reference: root shape mismatch")
    if value["tool_sha256"] != CAPTURE_SHA256:
        raise RuntimeError("service-runtime-reference: fixture producer mismatch")
    captures = [module.validate_partial(capture) for capture in value["captures"]]
    if {capture["profile"] for capture in captures} != set(module.EXPECTED_PROFILES):
        raise RuntimeError("service-runtime-reference: profile denominator mismatch")
    report = module.derivation_report(captures)
    if report != value["derivation_report"]:
        raise RuntimeError("service-runtime-reference: derivation report mismatch")
    if module.canonical_bytes(value) != fixture_bytes:
        raise RuntimeError("service-runtime-reference: fixture is not canonical")
    verify_privacy(value)

    linux = next(item for item in value["captures"] if item["platform"] == "linux")
    loaded_index = next(
        index for index, row in enumerate(linux["rows"]) if row["role"] == "loaded"
    )
    mutations = {}

    conflicting = copy.deepcopy(linux)
    conflicting["rows"][loaded_index]["semantics"] = {"forged": True}
    mutations["conflicting-semantics"] = conflicting

    invalid_byte = copy.deepcopy(linux)
    invalid_result = invalid_byte["rows"][loaded_index]["result"]
    invalid_result["stdout_b64"] = module.b64(b"\xff")
    invalid_result["stdout_sha256"] = module.sha256(b"\xff")
    mutations["invalid-byte"] = invalid_byte

    bounded_output = copy.deepcopy(linux)
    bounded_result = bounded_output["rows"][loaded_index]["result"]
    oversized = b"x" * (module.MAX_STREAM_BYTES + 1)
    bounded_result["stdout_b64"] = module.b64(oversized)
    bounded_result["stdout_sha256"] = module.sha256(oversized)
    mutations["bounded-output"] = bounded_output

    for name, mutated in mutations.items():
        try:
            module.validate_partial(mutated)
        except module.EvidenceError:
            continue
        raise RuntimeError(f"service-runtime-reference: {name} mutation was accepted")


def main() -> None:
    fixture_bytes = FIXTURE.read_bytes()
    if sha256(fixture_bytes) != FIXTURE_SHA256:
        raise RuntimeError("service-runtime-reference: fixture digest mismatch")
    verify_provenance()
    verify_semantics(load_capture(), fixture_bytes)
    print("service runtime reference verified")


if __name__ == "__main__":
    main()
