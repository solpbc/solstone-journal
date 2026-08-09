#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Write normalized macOS native wheel records for release evidence."""

from __future__ import annotations

import argparse
import json
import os
import zipfile
from collections.abc import Mapping, Sequence
from pathlib import Path
from typing import Any

from scripts.check_rust_release_manifest import (
    SHA256_RE,
    SOURCE_COMMIT_RE,
    Failure,
    _format_failures,
    canonical_json_bytes,
)
from scripts.check_wheel_contents import (
    CORE_SCRIPT_NAMES,
    PARAKEET_HELPER_MEMBER,
    SPEAKERS_ANALYZE_RUNTIME_INSTALL_DIR,
    SPEAKERS_ANALYZE_SCRIPT_NAMES,
    core_wheel_script_members,
)
from scripts.release_digest import file_sha256_size
from scripts.release_package_inventory import (
    NativePackage,
    load_release_package_inventory,
    native_role,
    normalized_distribution,
)
from scripts.release_public_evidence import validate_public_evidence_tree
from scripts.release_tool_pins import (
    MACOS_CODESIGN_PUBLIC_PIN,
    MACOS_NOTARYTOOL_PIN,
    MACOS_SIGNING_MODE,
    MACOS_SWIFT_PIN,
    MACOS_XCODE_PIN,
    PYTHON_MACOS_VERSION,
    tool_value_matches_pin,
)
from scripts.stage_speakers_analyze_runtime import TARGETS as SPEAKERS_ANALYZE_TARGETS

NativeRole = str

KIND = "macos-native-record/v1"
TARGET = {
    "triple": "aarch64-apple-darwin",
    "profile": "release",
    "features": [],
}
TOP_LEVEL_KEYS = frozenset(
    (
        "schema_version",
        "kind",
        "source_commit",
        "core_lock_sha256",
        "role",
        "target",
        "wheel",
        "member",
        "members",
        "unsigned_members",
        "tools",
        "signing_mode",
        "signing",
        "notarization_status",
    )
)
SIGNING_KEYS = frozenset(
    ("signer_pinned", "team_pinned", "hardened_runtime", "trusted_timestamp")
)
FACT_KEYS = SIGNING_KEYS | frozenset(
    ("signed_binary_sha256", "unsigned_binary_sha256", "notarization_status", "tools")
)
FACT_TOOL_KEYS = frozenset(("xcode", "swift", "codesign", "notarytool"))


class NativeRecordError(RuntimeError):
    def __init__(self, failures: Sequence[Failure]) -> None:
        self.failures = tuple(failures)
        super().__init__("; ".join(failure.error for failure in self.failures))


def _failure(error: str, *, expected: str, actual: str, repair: str) -> Failure:
    return Failure(error=error, expected=expected, actual=actual, repair=repair)


def _native_package_for_role(role: NativeRole) -> NativePackage | None:
    if role == "root":
        return None
    matches = [
        package
        for package in load_release_package_inventory().macos_native_packages
        if native_role(package) == role
    ]
    return matches[0] if len(matches) == 1 else None


def _members_for_role(
    wheel: zipfile.ZipFile, role: NativeRole
) -> dict[str, zipfile.ZipInfo] | None:
    if role == "root":
        helpers = [
            info for info in wheel.infolist() if info.filename == PARAKEET_HELPER_MEMBER
        ]
        return {"parakeet-helper": helpers[0]} if len(helpers) == 1 else None
    if role == "core":
        scripts = core_wheel_script_members(wheel)
        names = {Path(info.filename).name for info in scripts}
        if len(scripts) != len(CORE_SCRIPT_NAMES) or names != set(CORE_SCRIPT_NAMES):
            return None
        return {Path(info.filename).name: info for info in scripts}

    if role != "speakers-analyze":
        package = _native_package_for_role(role)
        if package is None:
            return None
        script_members = [
            info
            for info in wheel.infolist()
            if info.filename.endswith(f".data/scripts/{package.binary}")
        ]
        return {package.binary: script_members[0]} if len(script_members) == 1 else None

    script_members = [
        info
        for info in wheel.infolist()
        if info.filename.endswith(f".data/scripts/{SPEAKERS_ANALYZE_SCRIPT_NAMES[0]}")
    ]
    dylib_name = _speakers_analyze_dylib_name()
    dylib_suffix = (
        f".data/{SPEAKERS_ANALYZE_RUNTIME_INSTALL_DIR.as_posix()}/{dylib_name}"
    )
    dylib_members = [
        info for info in wheel.infolist() if info.filename.endswith(dylib_suffix)
    ]
    if len(script_members) != 1 or len(dylib_members) != 1:
        return None
    return {
        SPEAKERS_ANALYZE_SCRIPT_NAMES[0]: script_members[0],
        dylib_name: dylib_members[0],
    }


