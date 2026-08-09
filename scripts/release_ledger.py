#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Canonical release-candidate ledger writer."""

from __future__ import annotations

import hashlib
import json
import os
import zipfile
from collections.abc import Mapping, Sequence
from dataclasses import dataclass
from pathlib import Path
from typing import Any

import scripts.check_release_preflight as preflight
from scripts.check_rust_release_manifest import (
    SHA256_RE,
    SOURCE_COMMIT_RE,
    Failure,
    canonical_json_bytes,
    rust_artifact_targets,
)
from scripts.check_wheel_contents import (
    CORE_SCRIPT_NAMES,
    PARAKEET_HELPER_MEMBER,
    core_wheel_script_members,
)
from scripts.release_advisory_policy import PolicyRun, validate_snapshot_identity
from scripts.release_digest import candidate_digest, file_sha256_size
from scripts.release_install_smoke import CANDIDATE, PROOF_TARGETS
from scripts.release_nvattest_proof import CHALLENGE_RE
from scripts.release_nvattest_support import validate_support_declarations
from scripts.release_public_evidence import validate_public_evidence_tree

RELEASE_EVIDENCE_CONTRACT_DOC = "docs/release-evidence-contract.md"
CURRENT_LEDGER_SCHEMA_VERSION = 2


@dataclass(frozen=True)
class RetainedLedgerSchema:
    top_level_keys: frozenset[str]
    models_keys: frozenset[str]
    nvattest_keys: frozenset[str] | None
    policy_run_keys: frozenset[str]


LEDGER_SCHEMA_REGISTRY: dict[int, RetainedLedgerSchema] = {
    1: RetainedLedgerSchema(
        top_level_keys=frozenset(
            (
                "schema_version",
                "kind",
                "product",
                "version",
                "source_commit",
                "candidate",
                "models",
                "core_lock_sha256",
                "rust_targets",
                "tool_evidence",
                "native_members",
                "dependency_policy",
                "policy_run",
                "native_summary",
                "proofs",
                "redaction",
            )
        ),
        models_keys=frozenset(("decision", "package_version")),
        nvattest_keys=None,
        policy_run_keys=frozenset(
            (
                "advisory_source_id",
                "db_snapshot_basename",
                "db_commit",
                "db_archive_sha256",
                "advisory_count",
                "advisory_acquired_at",
                "db_commit_timestamp",
                "policy_checked_at",
                "result",
            )
        ),
    ),
    2: RetainedLedgerSchema(
        top_level_keys=frozenset(
            (
                "schema_version",
                "kind",
                "product",
                "version",
                "source_commit",
                "candidate",
                "models",
                "core_lock_sha256",
                "rust_targets",
                "tool_evidence",
                "native_members",
                "dependency_policy",
                "policy_run",
                "native_summary",
                "proofs",
                "nvattest",
                "redaction",
            )
        ),
        models_keys=frozenset(("decision", "package_version")),
        nvattest_keys=frozenset(
            ("challenge", "authority_sha256", "authority", "support_distributions")
        ),
        policy_run_keys=frozenset(
            (
                "advisory_source_id",
                "db_snapshot_basename",
                "db_commit",
                "db_archive_sha256",
                "advisory_count",
                "advisory_acquired_at",
                "db_commit_timestamp",
                "policy_checked_at",
                "result",
            )
        ),
    ),
}
RETAINED_LEDGER_CONSUMER_TOP_LEVEL_KEYS = frozenset(
    ("candidate", "core_lock_sha256", "models")
)


class LedgerError(RuntimeError):
    def __init__(self, failures: Sequence[Failure]) -> None:
        self.failures = tuple(failures)
        super().__init__("; ".join(failure.error for failure in self.failures))


def _failure(error: str, *, expected: str, actual: str, repair: str) -> Failure:
    return Failure(error=error, expected=expected, actual=actual, repair=repair)


def _registered_schema_versions() -> str:
    return ", ".join(str(version) for version in sorted(LEDGER_SCHEMA_REGISTRY))


def _registered_ledger_schema(version: int) -> RetainedLedgerSchema:
    try:
        return LEDGER_SCHEMA_REGISTRY[version]
    except KeyError as exc:
        raise AssertionError(
            f"ledger schema_version {version} is not registered"
        ) from exc


def _current_writer_schema() -> RetainedLedgerSchema:
    return _registered_ledger_schema(CURRENT_LEDGER_SCHEMA_VERSION)


def current_retained_ledger_schema() -> tuple[int, RetainedLedgerSchema]:
    return CURRENT_LEDGER_SCHEMA_VERSION, _current_writer_schema()


def retained_ledger_schema_declares_nvattest(
    schema: RetainedLedgerSchema,
) -> bool:
    return "nvattest" in schema.top_level_keys