def _expected_member_path(role: NativeRole) -> str:
    if role == "root":
        return PARAKEET_HELPER_MEMBER
    if role == "core":
        return ", ".join(f".data/scripts/{name}" for name in CORE_SCRIPT_NAMES)
    if role != "speakers-analyze":
        package = _native_package_for_role(role)
        return (
            f".data/scripts/{package.binary}"
            if package is not None
            else "known packaged native binary"
        )
    dylib_name = _speakers_analyze_dylib_name()
    return (
        f".data/scripts/{SPEAKERS_ANALYZE_SCRIPT_NAMES[0]} and "
        f".data/{SPEAKERS_ANALYZE_RUNTIME_INSTALL_DIR.as_posix()}/{dylib_name}"
    )


def _role_matches_wheel(role: NativeRole, wheel_name: str) -> bool:
    if role == "root":
        return wheel_name.startswith("solstone-") and wheel_name.endswith(".whl")
    if role == "core":
        return wheel_name.startswith("solstone_core-") and wheel_name.endswith(".whl")
    if role == "speakers-analyze":
        return wheel_name.startswith(
            "solstone_core_speakers_analyze-"
        ) and wheel_name.endswith(".whl")
    package = _native_package_for_role(role)
    return (
        package is not None
        and wheel_name.startswith(f"{normalized_distribution(package.distribution)}-")
        and wheel_name.endswith(".whl")
    )


def _primary_member_name(role: NativeRole) -> str:
    if role == "root":
        return "parakeet-helper"
    if role == "core":
        return "solstone-core"
    if role == "speakers-analyze":
        return SPEAKERS_ANALYZE_SCRIPT_NAMES[0]
    package = _native_package_for_role(role)
    if package is None:
        raise ValueError(f"unknown native role: {role}")
    return package.binary


def _speakers_analyze_dylib_name() -> str:
    return SPEAKERS_ANALYZE_TARGETS["macos-arm64"].runtime_staged_name


def _read_members(
    wheel_path: Path, role: NativeRole
) -> dict[str, tuple[str, bytes]] | list[Failure]:
    if wheel_path.is_symlink():
        return [
            _failure(
                "macOS native wheel is a symlink",
                expected="regular wheel file",
                actual=wheel_path.name,
                repair="python3 scripts/check_rust_release_manifest.py",
            )
        ]
    if not _role_matches_wheel(role, wheel_path.name):
        return [
            _failure(
                "macOS native record role does not match wheel name",
                expected=f"{role} wheel name",
                actual=wheel_path.name,
                repair="python3 scripts/check_rust_release_manifest.py",
            )
        ]
    with zipfile.ZipFile(wheel_path) as wheel:
        members = _members_for_role(wheel, role)
        if members is None:
            return [
                _failure(
                    "macOS native wheel member count is wrong",
                    expected=f"exactly {_expected_member_path(role)}",
                    actual="missing or duplicate",
                    repair="python3 scripts/check_rust_release_manifest.py",
                )
            ]
        return {
            name: (member.filename, wheel.read(member))
            for name, member in sorted(members.items())
        }


def _sha256_bytes(data: bytes) -> str:
    import hashlib

    return hashlib.sha256(data).hexdigest()


def _load_facts(path: Path) -> Mapping[str, Any]:
    with path.open(encoding="utf-8") as handle:
        data = json.load(handle)
    if not isinstance(data, Mapping):
        raise NativeRecordError(
            [
                _failure(
                    "macOS signing facts are not an object",
                    expected="JSON object",
                    actual=type(data).__name__,
                    repair="python3 scripts/check_rust_release_manifest.py",
                )
            ]
        )
    return data