def _resolve_retained_ledger_schema(
    payload: Mapping[str, Any],
) -> tuple[int | None, RetainedLedgerSchema | None, list[Failure]]:
    if "schema_version" not in payload:
        return (
            None,
            None,
            [
                _failure(
                    "retained ledger schema_version is missing",
                    expected="positive integer schema_version",
                    actual="<missing>",
                    repair=(
                        "restore the retained ledger schema_version; see "
                        f"{RELEASE_EVIDENCE_CONTRACT_DOC}"
                    ),
                )
            ],
        )

    version = payload["schema_version"]
    if type(version) is not int or version <= 0:
        return (
            None,
            None,
            [
                _failure(
                    f"retained ledger schema_version is malformed: {version!r}",
                    expected="positive integer schema_version",
                    actual=repr(version),
                    repair=(
                        "set schema_version to a registered positive integer; see "
                        f"{RELEASE_EVIDENCE_CONTRACT_DOC}"
                    ),
                )
            ],
        )

    schema = LEDGER_SCHEMA_REGISTRY.get(version)
    if schema is None:
        registered = _registered_schema_versions()
        return (
            version,
            None,
            [
                _failure(
                    f"retained ledger schema_version {version} is not registered",
                    expected=f"registered schema_version values: {registered}",
                    actual=str(version),
                    repair=(
                        "append a registered schema version or recover with the "
                        f"release-bound reader; see {RELEASE_EVIDENCE_CONTRACT_DOC}"
                    ),
                )
            ],
        )

    missing_consumer_keys = sorted(
        RETAINED_LEDGER_CONSUMER_TOP_LEVEL_KEYS - schema.top_level_keys
    )
    if missing_consumer_keys:
        missing = ", ".join(missing_consumer_keys)
        return (
            version,
            schema,
            [
                _failure(
                    (
                        f"retained ledger schema_version {version} omits current "
                        f"consumer top-level key {missing}; see "
                        f"{RELEASE_EVIDENCE_CONTRACT_DOC}"
                    ),
                    expected=(
                        f"schema_version {version} top-level keys include {missing}"
                    ),
                    actual=(f"schema_version {version} top-level keys omit {missing}"),
                    repair=(
                        "append a schema version or update the declared consumer "
                        f"requirements; see {RELEASE_EVIDENCE_CONTRACT_DOC}"
                    ),
                )
            ],
        )

    return version, schema, []


def resolve_retained_ledger_schema(
    payload: Mapping[str, Any],
) -> tuple[int, RetainedLedgerSchema]:
    version, schema, failures = _resolve_retained_ledger_schema(payload)
    if failures:
        raise LedgerError(failures)
    if version is None or schema is None:
        raise AssertionError("retained ledger schema resolution returned no schema")
    return version, schema


def _candidate_files(release_dir: Path) -> list[dict[str, Any]]:
    files: list[dict[str, Any]] = []
    for path in sorted(
        (path for path in release_dir.iterdir() if path.is_file()),
        key=lambda item: item.name,
    ):
        sha256, byte_count = file_sha256_size(path)
        files.append({"name": path.name, "sha256": sha256, "bytes": byte_count})
    return files


def _rust_targets() -> list[dict[str, Any]]:
    targets: list[dict[str, Any]] = []
    for artifact, (lane, target) in sorted(rust_artifact_targets().items()):
        targets.append({"lane": lane, "artifact": artifact, **target})
    return targets


def _validate_tool_evidence(
    tool_evidence: Mapping[str, Mapping[str, str]],
) -> list[Failure]:
    failures: list[Failure] = []
    expected_lanes = set(preflight.LANE_TOOL_KEYS)
    if set(tool_evidence) != expected_lanes:
        failures.append(
            _failure(
                "release tool evidence lanes are invalid",
                expected=", ".join(sorted(expected_lanes)),
                actual=", ".join(sorted(str(key) for key in tool_evidence))
                or "<empty>",
                repair="python3 scripts/check_release_preflight.py lane-tools --help",
            )
        )
    for lane in sorted(set(tool_evidence) & expected_lanes):
        evidence = tool_evidence[lane]
        if not isinstance(evidence, Mapping):
            failures.append(
                _failure(
                    "release tool evidence lane is invalid",
                    expected=f"{lane} tool evidence object",
                    actual=type(evidence).__name__,
                    repair="python3 scripts/check_release_preflight.py lane-tools --help",
                )
            )
            continue
        failures.extend(preflight.check_lane_tool_evidence(lane, evidence))
    failures.extend(
        validate_public_evidence_tree("ledger.tool_evidence", tool_evidence)
    )
    return failures


def _policy_run_payload(policy_run: PolicyRun) -> dict[str, Any]:
    schema = _current_writer_schema()
    payload = {
        "advisory_source_id": policy_run.advisory_source_id,
        "db_snapshot_basename": policy_run.db_snapshot_basename,
        "db_commit": policy_run.db_commit,
        "db_archive_sha256": policy_run.db_archive_sha256,
        "advisory_count": policy_run.advisory_count,
        "advisory_acquired_at": policy_run.advisory_acquired_at,
        "db_commit_timestamp": policy_run.db_commit_timestamp,
        "policy_checked_at": policy_run.policy_checked_at,
        "result": policy_run.result,
    }
    if set(payload) != schema.policy_run_keys:
        raise AssertionError("policy run payload key set drifted")
    failures = validate_snapshot_identity(
        "ledger.policy_run",
        db_commit=payload["db_commit"],
        db_archive_sha256=payload["db_archive_sha256"],
    )
    if failures:
        raise LedgerError(failures)
    return payload


def _validate_member_entry(label: str, value: Any) -> list[Failure]:
    failures: list[Failure] = []
    if not isinstance(value, Mapping) or set(value) != {"path", "sha256", "bytes"}:
        return [
            _failure(
                f"{label} native member entry is invalid",
                expected="path, sha256, bytes",
                actual=repr(value),
                repair="python3 scripts/check_rust_release_manifest.py",
            )
        ]
    if not isinstance(value.get("path"), str) or not value["path"]:
        failures.append(
            _failure(
                f"{label} native member path is invalid",
                expected="non-empty retained wheel member path",
                actual=repr(value.get("path")),
                repair="python3 scripts/check_rust_release_manifest.py",
            )
        )
    if not isinstance(value.get("sha256"), str) or not SHA256_RE.fullmatch(
        value["sha256"]
    ):
        failures.append(
            _failure(
                f"{label} native member sha256 is invalid",
                expected="lowercase SHA-256",
                actual=repr(value.get("sha256")),
                repair="python3 scripts/check_rust_release_manifest.py",
            )
        )
    if not isinstance(value.get("bytes"), int) or value["bytes"] < 0:
        failures.append(
            _failure(
                f"{label} native member byte count is invalid",
                expected="non-negative integer",
                actual=repr(value.get("bytes")),
                repair="python3 scripts/check_rust_release_manifest.py",
            )
        )
    return failures


def validate_native_members(value: Any) -> list[Failure]:
    failures: list[Failure] = []
    if not isinstance(value, Mapping):
        return [
            _failure(
                "retained ledger native_members is invalid",
                expected="target native member map",
                actual=type(value).__name__,
                repair="python3 scripts/check_rust_release_manifest.py",
            )
        ]
    expected_targets = set(PROOF_TARGETS)
    if set(value) != expected_targets:
        failures.append(
            _failure(
                "retained ledger native_members targets are invalid",
                expected=", ".join(sorted(expected_targets)),
                actual=", ".join(sorted(str(key) for key in value)) or "<empty>",
                repair="python3 scripts/check_rust_release_manifest.py",
            )
        )
    for target in sorted(set(value) & expected_targets):
        members = value[target]
        expected_member_names = (
            {*CORE_SCRIPT_NAMES, "parakeet-helper"}
            if target == "macos-arm64"
            else set(CORE_SCRIPT_NAMES)
        )
        if not isinstance(members, Mapping):
            failures.append(
                _failure(
                    f"retained ledger {target} native members are invalid",
                    expected="native member map",
                    actual=type(members).__name__,
                    repair="python3 scripts/check_rust_release_manifest.py",
                )
            )
            continue
        if set(members) != expected_member_names:
            failures.append(
                _failure(
                    f"retained ledger {target} native member set is invalid",
                    expected=", ".join(sorted(expected_member_names)),
                    actual=", ".join(sorted(str(key) for key in members)) or "<empty>",
                    repair="python3 scripts/check_rust_release_manifest.py",
                )
            )
        for name, entry in sorted(members.items()):
            failures.extend(_validate_member_entry(f"{target}.{name}", entry))
    failures.extend(validate_public_evidence_tree("ledger.native_members", value))
    return failures