def _validate_facts(facts: Mapping[str, Any]) -> list[Failure]:
    failures: list[Failure] = []
    if set(facts) != FACT_KEYS:
        failures.append(
            _failure(
                "macOS signing facts key set is wrong",
                expected=", ".join(sorted(FACT_KEYS)),
                actual=", ".join(sorted(str(key) for key in facts)),
                repair="python3 scripts/check_rust_release_manifest.py",
            )
        )
    for key, label in (
        ("signed_binary_sha256", "signed"),
        ("unsigned_binary_sha256", "unsigned"),
    ):
        digest = facts.get(key)
        if not isinstance(digest, str) or not SHA256_RE.fullmatch(digest):
            failures.append(
                _failure(
                    f"macOS {label} binary hash is invalid",
                    expected="lowercase SHA-256",
                    actual=str(digest),
                    repair="python3 scripts/check_rust_release_manifest.py",
                )
            )
    for key in SIGNING_KEYS:
        if facts.get(key) is not True:
            failures.append(
                _failure(
                    f"macOS signing fact {key} is not pinned true",
                    expected="true",
                    actual=repr(facts.get(key)),
                    repair="python3 scripts/check_rust_release_manifest.py",
                )
            )
    if facts.get("notarization_status") != "accepted":
        failures.append(
            _failure(
                "macOS notarization status is not accepted",
                expected="accepted",
                actual=str(facts.get("notarization_status")),
                repair="python3 scripts/check_rust_release_manifest.py",
            )
        )
    tools = facts.get("tools")
    if not isinstance(tools, Mapping) or set(tools) != FACT_TOOL_KEYS:
        failures.append(
            _failure(
                "macOS signing tool facts key set is wrong",
                expected=", ".join(sorted(FACT_TOOL_KEYS)),
                actual=(
                    ", ".join(sorted(str(key) for key in tools))
                    if isinstance(tools, Mapping)
                    else type(tools).__name__
                ),
                repair="python3 scripts/check_rust_release_manifest.py",
            )
        )
    else:
        expected_tools = {
            "xcode": MACOS_XCODE_PIN,
            "swift": MACOS_SWIFT_PIN,
            "codesign": MACOS_CODESIGN_PUBLIC_PIN,
            "notarytool": MACOS_NOTARYTOOL_PIN,
        }
        for key, expected in expected_tools.items():
            if not tool_value_matches_pin(key, expected, tools.get(key)):
                failures.append(
                    _failure(
                        f"macOS {key} tool evidence is not pinned",
                        expected=expected,
                        actual=str(tools.get(key)),
                        repair="python3 scripts/check_rust_release_manifest.py",
                    )
                )
    failures.extend(validate_public_evidence_tree("macos_signing_facts", facts))
    return failures


def _facts_by_member(
    role: NativeRole,
    signing_facts: Mapping[str, Any],
) -> tuple[dict[str, Mapping[str, Any]], list[Failure]]:
    if role == "root":
        return {"parakeet-helper": signing_facts}, _validate_facts(signing_facts)

    if set(signing_facts) != {"members"}:
        return {}, [
            _failure(
                f"macOS {role} signing facts key set is wrong",
                expected="members",
                actual=", ".join(sorted(str(key) for key in signing_facts))
                or "<empty>",
                repair="python3 scripts/check_rust_release_manifest.py",
            )
        ]
    members = signing_facts.get("members")
    if not isinstance(members, Mapping):
        return {}, [
            _failure(
                f"macOS {role} signing facts are missing members",
                expected=f"members object keyed by {_expected_member_path(role)}",
                actual=type(members).__name__,
                repair="python3 scripts/check_rust_release_manifest.py",
            )
        ]
    if role == "core":
        expected_names = set(CORE_SCRIPT_NAMES)
    elif role == "speakers-analyze":
        expected_names = {
            SPEAKERS_ANALYZE_SCRIPT_NAMES[0],
            SPEAKERS_ANALYZE_TARGETS["macos-arm64"].runtime_staged_name,
        }
    else:
        package = _native_package_for_role(role)
        expected_names = {package.binary} if package is not None else set()
    if set(members) != expected_names:
        return {}, [
            _failure(
                f"macOS {role} signing facts member set is wrong",
                expected=", ".join(sorted(expected_names)),
                actual=", ".join(sorted(str(key) for key in members)) or "<empty>",
                repair="python3 scripts/check_rust_release_manifest.py",
            )
        ]
    failures: list[Failure] = []
    normalized: dict[str, Mapping[str, Any]] = {}
    for name in sorted(expected_names):
        facts = members.get(name)
        if not isinstance(facts, Mapping):
            failures.append(
                _failure(
                    f"macOS {role} signing facts for {name} are invalid",
                    expected="JSON object",
                    actual=type(facts).__name__,
                    repair="python3 scripts/check_rust_release_manifest.py",
                )
            )
            continue
        failures.extend(_validate_facts(facts))
        normalized[name] = facts
    return normalized, failures