def validate_models_payload(
    value: Any,
    candidate: Any | None = None,
    *,
    schema: RetainedLedgerSchema | None = None,
) -> list[Failure]:
    failures: list[Failure] = []
    resolved_schema = schema or _current_writer_schema()
    if not isinstance(value, Mapping):
        return [
            _failure(
                "retained ledger models binding is invalid",
                expected="models decision object",
                actual=type(value).__name__,
                repair="python3 scripts/check_rust_release_manifest.py",
            )
        ]
    if set(value) != resolved_schema.models_keys:
        failures.append(
            _failure(
                "retained ledger models key set is invalid",
                expected=", ".join(sorted(resolved_schema.models_keys)),
                actual=", ".join(sorted(str(key) for key in value)) or "<empty>",
                repair="python3 scripts/check_rust_release_manifest.py",
            )
        )
    decision = value.get("decision")
    if decision not in {"include", "exclude"}:
        failures.append(
            _failure(
                "retained ledger models decision is invalid",
                expected="include or exclude",
                actual=str(decision),
                repair="python3 scripts/check_rust_release_manifest.py",
            )
        )
    package_version = value.get("package_version")
    if not isinstance(package_version, str) or not package_version:
        failures.append(
            _failure(
                "retained ledger models package version is invalid",
                expected="non-empty package version",
                actual=repr(package_version),
                repair="python3 scripts/check_rust_release_manifest.py",
            )
        )
    if (
        isinstance(candidate, Mapping)
        and decision in {"include", "exclude"}
        and isinstance(package_version, str)
        and package_version
    ):
        raw_files = candidate.get("files")
        files = raw_files if isinstance(raw_files, Sequence) else ()
        package_names = {
            str(item.get("name"))
            for item in files
            if isinstance(item, Mapping)
            and isinstance(item.get("name"), str)
            and not str(item.get("name")).endswith(".rust-release-manifest.json")
        }
        model_names = {
            name
            for name in package_names
            if name.startswith("solstone_journal_models-")
        }
        expected_models = {
            f"solstone_journal_models-{package_version}.tar.gz",
            f"solstone_journal_models-{package_version}-py3-none-any.whl",
        }
        expected_model_names = expected_models if decision == "include" else set()
        if model_names != expected_model_names:
            failures.append(
                _failure(
                    "retained ledger candidate inventory does not match models decision",
                    expected=", ".join(sorted(expected_model_names)) or "<no models>",
                    actual=", ".join(sorted(model_names)) or "<no models>",
                    repair="python3 scripts/check_rust_release_manifest.py",
                )
            )
    failures.extend(validate_public_evidence_tree("ledger.models", value))
    return failures


def _canonical_nvattest_authority_bytes(payload: Mapping[str, Any]) -> bytes:
    return (json.dumps(payload, indent=2, sort_keys=True) + "\n").encode("utf-8")


def validate_nvattest_payload(
    value: Any, *, schema: RetainedLedgerSchema | None = None
) -> list[Failure]:
    failures: list[Failure] = []
    resolved_schema = schema or _current_writer_schema()
    if not retained_ledger_schema_declares_nvattest(resolved_schema):
        if value is None:
            return []
        return [
            _failure(
                "retained ledger schema does not declare nvattest",
                expected="schema with no nvattest binding",
                actual="nvattest present",
                repair="python3 scripts/check_rust_release_manifest.py",
            )
        ]
    if not isinstance(value, Mapping):
        return [
            _failure(
                "retained ledger nvattest binding is invalid",
                expected="nvattest object",
                actual=type(value).__name__,
                repair="python3 scripts/check_rust_release_manifest.py",
            )
        ]
    if resolved_schema.nvattest_keys is None:
        raise AssertionError("nvattest schema has no declared key set")
    if set(value) != resolved_schema.nvattest_keys:
        failures.append(
            _failure(
                "retained ledger nvattest key set is invalid",
                expected=", ".join(sorted(resolved_schema.nvattest_keys)),
                actual=", ".join(sorted(str(key) for key in value)) or "<empty>",
                repair="python3 scripts/check_rust_release_manifest.py",
            )
        )
    challenge = value.get("challenge")
    if not isinstance(challenge, str) or not CHALLENGE_RE.fullmatch(challenge):
        failures.append(
            _failure(
                "retained ledger nvattest challenge is invalid",
                expected="64 lowercase hexadecimal characters",
                actual=repr(challenge),
                repair="python3 scripts/check_rust_release_manifest.py",
            )
        )
    authority_sha256 = value.get("authority_sha256")
    if not isinstance(authority_sha256, str) or not SHA256_RE.fullmatch(
        authority_sha256
    ):
        failures.append(
            _failure(
                "retained ledger nvattest authority_sha256 is invalid",
                expected="lowercase SHA-256",
                actual=repr(authority_sha256),
                repair="python3 scripts/check_rust_release_manifest.py",
            )
        )
    authority = value.get("authority")
    if not isinstance(authority, Mapping):
        failures.append(
            _failure(
                "retained ledger nvattest authority is invalid",
                expected="parsed authority JSON object",
                actual=type(authority).__name__,
                repair="python3 scripts/check_rust_release_manifest.py",
            )
        )
    elif isinstance(authority_sha256, str) and SHA256_RE.fullmatch(authority_sha256):
        digest = hashlib.sha256(
            _canonical_nvattest_authority_bytes(authority)
        ).hexdigest()
        if digest != authority_sha256:
            failures.append(
                _failure(
                    "retained ledger nvattest authority digest is invalid",
                    expected=authority_sha256,
                    actual=digest,
                    repair="python3 scripts/check_rust_release_manifest.py",
                )
            )
    failures.extend(
        validate_support_declarations(
            value.get("support_distributions"),
            repair="python3 scripts/check_rust_release_manifest.py",
        )
    )
    failures.extend(validate_public_evidence_tree("ledger.nvattest", value))
    return failures