def _member_entry(member_path: str, member_bytes: bytes) -> dict[str, Any]:
    return {
        "path": member_path,
        "sha256": _sha256_bytes(member_bytes),
        "bytes": len(member_bytes),
    }


def build_macos_native_record(
    *,
    role: NativeRole,
    wheel_path: Path,
    signing_facts: Mapping[str, Any],
    source_commit: str,
    core_lock_sha256: str,
    python_version: str = PYTHON_MACOS_VERSION,
) -> dict[str, Any]:
    facts_by_member, failures = _facts_by_member(role, signing_facts)
    if not SOURCE_COMMIT_RE.fullmatch(source_commit):
        failures.append(
            _failure(
                "macOS native record source commit is invalid",
                expected="full lowercase commit",
                actual=source_commit,
                repair="python3 scripts/check_rust_release_manifest.py",
            )
        )
    if not SHA256_RE.fullmatch(core_lock_sha256):
        failures.append(
            _failure(
                "macOS native record core lock hash is invalid",
                expected="lowercase SHA-256",
                actual=core_lock_sha256,
                repair="python3 scripts/check_rust_release_manifest.py",
            )
        )
    if python_version != PYTHON_MACOS_VERSION:
        failures.append(
            _failure(
                "macOS Python evidence is not pinned",
                expected=PYTHON_MACOS_VERSION,
                actual=python_version,
                repair="python3 scripts/check_rust_release_manifest.py",
            )
        )
    wheel_members = _read_members(wheel_path, role)
    if isinstance(wheel_members, list):
        failures.extend(wheel_members)
        member_payloads = {}
    else:
        member_payloads = {
            name: _member_entry(member_path, member_bytes)
            for name, (member_path, member_bytes) in wheel_members.items()
        }
        for name, (_member_path, member_bytes) in wheel_members.items():
            facts = facts_by_member.get(name, {})
            signed_sha256 = facts.get("signed_binary_sha256")
            member_sha256 = _sha256_bytes(member_bytes)
            if isinstance(signed_sha256, str) and member_sha256 != signed_sha256:
                failures.append(
                    _failure(
                        "macOS signed binary hash does not match final wheel member",
                        expected=signed_sha256,
                        actual=member_sha256,
                        repair="python3 scripts/check_rust_release_manifest.py",
                    )
                )
    if facts_by_member:
        tool_payloads = {
            canonical_json_bytes(facts["tools"])
            for facts in facts_by_member.values()
            if isinstance(facts.get("tools"), Mapping)
        }
        if len(tool_payloads) != 1:
            failures.append(
                _failure(
                    f"macOS signing tool facts differ across {role} members",
                    expected=f"identical signing tool facts for every {role} member",
                    actual=str(len(tool_payloads)),
                    repair="python3 scripts/check_rust_release_manifest.py",
                )
            )
    if failures:
        raise NativeRecordError(failures)

    wheel_sha256, wheel_bytes = file_sha256_size(wheel_path)
    primary_name = _primary_member_name(role)
    primary_facts = facts_by_member[primary_name]
    tools = primary_facts["tools"]
    record: dict[str, Any] = {
        "schema_version": 1,
        "kind": KIND,
        "source_commit": source_commit,
        "core_lock_sha256": core_lock_sha256,
        "role": role,
        "target": dict(TARGET),
        "wheel": {
            "name": wheel_path.name,
            "sha256": wheel_sha256,
            "bytes": wheel_bytes,
        },
        "member": member_payloads[primary_name],
        "members": {key: member_payloads[key] for key in sorted(member_payloads)},
        "unsigned_members": {
            key: facts_by_member[key]["unsigned_binary_sha256"]
            for key in sorted(member_payloads)
        },
        "tools": {
            "python": python_version,
            "xcode": tools["xcode"],
            "swift": tools["swift"],
            "codesign": tools["codesign"],
            "notarytool": tools["notarytool"],
        },
        "signing_mode": MACOS_SIGNING_MODE,
        "signing": {key: primary_facts[key] for key in sorted(SIGNING_KEYS)},
        "notarization_status": primary_facts["notarization_status"],
    }
    record_failures = validate_macos_native_record(
        record,
        role=role,
        wheel_path=wheel_path,
        source_commit=source_commit,
        core_lock_sha256=core_lock_sha256,
    )
    if record_failures:
        raise NativeRecordError(record_failures)
    return record