def validate_retained_ledger(payload: Mapping[str, Any]) -> list[Failure]:
    failures: list[Failure] = []
    _version, schema, schema_failures = _resolve_retained_ledger_schema(payload)
    if schema_failures:
        return schema_failures
    if schema is None:
        raise AssertionError("retained ledger schema resolution returned no schema")

    if set(payload) != schema.top_level_keys:
        failures.append(
            _failure(
                "retained ledger top-level key set is invalid",
                expected=", ".join(sorted(schema.top_level_keys)),
                actual=", ".join(sorted(str(key) for key in payload)) or "<empty>",
                repair="python3 scripts/check_rust_release_manifest.py",
            )
        )
    policy_run = payload.get("policy_run")
    if not isinstance(policy_run, Mapping):
        failures.append(
            _failure(
                "retained ledger policy_run is invalid",
                expected="policy_run object",
                actual=str(policy_run),
                repair="python3 scripts/check_rust_release_manifest.py",
            )
        )
    else:
        if set(policy_run) != schema.policy_run_keys:
            failures.append(
                _failure(
                    "retained ledger policy_run key set is invalid",
                    expected=", ".join(sorted(schema.policy_run_keys)),
                    actual=", ".join(sorted(str(key) for key in policy_run))
                    or "<empty>",
                    repair="python3 scripts/check_rust_release_manifest.py",
                )
            )
        failures.extend(
            validate_snapshot_identity(
                "ledger.policy_run",
                db_commit=policy_run.get("db_commit"),
                db_archive_sha256=policy_run.get("db_archive_sha256"),
            )
        )
    tool_evidence = payload.get("tool_evidence")
    if isinstance(tool_evidence, Mapping):
        failures.extend(_validate_tool_evidence(tool_evidence))
    else:
        failures.append(
            _failure(
                "retained ledger tool_evidence is invalid",
                expected="per-lane full tool evidence",
                actual=type(tool_evidence).__name__,
                repair="python3 scripts/check_release_preflight.py lane-tools --help",
            )
        )
    failures.extend(
        validate_models_payload(
            payload.get("models"), payload.get("candidate"), schema=schema
        )
    )
    failures.extend(validate_nvattest_payload(payload.get("nvattest"), schema=schema))
    failures.extend(validate_native_members(payload.get("native_members")))
    failures.extend(validate_public_evidence_tree("ledger", payload))
    return failures


def read_retained_ledger(path: Path) -> dict[str, Any]:
    try:
        payload = json.loads(path.read_text(encoding="utf-8"))
    except json.JSONDecodeError as exc:
        raise LedgerError(
            [
                _failure(
                    "retained ledger is not valid JSON",
                    expected="JSON object",
                    actual=str(exc),
                    repair="python3 scripts/check_rust_release_manifest.py",
                )
            ]
        ) from exc
    if not isinstance(payload, dict):
        raise LedgerError(
            [
                _failure(
                    "retained ledger is not a JSON object",
                    expected="JSON object",
                    actual=type(payload).__name__,
                    repair="python3 scripts/check_rust_release_manifest.py",
                )
            ]
        )
    failures = validate_retained_ledger(payload)
    if failures:
        raise LedgerError(failures)
    return payload


def _native_summary(records: Sequence[Mapping[str, Any]]) -> dict[str, Any]:
    by_role: dict[str, Mapping[str, Any]] = {}
    failures: list[Failure] = []
    for record in records:
        role = record.get("role")
        if not isinstance(role, str) or not role:
            failures.append(
                _failure(
                    "macOS native record role is invalid",
                    expected="non-empty native package role",
                    actual=str(role),
                    repair="python3 scripts/check_rust_release_manifest.py",
                )
            )
            continue
        if role in by_role:
            failures.append(
                _failure(
                    "macOS native record role is duplicated",
                    expected="one record per native package role",
                    actual=str(role),
                    repair="python3 scripts/check_rust_release_manifest.py",
                )
            )
            continue
        by_role[str(role)] = record
    required_roles = {"root", "core", "speakers-analyze"}
    if not required_roles.issubset(by_role):
        failures.append(
            _failure(
                "macOS native record set is incomplete",
                expected="at least root, core, and speakers-analyze records",
                actual=", ".join(sorted(by_role)) or "<empty>",
                repair="python3 scripts/check_rust_release_manifest.py",
            )
        )
    if failures:
        raise LedgerError(failures)

    def summarize(record: Mapping[str, Any]) -> dict[str, Any]:
        signing = record.get("signing", {})
        return {
            "wheel": record.get("wheel"),
            "member": record.get("member"),
            "tools": record.get("tools"),
            "signing_mode": record.get("signing_mode"),
            "signer_pinned": signing.get("signer_pinned")
            if isinstance(signing, Mapping)
            else None,
            "team_pinned": signing.get("team_pinned")
            if isinstance(signing, Mapping)
            else None,
            "hardened_runtime": signing.get("hardened_runtime")
            if isinstance(signing, Mapping)
            else None,
            "trusted_timestamp": signing.get("trusted_timestamp")
            if isinstance(signing, Mapping)
            else None,
            "notarization_status": record.get("notarization_status"),
        }

    return {
        "macos_root_helper": summarize(by_role["root"]),
        "macos_core_script": summarize(by_role["core"]),
        "macos_speakers_analyze": summarize(by_role["speakers-analyze"]),
    }


def _macos_records_by_role(
    records: Sequence[Mapping[str, Any]],
) -> tuple[dict[str, Mapping[str, Any]], list[Failure]]:
    failures: list[Failure] = []
    by_role: dict[str, Mapping[str, Any]] = {}
    for record in records:
        role = record.get("role")
        if not isinstance(role, str) or not role:
            failures.append(
                _failure(
                    "macOS native record role is invalid",
                    expected="non-empty native package role",
                    actual=str(role),
                    repair="python3 scripts/check_rust_release_manifest.py",
                )
            )
            continue
        if role in by_role:
            failures.append(
                _failure(
                    "macOS native record role is duplicated",
                    expected="one record per native package role",
                    actual=str(role),
                    repair="python3 scripts/check_rust_release_manifest.py",
                )
            )
            continue
        by_role[str(role)] = record
    required_roles = {"root", "core", "speakers-analyze"}
    if not required_roles.issubset(by_role):
        failures.append(
            _failure(
                "macOS native record set is incomplete",
                expected="at least root, core, and speakers-analyze records",
                actual=", ".join(sorted(by_role)) or "<empty>",
                repair="python3 scripts/check_rust_release_manifest.py",
            )
        )
    return by_role, failures


def _wheel_member_entry(
    path: Path, *, member_name: str, label: str
) -> tuple[dict[str, Any] | None, list[Failure]]:
    failures: list[Failure] = []
    try:
        with zipfile.ZipFile(path) as wheel:
            members = [
                info for info in wheel.infolist() if info.filename == member_name
            ]
            if len(members) != 1:
                return None, [
                    _failure(
                        f"{label} native member count is wrong",
                        expected=f"exactly one {member_name}",
                        actual=str(len(members)),
                        repair="python3 scripts/check_rust_release_manifest.py",
                    )
                ]
            member = members[0]
            member_bytes = wheel.read(member)
    except (OSError, zipfile.BadZipFile) as exc:
        failures.append(
            _failure(
                f"{label} native member could not be read",
                expected="valid wheel with retained native member",
                actual=type(exc).__name__,
                repair="python3 scripts/check_rust_release_manifest.py",
            )
        )
        return None, failures
    return (
        {
            "path": member.filename,
            "sha256": hashlib.sha256(member_bytes).hexdigest(),
            "bytes": len(member_bytes),
        },
        [],
    )


def _core_members_from_wheel(
    path: Path,
) -> tuple[dict[str, dict[str, Any]], list[Failure]]:
    try:
        with zipfile.ZipFile(path) as wheel:
            scripts = core_wheel_script_members(wheel)
            script_names = {Path(info.filename).name for info in scripts}
            if len(scripts) != len(CORE_SCRIPT_NAMES) or script_names != set(
                CORE_SCRIPT_NAMES
            ):
                return {}, [
                    _failure(
                        "core wheel native member set is wrong",
                        expected=", ".join(
                            f".data/scripts/{name}" for name in CORE_SCRIPT_NAMES
                        ),
                        actual=", ".join(info.filename for info in scripts)
                        or "<empty>",
                        repair="python3 scripts/check_rust_release_manifest.py",
                    )
                ]
            entries: dict[str, dict[str, Any]] = {}
            for member in scripts:
                member_bytes = wheel.read(member)
                entries[Path(member.filename).name] = {
                    "path": member.filename,
                    "sha256": hashlib.sha256(member_bytes).hexdigest(),
                    "bytes": len(member_bytes),
                }
    except (OSError, zipfile.BadZipFile) as exc:
        return {}, [
            _failure(
                "core wheel native member could not be read",
                expected="valid core wheel with native script member",
                actual=type(exc).__name__,
                repair="python3 scripts/check_rust_release_manifest.py",
            )
        ]
    return entries, []


def _root_wheel_name_from_record(
    records_by_role: Mapping[str, Mapping[str, Any]],
) -> tuple[dict[str, Any] | None, list[Failure]]:
    record = records_by_role.get("root")
    wheel = record.get("wheel") if isinstance(record, Mapping) else None
    if not isinstance(wheel, Mapping) or not isinstance(wheel.get("name"), str):
        return None, [
            _failure(
                "macOS root native record wheel name is invalid",
                expected="root native record wheel name",
                actual=repr(wheel),
                repair="python3 scripts/check_rust_release_manifest.py",
            )
        ]
    return {"name": wheel["name"]}, []