def validate_macos_native_record(
    record: Mapping[str, Any],
    *,
    role: NativeRole,
    wheel_path: Path,
    source_commit: str,
    core_lock_sha256: str,
) -> list[Failure]:
    failures: list[Failure] = []
    if set(record) != TOP_LEVEL_KEYS:
        failures.append(
            _failure(
                "macOS native record key set is wrong",
                expected=", ".join(sorted(TOP_LEVEL_KEYS)),
                actual=", ".join(sorted(str(key) for key in record)),
                repair="python3 scripts/check_rust_release_manifest.py",
            )
        )
    expected_scalars = {
        "schema_version": 1,
        "kind": KIND,
        "source_commit": source_commit,
        "core_lock_sha256": core_lock_sha256,
        "role": role,
        "target": TARGET,
        "signing_mode": MACOS_SIGNING_MODE,
        "notarization_status": "accepted",
    }
    for key, expected in expected_scalars.items():
        if record.get(key) != expected:
            failures.append(
                _failure(
                    f"macOS native record {key} is wrong",
                    expected=repr(expected),
                    actual=repr(record.get(key)),
                    repair="python3 scripts/check_rust_release_manifest.py",
                )
            )
    signing = record.get("signing")
    if not isinstance(signing, Mapping) or set(signing) != SIGNING_KEYS:
        failures.append(
            _failure(
                "macOS native record signing key set is wrong",
                expected=", ".join(sorted(SIGNING_KEYS)),
                actual=(
                    ", ".join(sorted(str(key) for key in signing))
                    if isinstance(signing, Mapping)
                    else type(signing).__name__
                ),
                repair="python3 scripts/check_rust_release_manifest.py",
            )
        )
    else:
        for key in SIGNING_KEYS:
            if signing.get(key) is not True:
                failures.append(
                    _failure(
                        f"macOS native record signing {key} is not true",
                        expected="true",
                        actual=repr(signing.get(key)),
                        repair="python3 scripts/check_rust_release_manifest.py",
                    )
                )
    tools = record.get("tools")
    expected_tools = {
        "python": PYTHON_MACOS_VERSION,
        "xcode": MACOS_XCODE_PIN,
        "swift": MACOS_SWIFT_PIN,
        "codesign": MACOS_CODESIGN_PUBLIC_PIN,
        "notarytool": MACOS_NOTARYTOOL_PIN,
    }
    if not isinstance(tools, Mapping) or set(tools) != set(expected_tools):
        failures.append(
            _failure(
                "macOS native record tools key set is wrong",
                expected=", ".join(sorted(expected_tools)),
                actual=(
                    ", ".join(sorted(str(key) for key in tools))
                    if isinstance(tools, Mapping)
                    else type(tools).__name__
                ),
                repair="python3 scripts/check_rust_release_manifest.py",
            )
        )
    else:
        for key, expected in expected_tools.items():
            if not tool_value_matches_pin(key, expected, tools.get(key)):
                failures.append(
                    _failure(
                        f"macOS native record tool {key} is not pinned",
                        expected=expected,
                        actual=str(tools.get(key)),
                        repair="python3 scripts/check_rust_release_manifest.py",
                    )
                )

    wheel_members = _read_members(wheel_path, role)
    if isinstance(wheel_members, list):
        failures.extend(wheel_members)
    else:
        expected_members = {
            name: _member_entry(member_path, member_bytes)
            for name, (member_path, member_bytes) in wheel_members.items()
        }
        primary_name = _primary_member_name(role)
        expected_member = expected_members[primary_name]
        if record.get("member") != expected_member:
            failures.append(
                _failure(
                    "macOS native record member does not match wheel",
                    expected=repr(expected_member),
                    actual=repr(record.get("member")),
                    repair="python3 scripts/check_rust_release_manifest.py",
                )
            )
        if record.get("members") != expected_members:
            failures.append(
                _failure(
                    "macOS native record members do not match wheel",
                    expected=repr(expected_members),
                    actual=repr(record.get("members")),
                    repair="python3 scripts/check_rust_release_manifest.py",
                )
            )
    expected_wheel_sha256, expected_wheel_bytes = file_sha256_size(wheel_path)
    expected_wheel = {
        "name": wheel_path.name,
        "sha256": expected_wheel_sha256,
        "bytes": expected_wheel_bytes,
    }
    if record.get("wheel") != expected_wheel:
        failures.append(
            _failure(
                "macOS native record wheel does not match final wheel",
                expected=repr(expected_wheel),
                actual=repr(record.get("wheel")),
                repair="python3 scripts/check_rust_release_manifest.py",
            )
        )
    unsigned_members = record.get("unsigned_members")
    record_members = record.get("members")
    if not isinstance(unsigned_members, Mapping):
        failures.append(
            _failure(
                "macOS native record unsigned members are invalid",
                expected="unsigned_members object keyed like members",
                actual=type(unsigned_members).__name__,
                repair="python3 scripts/check_rust_release_manifest.py",
            )
        )
    elif not isinstance(record_members, Mapping) or set(unsigned_members) != set(
        record_members
    ):
        expected_names = (
            ", ".join(sorted(str(key) for key in record_members))
            if isinstance(record_members, Mapping)
            else "<invalid members>"
        )
        failures.append(
            _failure(
                "macOS native record unsigned member set is wrong",
                expected=expected_names,
                actual=", ".join(sorted(str(key) for key in unsigned_members))
                or "<empty>",
                repair="python3 scripts/check_rust_release_manifest.py",
            )
        )
    else:
        invalid_unsigned = [
            str(name)
            for name, digest in unsigned_members.items()
            if not isinstance(digest, str) or not SHA256_RE.fullmatch(digest)
        ]
        if invalid_unsigned:
            failures.append(
                _failure(
                    "macOS native record unsigned member hash is invalid",
                    expected="lowercase SHA-256 for every unsigned member",
                    actual=", ".join(sorted(invalid_unsigned)),
                    repair="python3 scripts/check_rust_release_manifest.py",
                )
            )
        if role == "speakers-analyze":
            dylib_name = _speakers_analyze_dylib_name()
            expected_unsigned = SPEAKERS_ANALYZE_TARGETS["macos-arm64"].runtime_sha256
            actual_unsigned = unsigned_members.get(dylib_name)
            if actual_unsigned != expected_unsigned:
                failures.append(
                    _failure(
                        (
                            "macOS speakers-analyze unsigned ONNX Runtime hash "
                            "does not match staged pin"
                        ),
                        expected=expected_unsigned,
                        actual=str(actual_unsigned),
                        repair="python3 scripts/stage_speakers_analyze_runtime.py --target macos-arm64",
                    )
                )
    failures.extend(validate_public_evidence_tree("macos_native_record", record))
    return failures


def write_macos_native_record(
    *,
    role: NativeRole,
    wheel_path: Path,
    signing_facts_path: Path,
    output_path: Path,
    source_commit: str,
    core_lock_sha256: str,
    python_version: str = PYTHON_MACOS_VERSION,
) -> Path:
    record = build_macos_native_record(
        role=role,
        wheel_path=wheel_path,
        signing_facts=_load_facts(signing_facts_path),
        source_commit=source_commit,
        core_lock_sha256=core_lock_sha256,
        python_version=python_version,
    )
    payload = canonical_json_bytes(record)
    output_path.parent.mkdir(parents=True, exist_ok=True)
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
    failures = validate_macos_native_record(
        readback,
        role=role,
        wheel_path=wheel_path,
        source_commit=source_commit,
        core_lock_sha256=core_lock_sha256,
    )
    if failures:
        raise NativeRecordError(failures)
    return output_path


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--role", required=True)
    parser.add_argument("--wheel", type=Path, required=True)
    parser.add_argument("--signing-facts", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--source-commit", required=True)
    parser.add_argument("--core-lock-sha256", required=True)
    parser.add_argument("--python-version", default=PYTHON_MACOS_VERSION)
    args = parser.parse_args(argv)
    try:
        write_macos_native_record(
            role=args.role,
            wheel_path=args.wheel,
            signing_facts_path=args.signing_facts,
            output_path=args.out,
            source_commit=args.source_commit,
            core_lock_sha256=args.core_lock_sha256,
            python_version=args.python_version,
        )
    except NativeRecordError as exc:
        _format_failures(exc.failures)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