def _rust_targets_from_payload(
    payload: Mapping[str, Any],
) -> tuple[dict[str, tuple[str, Any]], list[Failure]]:
    raw_targets = payload.get("rust_targets")
    if not isinstance(raw_targets, list):
        return {}, [
            _failure(
                "retained ledger Rust targets are invalid",
                expected="rust_targets list",
                actual=type(raw_targets).__name__,
                repair="python3 scripts/check_rust_release_manifest.py",
            )
        ]
    targets: dict[str, tuple[str, Any]] = {}
    failures: list[Failure] = []
    for index, item in enumerate(raw_targets):
        if not isinstance(item, Mapping):
            failures.append(
                _failure(
                    "retained ledger Rust target entry is invalid",
                    expected=f"rust_targets[{index}] object",
                    actual=type(item).__name__,
                    repair="python3 scripts/check_rust_release_manifest.py",
                )
            )
            continue
        artifact = item.get("artifact")
        lane = item.get("lane")
        if not isinstance(artifact, str) or not isinstance(lane, str):
            failures.append(
                _failure(
                    "retained ledger Rust target entry is invalid",
                    expected=f"rust_targets[{index}] artifact and lane strings",
                    actual=repr(item),
                    repair="python3 scripts/check_rust_release_manifest.py",
                )
            )
            continue
        targets[artifact] = (lane, item)
    return targets, failures


def _native_members_from_wheels(
    release_dir: Path,
    *,
    root_wheel_name: str,
    rust_targets_by_artifact: Mapping[str, tuple[str, Any]] | None = None,
) -> dict[str, dict[str, dict[str, Any]]]:
    failures: list[Failure] = []
    members: dict[str, dict[str, dict[str, Any]]] = {}
    targets = (
        rust_targets_by_artifact
        if rust_targets_by_artifact is not None
        else rust_artifact_targets()
    )
    for artifact, (lane, _target) in sorted(targets.items()):
        if lane == "source":
            continue
        core_members, core_failures = _core_members_from_wheel(release_dir / artifact)
        failures.extend(core_failures)
        if core_members:
            members.setdefault(lane, {}).update(core_members)
    helper_member, helper_failures = _wheel_member_entry(
        release_dir / root_wheel_name,
        member_name=PARAKEET_HELPER_MEMBER,
        label="macOS root wheel",
    )
    failures.extend(helper_failures)
    if helper_member is not None:
        members.setdefault("macos-arm64", {})["parakeet-helper"] = helper_member
    failures.extend(validate_native_members(members))
    if failures:
        raise LedgerError(failures)
    return members


def _native_members(
    release_dir: Path,
    native_records: Sequence[Mapping[str, Any]],
) -> dict[str, dict[str, dict[str, Any]]]:
    records_by_role, record_failures = _macos_records_by_role(native_records)
    root_wheel_name, root_name_failures = _root_wheel_name_from_record(records_by_role)
    failures = [*record_failures, *root_name_failures]
    if failures:
        raise LedgerError(failures)
    if root_wheel_name is None:
        raise AssertionError("root wheel name missing without failures")
    return _native_members_from_wheels(
        release_dir,
        root_wheel_name=str(root_wheel_name["name"]),
    )


def _root_wheel_name_from_ledger(
    payload: Mapping[str, Any],
) -> tuple[str | None, list[Failure]]:
    native_summary = payload.get("native_summary")
    root_summary = (
        native_summary.get("macos_root_helper")
        if isinstance(native_summary, Mapping)
        else None
    )
    wheel = root_summary.get("wheel") if isinstance(root_summary, Mapping) else None
    if not isinstance(wheel, Mapping) or not isinstance(wheel.get("name"), str):
        return None, [
            _failure(
                "retained ledger macOS root wheel name is invalid",
                expected="native_summary macOS root wheel name",
                actual=repr(wheel),
                repair="python3 scripts/check_rust_release_manifest.py",
            )
        ]
    return str(wheel["name"]), []


def validate_native_members_against_release_dir(
    release_dir: Path, payload: Mapping[str, Any]
) -> list[Failure]:
    root_wheel_name, failures = _root_wheel_name_from_ledger(payload)
    if failures:
        return failures
    if root_wheel_name is None:
        return []
    rust_targets_by_artifact, target_failures = _rust_targets_from_payload(payload)
    if target_failures:
        return target_failures
    try:
        actual = _native_members_from_wheels(
            release_dir,
            root_wheel_name=root_wheel_name,
            rust_targets_by_artifact=rust_targets_by_artifact,
        )
    except LedgerError as exc:
        return list(exc.failures)
    retained = payload.get("native_members")
    if actual != retained:
        return [
            _failure(
                "retained ledger native_members do not match finalized wheels",
                expected="native member path/hash map rederived from release payload",
                actual="retained native_members differ",
                repair="bash scripts/release.sh --recover",
            )
        ]
    return []


def build_ledger(
    *,
    version: str,
    source_commit: str,
    release_dir: Path,
    core_lock_path: Path,
    tool_evidence: Mapping[str, Mapping[str, str]],
    policy_run: PolicyRun,
    native_records: Sequence[Mapping[str, Any]],
    models: Mapping[str, str],
    nvattest: Mapping[str, Any],
) -> dict[str, Any]:
    failures: list[Failure] = []
    schema_version = CURRENT_LEDGER_SCHEMA_VERSION
    schema = _registered_ledger_schema(schema_version)
    if not SOURCE_COMMIT_RE.fullmatch(source_commit):
        failures.append(
            _failure(
                "ledger source commit is invalid",
                expected="full lowercase commit",
                actual=source_commit,
                repair="python3 scripts/check_rust_release_manifest.py",
            )
        )
    core_lock_sha256, _core_lock_bytes = file_sha256_size(core_lock_path)
    files = _candidate_files(release_dir)
    candidate = {
        "path": CANDIDATE,
        "file_count": len(files),
        "package_file_count": sum(
            1
            for item in files
            if not item["name"].endswith(".rust-release-manifest.json")
        ),
        "manifest_file_count": sum(
            1 for item in files if item["name"].endswith(".rust-release-manifest.json")
        ),
        "candidate_digest": candidate_digest(release_dir),
        "files": files,
    }
    try:
        native_summary = _native_summary(native_records)
    except LedgerError as exc:
        failures.extend(exc.failures)
        native_summary = {}
    try:
        native_members = _native_members(release_dir, native_records)
    except LedgerError as exc:
        failures.extend(exc.failures)
        native_members = {}
    failures.extend(validate_models_payload(models, candidate, schema=schema))
    failures.extend(validate_nvattest_payload(nvattest, schema=schema))
    failures.extend(_validate_tool_evidence(tool_evidence))
    if failures:
        raise LedgerError(failures)

    ledger: dict[str, Any] = {
        "schema_version": schema_version,
        "kind": "solstone-release-ledger",
        "product": "solstone",
        "version": version,
        "source_commit": source_commit,
        "candidate": candidate,
        "models": {key: models[key] for key in sorted(models)},
        "core_lock_sha256": core_lock_sha256,
        "rust_targets": _rust_targets(),
        "tool_evidence": {
            lane: dict(tool_evidence[lane]) for lane in sorted(tool_evidence)
        },
        "native_members": native_members,
        "dependency_policy": policy_run.manifest_dependency_policy(),
        "policy_run": _policy_run_payload(policy_run),
        "native_summary": native_summary,
        "proofs": {"expected_targets": list(PROOF_TARGETS)},
        "nvattest": {
            "authority": nvattest["authority"],
            "authority_sha256": nvattest["authority_sha256"],
            "challenge": nvattest["challenge"],
            "support_distributions": [
                dict(entry) for entry in nvattest["support_distributions"]
            ],
        },
        "redaction": {"validator": "recursive-key-value-public-evidence"},
    }
    if set(ledger) != schema.top_level_keys:
        raise AssertionError("ledger top-level key set drifted")
    public_failures = validate_public_evidence_tree("ledger", ledger)
    if public_failures:
        raise LedgerError(public_failures)
    return ledger


def write_ledger(
    *,
    evidence_root: Path,
    version: str,
    source_commit: str,
    release_dir: Path,
    core_lock_path: Path,
    tool_evidence: Mapping[str, Mapping[str, str]],
    policy_run: PolicyRun,
    native_records: Sequence[Mapping[str, Any]],
    models: Mapping[str, str],
    nvattest: Mapping[str, Any],
    output_dir: Path | None = None,
) -> Path:
    ledger = build_ledger(
        version=version,
        source_commit=source_commit,
        release_dir=release_dir,
        core_lock_path=core_lock_path,
        tool_evidence=tool_evidence,
        policy_run=policy_run,
        native_records=native_records,
        models=models,
        nvattest=nvattest,
    )
    resolved_output_dir = output_dir or (evidence_root / version)
    resolved_output_dir.mkdir(parents=True, exist_ok=True)
    output_path = resolved_output_dir / "ledger.json"
    payload = canonical_json_bytes(ledger)
    temp_path = output_path.with_name(f".{output_path.name}.tmp")
    try:
        with temp_path.open("wb") as handle:
            handle.write(payload)
            handle.flush()
            os.fsync(handle.fileno())
        os.rename(temp_path, output_path)
    finally:
        temp_path.unlink(missing_ok=True)
    readback = json.loads(output_path.read_text(encoding="utf-8"))
    failures = validate_retained_ledger(readback)
    if failures:
        raise LedgerError(failures)
    return output_path
