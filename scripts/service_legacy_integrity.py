#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Fail-closed integrity helpers for the service-generator evidence corpus.

This module is imported by the hand-run capture transaction and is also exposed
as a small command so the standalone Rust evidence gate can exercise the same
production checks against controlled fixtures.
"""

from __future__ import annotations

import argparse
import base64
import binascii
import hashlib
import json
import plistlib
import re
import subprocess
import tempfile
import tomllib
import urllib.parse
from collections import Counter
from dataclasses import dataclass
from pathlib import Path, PurePosixPath
from typing import Any, Iterable, Iterator

CANONICAL_REPOSITORY = "https://github.com/solpbc/solstone-journal.git"
SEMANTIC_BASE_COMMIT = "f3d90d1c83ab3b31587b4e7cf5f3019b403db3e7"
NON_OWNED_SOURCE_PATHS = (
    "packages/solstone-journal/scripts/journal",
    "core/crates/solstone-core-journal-cli/src/processes.rs",
    "core/crates/solstone-core-journal-cli/src/runner.rs",
    "core/crates/solstone-core-journal-cli/src/lib.rs",
)
SYNTHETIC_ROOT = PurePosixPath("/opt/solstone-service-legacy-evidence")
EXPECTED_FIXTURES = 340
EXPECTED_ROLE_OCCURRENCES = 4_076
EXPECTED_TAGS = 66
EXPECTED_FOLLOW = 44
EXPECTED_ROLE_ORACLE_SHA256 = (
    "5a77ea6608a9c51baa9ad1c58a98f48cc554c2c2c3ff1219ba77c02585e20be7"
)
GIT_HELPER_LINKS = (
    Path("/usr/libexec/git-core/git-remote-https"),
    Path("/usr/lib/git-core/git-remote-https"),
)
HOST_TOOL_PATHS = {
    "ar": "/usr/bin/ar",
    "as": "/usr/bin/as",
    "cc": "/usr/bin/cc",
    "ld": "/usr/bin/ld",
    "sh": "/bin/sh",
    "strip": "/usr/bin/strip",
}

DUMMY_FIELDS = {
    "ANTHROPIC_API_KEY": "__SERVICE_LEGACY_DUMMY_ANTHROPIC__",
    "OPENAI_API_KEY": "__SERVICE_LEGACY_DUMMY_OPENAI__",
    "GOOGLE_API_KEY": "__SERVICE_LEGACY_DUMMY_GOOGLE__",
    "REVAI_ACCESS_TOKEN": "__SERVICE_LEGACY_DUMMY_REVAI__",
    "PLAUD_ACCESS_TOKEN": "__SERVICE_LEGACY_DUMMY_PLAUD__",
}

TOKEN_PATTERNS = {
    "openai_anthropic": re.compile(
        r"(?<![A-Za-z0-9_-])sk-(?:ant-|proj-|svcacct-)?[A-Za-z0-9_-]{20,256}(?![A-Za-z0-9_-])"
    ),
    "google": re.compile(r"(?<![0-9A-Za-z_-])AIza[0-9A-Za-z_-]{35}(?![0-9A-Za-z_-])"),
    "github": re.compile(
        r"(?<![A-Za-z0-9_])(?:gh[pousr]_|github_pat_)[A-Za-z0-9_]{20,255}(?![A-Za-z0-9_])"
    ),
    "aws": re.compile(r"(?<![0-9A-Z])AKIA[0-9A-Z]{16}(?![0-9A-Z])"),
}
JWT_PATTERN = re.compile(
    r"(?<![A-Za-z0-9_-])([A-Za-z0-9_-]{2,4096})\.([A-Za-z0-9_-]{2,4096})\.([A-Za-z0-9_-]{2,4096})(?![A-Za-z0-9_-])"
)
ABSOLUTE_PATH = re.compile(r"(?<![A-Za-z0-9_.:/-])/(?:[^\s\x00\"'<>]|\\ )+")
OPERATOR_HOME_PATH = re.compile(
    r"""(?:^|[\s=:"'(\[+@!-]|file://(?:localhost)?)/(?:home|Users)/"""
)
ESCAPED_CODEPOINT = re.compile(
    r"\\+(?:u([0-9a-fA-F]{4})|U([0-9a-fA-F]{8})|x([0-9a-fA-F]{2}))"
)


class IntegrityError(RuntimeError):
    """Evidence failed a named, fail-closed integrity guard."""

    def __init__(self, guard: str, detail: str):
        super().__init__(f"{guard}: {detail}")
        self.guard = guard


def contains_operator_path(text: str) -> bool:
    normalized = text
    while True:
        unescaped = re.sub(r"\\+/", "/", normalized)
        unescaped = ESCAPED_CODEPOINT.sub(
            lambda match: chr(
                int(match.group(1) or match.group(2) or match.group(3), 16)
            ),
            unescaped,
        )
        unescaped = urllib.parse.unquote(unescaped)
        if unescaped == normalized:
            break
        normalized = unescaped
    return (
        bool(OPERATOR_HOME_PATH.search(normalized)) or ".hopper/worktrees" in normalized
    )


def installed_git_helper_targets() -> set[Path]:
    targets = {path.resolve() for path in GIT_HELPER_LINKS if path.is_file()}
    if not targets:
        raise IntegrityError("git-tool", "pinned HTTPS helper is unavailable")
    return targets


def verified_host_tool_paths(packaging: dict[str, Any]) -> set[str]:
    try:
        host_tools = packaging["build"]["bundle"]["host_tools"]
    except (KeyError, TypeError) as exc:
        raise IntegrityError("host-tools", "packaging host tools are absent") from exc
    if not isinstance(host_tools, dict) or set(host_tools) != set(HOST_TOOL_PATHS):
        raise IntegrityError("host-tools", "packaging host-tool roles are not exact")
    for role, expected_path in HOST_TOOL_PATHS.items():
        row = host_tools.get(role)
        path = Path(expected_path)
        if (
            not isinstance(row, dict)
            or row.get("selected_path") != expected_path
            or not path.is_file()
            or row.get("sha256") != sha256_file(path.resolve())
        ):
            raise IntegrityError("host-tools", f"packaging {role} fact differs")
    return set(HOST_TOOL_PATHS.values())


def canonical_bytes(value: Any) -> bytes:
    return json.dumps(
        value, sort_keys=True, separators=(",", ":"), ensure_ascii=False
    ).encode("utf-8")


def sha256_bytes(value: bytes) -> str:
    return hashlib.sha256(value).hexdigest()


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def read_json(path: Path) -> Any:
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeDecodeError, json.JSONDecodeError) as exc:
        raise IntegrityError("json", f"cannot strictly parse {path}: {exc}") from exc


def decode_plist(encoded: str, label: str) -> Any:
    try:
        raw = base64.b64decode(encoded, validate=True)
        return plistlib.loads(raw)
    except (ValueError, binascii.Error, plistlib.InvalidFileException) as exc:  # type: ignore[name-defined]
        raise IntegrityError("plist", f"cannot strictly decode {label}: {exc}") from exc


def synthetic_executable(bucket: str) -> str:
    if bucket not in {"cpython37", "cpython39"}:
        raise IntegrityError("interpreter", f"unknown interpreter bucket {bucket!r}")
    return str(SYNTHETIC_ROOT / "interpreters" / bucket / "bin" / "python")


def expected_roots(blob: str, profile: str) -> tuple[str, str]:
    if not re.fullmatch(r"[0-9a-f]{40}", blob):
        raise IntegrityError("identity", f"invalid historical blob {blob!r}")
    if profile not in {
        "default",
        "spaces_nonascii",
        "alt_port_path",
        "keys_present",
        "keys_absent",
    }:
        raise IntegrityError("identity", f"invalid capture profile {profile!r}")
    base = SYNTHETIC_ROOT / blob / profile
    if profile == "spaces_nonascii":
        return str(base / "home space café"), str(base / "journal space café")
    return str(base / "home"), str(base / "journal")


def credential_hits(value: str) -> list[str]:
    hits = [name for name, pattern in TOKEN_PATTERNS.items() if pattern.search(value)]
    for match in JWT_PATTERN.finditer(value):
        header = match.group(1)
        padding = "=" * (-len(header) % 4)
        try:
            decoded = base64.urlsafe_b64decode(header + padding)
            parsed = json.loads(decoded)
        except (ValueError, json.JSONDecodeError, UnicodeDecodeError):
            continue
        if isinstance(parsed, dict):
            hits.append("jwt")
    return hits


def scalar_strings(value: Any, location: str = "$") -> Iterable[tuple[str, str]]:
    if isinstance(value, dict):
        for key, child in value.items():
            yield from scalar_strings(child, f"{location}.{key}")
    elif isinstance(value, list):
        for index, child in enumerate(value):
            yield from scalar_strings(child, f"{location}[{index}]")
    elif isinstance(value, str):
        yield location, value


def _path_tokens(value: str) -> list[str]:
    value = value.replace("append:/", "/")
    return [token.rstrip(",;)]}") for token in ABSOLUTE_PATH.findall(value)]


@dataclass(frozen=True)
class FixtureIdentity:
    blob: str
    profile: str
    platform: str
    bucket: str
    home: str
    journal: str
    path: str


def _identity(payload: dict[str, Any]) -> FixtureIdentity:
    try:
        blob = payload["blob"]
        profile = payload["profile"]
        platform = payload["platform"]
        bucket = payload["interpreter_bucket"]
        env = payload["inputs"]["env"]
        journal = payload["inputs"]["journal_path"]
    except (KeyError, TypeError) as exc:
        raise IntegrityError(
            "identity", "raw fixture identity fields are incomplete"
        ) from exc
    if platform not in {"linux", "macos"}:
        raise IntegrityError("identity", f"unknown platform {platform!r}")
    home, expected_journal = expected_roots(blob, profile)
    expected_path = (
        "/bin:/opt/service-legacy-alt/bin:/usr/bin"
        if profile == "alt_port_path"
        else "/usr/bin:/bin"
    )
    if env.get("HOME") != home or journal != expected_journal:
        raise IntegrityError(
            "profile-path",
            f"fixture {blob}/{platform}/{profile} self-authorizes HOME/journal",
        )
    if env.get("PATH") != expected_path:
        raise IntegrityError(
            "profile-path", f"fixture {blob}/{platform}/{profile} has wrong PATH"
        )
    for key in ("_SOLSTONE_JOURNAL_OVERRIDE", "SOLSTONE_JOURNAL"):
        if key in env and env[key] != expected_journal:
            raise IntegrityError(
                "profile-path", f"fixture {blob}/{platform}/{profile} has wrong {key}"
            )
    synthetic_executable(bucket)
    return FixtureIdentity(
        blob, profile, platform, bucket, home, expected_journal, expected_path
    )


def _allow_launcher(value: str, identity: FixtureIdentity) -> bool:
    synthetic = PurePosixPath(synthetic_executable(identity.bucket)).parent / "sol"
    return value in {
        str(synthetic),
        f"{identity.home}/.local/bin/sol",
        f"{identity.home}/.local/bin/journal",
    }


def _audit_credentials(payload: Any, plist: Any, label: str) -> None:
    for location, value in list(scalar_strings(payload)) + list(
        scalar_strings(plist, "$.decoded_plist")
    ):
        if location.endswith(tuple(f".{name}" for name in DUMMY_FIELDS)):
            field = location.rsplit(".", 1)[-1]
            if value != DUMMY_FIELDS[field]:
                raise IntegrityError(
                    "credential-field", f"{label} {location} is not its declared dummy"
                )
        hits = credential_hits(value)
        if hits:
            raise IntegrityError(
                "credential-pattern", f"{label} {location} contains {','.join(hits)}"
            )


def audit_fixture(
    path: Path, root: Path, *, allow_legacy_launcher: bool = False
) -> list[str]:
    payload = read_json(path)
    if not isinstance(payload, dict):
        raise IntegrityError("json", f"fixture is not an object: {path}")
    identity = _identity(payload)
    try:
        raw = payload["raw"]
        plist = plistlib.loads(base64.b64decode(raw["plist_base64"], validate=True))
        unit = raw["systemd_unit"]
    except Exception as exc:
        raise IntegrityError("plist", f"cannot strictly decode {path}: {exc}") from exc
    if not isinstance(plist, dict) or not isinstance(unit, str):
        raise IntegrityError("shape", f"fixture artifact shape is invalid: {path}")
    _audit_credentials(payload, plist, path.as_posix())

    roles: list[str] = []
    env = payload["inputs"]["env"]
    for key in ("HOME", "PATH", "_SOLSTONE_JOURNAL_OVERRIDE", "SOLSTONE_JOURNAL"):
        if key in env:
            roles.append(f"json.inputs.env.{key}")
    roles.append("json.inputs.journal_path")

    plist_env = plist.get("EnvironmentVariables")
    if not isinstance(plist_env, dict):
        raise IntegrityError("shape", f"plist lacks EnvironmentVariables: {path}")
    if plist_env != env:
        raise IntegrityError(
            "profile-path", f"plist env differs from fixture inputs: {path}"
        )
    for key in ("HOME", "PATH", "_SOLSTONE_JOURNAL_OVERRIDE", "SOLSTONE_JOURNAL"):
        if key in plist_env:
            roles.append(f"plist.EnvironmentVariables.{key}")
    arguments = plist.get("ProgramArguments")
    if (
        not isinstance(arguments, list)
        or not arguments
        or not isinstance(arguments[0], str)
    ):
        raise IntegrityError("shape", f"plist lacks launcher argument: {path}")
    if not _allow_launcher(arguments[0], identity) and not (
        allow_legacy_launcher and arguments[0].endswith("/bin/sol")
    ):
        raise IntegrityError(
            "launcher", f"unexpected plist launcher {arguments[0]!r}: {path}"
        )
    roles.append("plist.ProgramArguments[0]")
    output_suffixes = {
        "StandardOutPath": {"launchd-stdout.log", "service.log"},
        "StandardErrorPath": {"launchd-stderr.log", "service.log"},
    }
    for key, suffixes in output_suffixes.items():
        if key not in plist:
            continue
        value = plist[key]
        if (
            not isinstance(value, str)
            or not value.startswith(identity.journal + "/health/")
            or value.rsplit("/", 1)[-1] not in suffixes
        ):
            raise IntegrityError("log-path", f"unexpected {key} {value!r}: {path}")
        roles.append(f"plist.{key}")

    systemd_env: dict[str, str] = {}
    seen_exec = False
    for line in unit.splitlines():
        if line.startswith("Environment="):
            assignment = line.removeprefix("Environment=")
            key, separator, value = assignment.partition("=")
            if not separator:
                raise IntegrityError("systemd", f"invalid environment line: {line}")
            systemd_env[key] = value
            if key in {
                "HOME",
                "PATH",
                "_SOLSTONE_JOURNAL_OVERRIDE",
                "SOLSTONE_JOURNAL",
            }:
                roles.append(f"systemd.Environment.{key}")
        elif line.startswith("ExecStart="):
            command = line.removeprefix("ExecStart=")
            executable = arguments[0]
            if not (
                command == executable or command.startswith(executable + " ")
            ) or not (_allow_launcher(executable, identity) or allow_legacy_launcher):
                raise IntegrityError(
                    "launcher", f"unexpected systemd launcher {command!r}: {path}"
                )
            roles.append("systemd.ExecStart")
            seen_exec = True
        elif line.startswith(("StandardOutput=", "StandardError=")):
            key, value = line.split("=", 1)
            if value != "inherit":
                expected = f"append:{identity.journal}/health/service.log"
                if value != expected:
                    raise IntegrityError(
                        "log-path", f"unexpected {key} {value!r}: {path}"
                    )
                roles.append(f"systemd.{key}")
    if systemd_env != env or not seen_exec:
        raise IntegrityError(
            "systemd", f"systemd environment/launcher differs from inputs: {path}"
        )

    classified: dict[str, set[str]] = {
        "$.inputs.env.HOME": {identity.home},
        "$.inputs.env.PATH": {identity.path},
        "$.inputs.journal_path": {identity.journal},
        "$.decoded_plist.EnvironmentVariables.HOME": {identity.home},
        "$.decoded_plist.EnvironmentVariables.PATH": {identity.path},
        "$.decoded_plist.ProgramArguments[0]": {arguments[0]},
    }
    for key in ("_SOLSTONE_JOURNAL_OVERRIDE", "SOLSTONE_JOURNAL"):
        if key in env:
            classified[f"$.inputs.env.{key}"] = {identity.journal}
        if key in plist_env:
            classified[f"$.decoded_plist.EnvironmentVariables.{key}"] = {
                identity.journal
            }
    for key in output_suffixes:
        if key in plist:
            classified[f"$.decoded_plist.{key}"] = {plist[key]}
    for location, value in list(scalar_strings(payload)) + list(
        scalar_strings(plist, "$.decoded_plist")
    ):
        if location == "$.raw.systemd_unit":
            continue
        permitted = classified.get(location, set())
        residual = value
        for candidate in sorted(permitted, key=len, reverse=True):
            residual = residual.replace(candidate, "")
        for token in _path_tokens(residual):
            raise IntegrityError(
                "absolute-path", f"unclassified {token!r} at {path}:{location}"
            )
    for line in unit.splitlines():
        allowed: set[str] = set()
        if line.startswith("Environment="):
            key, separator, value = line.removeprefix("Environment=").partition("=")
            if separator and key in systemd_env:
                allowed.add(value)
        elif line.startswith("ExecStart="):
            allowed.add(arguments[0])
        elif line.startswith(("StandardOutput=", "StandardError=")):
            _key, value = line.split("=", 1)
            if value != "inherit":
                allowed.add(value.removeprefix("append:"))
        residual = line
        for candidate in sorted(allowed, key=len, reverse=True):
            residual = residual.replace(candidate, "")
        for token in _path_tokens(residual):
            token = token.removeprefix("append:")
            raise IntegrityError(
                "absolute-path", f"unclassified {token!r} in {path}:systemd"
            )
    return sorted(roles)


def role_oracle(
    raw_root: Path, *, allow_legacy_launcher: bool = False
) -> dict[str, list[str]]:
    fixtures: dict[str, list[str]] = {}
    for path in sorted(raw_root.rglob("*.json")):
        fixtures[path.relative_to(raw_root).as_posix()] = audit_fixture(
            path, raw_root, allow_legacy_launcher=allow_legacy_launcher
        )
    return fixtures


def audit_corpus(
    evidence_root: Path, oracle_path: Path | None = None
) -> dict[str, Any]:
    raw_root = evidence_root / "raw"
    oracle = role_oracle(raw_root)
    occurrences = sum(len(roles) for roles in oracle.values())
    role_counts = Counter(role for roles in oracle.values() for role in roles)
    if len(oracle) != EXPECTED_FIXTURES:
        raise IntegrityError(
            "fixture-count", f"expected {EXPECTED_FIXTURES}, found {len(oracle)}"
        )
    if occurrences != EXPECTED_ROLE_OCCURRENCES:
        raise IntegrityError(
            "role-count", f"expected {EXPECTED_ROLE_OCCURRENCES}, found {occurrences}"
        )
    if not role_counts or any(count <= 0 for count in role_counts.values()):
        raise IntegrityError("role-count", "one or more allowed roles has no positive")
    if oracle_path is not None:
        expected = read_json(oracle_path)
        expected_sha = sha256_bytes(canonical_bytes(expected.get("fixtures")))
        if expected_sha != EXPECTED_ROLE_ORACLE_SHA256:
            raise IntegrityError(
                "role-oracle", f"unexpected canonical hash {expected_sha}"
            )
        if expected.get("fixtures") != oracle:
            raise IntegrityError(
                "role-oracle", "fixture carrier/location/role multiset changed"
            )
    dummy_occurrences = 0
    for path in sorted(evidence_root.rglob("*.json")):
        payload = read_json(path)
        if contains_operator_path(path.read_text(encoding="utf-8")):
            # Raw fixtures use the synthetic /opt HOME only; no evidence file has a
            # legitimate operator-home role.
            raise IntegrityError("operator-path", f"operator path found in {path}")
        for location, value in scalar_strings(payload):
            hits = credential_hits(value)
            if hits:
                raise IntegrityError(
                    "credential-pattern", f"{path}:{location} contains {hits}"
                )
    for path in sorted(raw_root.rglob("*.json")):
        payload = read_json(path)
        plist = decode_plist(payload["raw"]["plist_base64"], path.as_posix())
        for _location, value in list(scalar_strings(payload)) + list(
            scalar_strings(plist, "$.decoded_plist")
        ):
            dummy_occurrences += sum(
                value.count(sentinel) for sentinel in DUMMY_FIELDS.values()
            )
    if dummy_occurrences != 570:
        raise IntegrityError(
            "credential-sentinel-count", f"expected 570, found {dummy_occurrences}"
        )
    audit_negative_bundles(evidence_root)
    packaging = read_json(evidence_root / "packaging-provenance.json")
    stable_literals = {
        "/usr/bin/ar",
        "/usr/bin/as",
        "/usr/bin/cc",
        "/usr/bin/git",
        "/usr/bin/ld",
        "/usr/bin/python3",
        "/usr/bin/strip",
        "/usr/bin:/bin",
        "/bin:/opt/service-legacy-alt/bin:/usr/bin",
        "/usr/lib/git-core/git-remote-https",
        "/usr/libexec/git-core/git-remote-https",
    }
    stable_literals.update(verified_host_tool_paths(packaging))
    stable_literals.update(str(path) for path in installed_git_helper_targets())
    for path in sorted(evidence_root.glob("*.json")):
        payload = read_json(path)
        for location, value in scalar_strings(payload):
            if value.startswith(("http://", "https://")):
                continue
            for token in _path_tokens(value):
                if token not in stable_literals:
                    raise IntegrityError(
                        "absolute-path", f"unclassified {token!r} at {path}:{location}"
                    )
    audit_manifest_provenance(evidence_root)
    return {
        "fixtures": len(oracle),
        "role_occurrences": occurrences,
        "role_oracle_sha256": sha256_bytes(canonical_bytes(oracle)),
        "role_counts": dict(sorted(role_counts.items())),
        "credential_sentinels": dummy_occurrences,
    }


def audit_negative_bundles(evidence_root: Path) -> None:
    raw_root = evidence_root / "raw"
    for path in sorted((evidence_root / "negative").rglob("*.json")):
        bundle = read_json(path)
        blob = bundle.get("blob")
        platform_name = bundle.get("platform")
        base_profile = bundle.get("base_profile")
        if not all(
            isinstance(value, str) for value in (blob, platform_name, base_profile)
        ):
            raise IntegrityError("negative-shape", f"invalid bundle identity: {path}")
        raw = read_json(raw_root / blob / platform_name / f"{base_profile}.json")
        identity = _identity(raw)
        permitted = {
            identity.home,
            identity.journal,
            identity.path,
            synthetic_executable(identity.bucket),
            str(PurePosixPath(synthetic_executable(identity.bucket)).parent / "sol"),
            f"{identity.home}/.local/bin/sol",
            f"{identity.home}/.local/bin/journal",
            f"{identity.journal}/health/launchd-stdout.log",
            f"{identity.journal}/health/launchd-stderr.log",
            f"{identity.journal}/health/service.log",
        }
        twins = bundle.get("twins")
        if not isinstance(twins, list):
            raise IntegrityError("negative-shape", f"twins are absent: {path}")
        for twin in twins:
            mutated = twin.get("mutated", {})
            if not isinstance(mutated, dict):
                raise IntegrityError(
                    "negative-shape", f"mutated row is invalid: {path}"
                )
            plist = decode_plist(
                mutated.get("plist_base64", ""), f"{path}:{twin.get('id')}"
            )
            unit = mutated.get("systemd_unit")
            if not isinstance(unit, str):
                raise IntegrityError("negative-shape", f"unit is invalid: {path}")
            _audit_credentials(mutated, plist, f"{path}:{twin.get('id')}")
            for _location, value in scalar_strings(plist):
                residual = value
                for candidate in sorted(permitted, key=len, reverse=True):
                    residual = residual.replace(candidate, "")
                for token in _path_tokens(residual):
                    raise IntegrityError(
                        "absolute-path", f"negative plist has {token!r}: {path}"
                    )
            for line in unit.splitlines():
                residual = line
                for candidate in sorted(permitted, key=len, reverse=True):
                    residual = residual.replace(candidate, "")
                for token in _path_tokens(residual):
                    raise IntegrityError(
                        "absolute-path", f"negative unit has {token!r}: {path}"
                    )


def _manifest_path(evidence_root: Path, declared: str) -> Path:
    prefix = "core/fixtures/service_legacy_evidence/"
    if declared.startswith(prefix):
        return evidence_root / declared.removeprefix(prefix)
    return Path(__file__).resolve().parents[1] / declared


def _git_bytes(commit: str, path: str) -> bytes:
    result = subprocess.run(
        ["/usr/bin/git", "show", f"{commit}:{path}"],
        cwd=Path(__file__).resolve().parents[1],
        capture_output=True,
        check=False,
    )
    if result.returncode:
        raise IntegrityError("git-blob", f"cannot resolve {commit}:{path}")
    return result.stdout


def _compare_non_owned_sources(
    commit: str,
    parent: str,
    semantic_base: str,
    loader: Any,
) -> None:
    for path in NON_OWNED_SOURCE_PATHS:
        baseline = loader(semantic_base, path)
        if loader(parent, path) != baseline or loader(commit, path) != baseline:
            raise IntegrityError(
                "non-owned-source",
                f"launcher/dispatch source changed outside the repair: {path}",
            )


def _verify_non_owned_source_paths(launcher_path: Any, dispatch: Any) -> None:
    if launcher_path != NON_OWNED_SOURCE_PATHS[0] or not isinstance(dispatch, list):
        raise IntegrityError(
            "non-owned-source-denominator",
            "launcher/dispatch source set differs from the four-path ground",
        )
    paths = tuple(row.get("path") for row in dispatch if isinstance(row, dict))
    if len(paths) != len(dispatch) or paths != NON_OWNED_SOURCE_PATHS[1:]:
        raise IntegrityError(
            "non-owned-source-denominator",
            "launcher/dispatch source set differs from the four-path ground",
        )


def verify_non_owned_source_closure(commit: str) -> None:
    parent_result = subprocess.run(
        ["/usr/bin/git", "rev-parse", f"{commit}^"],
        cwd=Path(__file__).resolve().parents[1],
        text=True,
        capture_output=True,
        check=False,
    )
    if parent_result.returncode or not re.fullmatch(
        r"[0-9a-f]{40}", parent_result.stdout.strip()
    ):
        raise IntegrityError("non-owned-source", "Commit A parent is unavailable")
    _compare_non_owned_sources(
        commit,
        parent_result.stdout.strip(),
        SEMANTIC_BASE_COMMIT,
        _git_bytes,
    )


def source_closure_self_test() -> None:
    baseline = {
        (revision, path): f"fixed:{path}".encode()
        for revision in ("commit", "parent", "semantic")
        for path in NON_OWNED_SOURCE_PATHS
    }

    def load(revision: str, path: str) -> bytes:
        return baseline[(revision, path)]

    _compare_non_owned_sources("commit", "parent", "semantic", load)
    _verify_non_owned_source_paths(
        NON_OWNED_SOURCE_PATHS[0],
        [{"path": path} for path in NON_OWNED_SOURCE_PATHS[1:]],
    )
    poisoned = dict(baseline)
    poisoned[("commit", NON_OWNED_SOURCE_PATHS[1])] = b"changed-with-matching-record"
    _expect_guard(
        "non-owned-source",
        lambda: _compare_non_owned_sources(
            "commit",
            "parent",
            "semantic",
            lambda revision, path: poisoned[(revision, path)],
        ),
    )
    poisoned_parent = dict(baseline)
    poisoned_parent[("parent", NON_OWNED_SOURCE_PATHS[2])] = b"changed-then-restored"
    _expect_guard(
        "non-owned-source",
        lambda: _compare_non_owned_sources(
            "commit",
            "parent",
            "semantic",
            lambda revision, path: poisoned_parent[(revision, path)],
        ),
    )
    poisoned_shared = dict(baseline)
    poisoned_shared[("parent", NON_OWNED_SOURCE_PATHS[3])] = b"shared-drift"
    poisoned_shared[("commit", NON_OWNED_SOURCE_PATHS[3])] = b"shared-drift"
    _expect_guard(
        "non-owned-source",
        lambda: _compare_non_owned_sources(
            "commit",
            "parent",
            "semantic",
            lambda revision, path: poisoned_shared[(revision, path)],
        ),
    )
    _expect_guard(
        "non-owned-source-denominator",
        lambda: _verify_non_owned_source_paths(
            NON_OWNED_SOURCE_PATHS[0],
            [{"path": path} for path in NON_OWNED_SOURCE_PATHS[1:-1]],
        ),
    )
    _expect_guard(
        "non-owned-source-denominator",
        lambda: _verify_non_owned_source_paths(
            NON_OWNED_SOURCE_PATHS[0],
            [
                *({"path": path} for path in NON_OWNED_SOURCE_PATHS[1:]),
                {"path": "extra.rs"},
            ],
        ),
    )


def _verify_reference(
    evidence_root: Path, commit: str, value: dict[str, Any], label: str
) -> None:
    path = value.get("path")
    expected = value.get("sha256")
    if (
        not isinstance(path, str)
        or not isinstance(expected, str)
        or not re.fullmatch(r"[0-9a-f]{64}", expected)
    ):
        raise IntegrityError("reference", f"{label} reference is malformed")
    actual_path = _manifest_path(evidence_root, path)
    if not actual_path.is_file() or sha256_file(actual_path) != expected:
        raise IntegrityError("reference", f"{label} bytes differ for {path}")
    if not actual_path.is_relative_to(evidence_root):
        if sha256_bytes(_git_bytes(commit, path)) != expected:
            raise IntegrityError("reference", f"{label} Git blob differs for {path}")


def _walk_references(
    value: Any, location: str = "$"
) -> Iterator[tuple[str, dict[str, Any]]]:
    if isinstance(value, dict):
        if "sha256" in value:
            if "path" not in value:
                raise IntegrityError("reference", f"partial reference at {location}")
            yield location, value
        for key, child in value.items():
            yield from _walk_references(child, f"{location}.{key}")
    elif isinstance(value, list):
        for index, child in enumerate(value):
            yield from _walk_references(child, f"{location}[{index}]")


def audit_manifest_provenance(evidence_root: Path) -> None:
    manifest = read_json(evidence_root / "manifest.json")
    if not isinstance(manifest, dict):
        raise IntegrityError("manifest", "manifest is not an object")
    source = manifest.get("source")
    if not isinstance(source, dict):
        raise IntegrityError("manifest", "manifest source is absent")
    commit = source.get("commit")
    if not isinstance(commit, str) or not re.fullmatch(r"[0-9a-f]{40}", commit):
        raise IntegrityError("manifest", "source commit is not exact")
    verify_non_owned_source_closure(commit)
    references = list(_walk_references(manifest))
    if len(references) < 300:
        raise IntegrityError(
            "reference-count", f"manifest exposes only {len(references)} references"
        )
    for location, reference in references:
        _verify_reference(evidence_root, commit, reference, location)
    for row in source.get("tooling", []):
        path = row.get("path")
        expected = row.get("sha256")
        if not isinstance(path, str) or not isinstance(expected, str):
            raise IntegrityError("tooling", "tooling row is malformed")
        actual_path = _manifest_path(evidence_root, path)
        if sha256_file(actual_path) != expected:
            raise IntegrityError("tooling", f"worktree bytes differ for {path}")
        if sha256_bytes(_git_bytes(commit, path)) != expected:
            raise IntegrityError("tooling", f"Git blob differs for {path}")

    inventory = manifest.get("inventory")
    if not isinstance(inventory, list):
        raise IntegrityError("inventory", "manifest inventory is absent")
    declared: dict[str, str] = {}
    for row in inventory:
        path = row.get("path")
        digest = row.get("sha256")
        if not isinstance(path, str) or not isinstance(digest, str) or path in declared:
            raise IntegrityError("inventory", "duplicate or malformed inventory row")
        declared[path] = digest
    actual = {
        (
            Path("core/fixtures/service_legacy_evidence")
            / path.relative_to(evidence_root)
        ).as_posix(): sha256_file(path)
        for path in evidence_root.rglob("*")
        if path.is_file() and path.name != "manifest.json"
    }
    if declared != actual:
        raise IntegrityError("inventory", "manifest inventory is not exact")

    capture = read_json(evidence_root / "capture-input.json")
    if capture.get("capture_input") != commit:
        raise IntegrityError("capture-input", "manifest and capture record disagree")
    packaging = read_json(evidence_root / "packaging-provenance.json")
    if packaging.get("source", {}).get("commit") != commit:
        raise IntegrityError("capture-input", "packaging source differs from manifest")
    git_fact = capture.get("git", {})
    git_path = Path(git_fact.get("executable", ""))
    if git_path != Path("/usr/bin/git") or sha256_file(git_path) != git_fact.get(
        "executable_sha256"
    ):
        raise IntegrityError("git-tool", "capture Git executable fact differs")
    helper_path = Path(git_fact.get("https_helper_path", ""))
    if (
        git_fact.get("https_helper") != "git-remote-https"
        or helper_path.resolve() not in installed_git_helper_targets()
        or not helper_path.is_file()
        or sha256_file(helper_path) != git_fact.get("https_helper_sha256")
    ):
        raise IntegrityError("git-tool", "capture HTTPS helper fact differs")
    remote = capture.get("remote", {})
    if remote.get("repository") != CANONICAL_REPOSITORY:
        raise IntegrityError("remote-ref", "capture repository identity differs")
    remote_main = subprocess.run(
        ["/usr/bin/git", "rev-parse", "refs/service-legacy/authoritative-main"],
        cwd=Path(__file__).resolve().parents[1],
        text=True,
        capture_output=True,
        check=False,
    )
    if remote_main.returncode or remote_main.stdout.strip() != remote.get("main"):
        raise IntegrityError(
            "remote-main", "capture main object differs from fetched ref"
        )

    launcher = packaging.get("launcher_chain", {}).get("journal_launcher", {})
    launcher_path = launcher.get("source_path")
    launcher_sha = launcher.get("source_sha256")
    dispatch = packaging.get("launcher_chain", {}).get("native_dispatch_sources", [])
    _verify_non_owned_source_paths(launcher_path, dispatch)
    if (
        not isinstance(launcher_path, str)
        or not isinstance(launcher_sha, str)
        or sha256_bytes(_git_bytes(commit, launcher_path)) != launcher_sha
        or sha256_file(Path(__file__).resolve().parents[1] / launcher_path)
        != launcher_sha
    ):
        raise IntegrityError("launcher-source", "launcher source fact differs")
    for row in dispatch:
        _verify_reference(evidence_root, commit, row, "packaging.native_dispatch")
    follow = read_json(evidence_root / "follow-census.json")
    for row in follow.get("entries", []):
        result = subprocess.run(
            ["/usr/bin/git", "rev-parse", f"{row['commit']}:{row['path']}"],
            cwd=Path(__file__).resolve().parents[1],
            text=True,
            capture_output=True,
            check=False,
        )
        if result.returncode or result.stdout.strip() != row["blob"]:
            raise IntegrityError("follow-blob", f"{row['commit']}:{row['path']}")
    authoritative_tags = {
        row["ref"].removeprefix("refs/tags/"): row
        for row in capture.get("remote", {}).get("tags", [])
    }
    tag_census = read_json(evidence_root / "tag-census.json")
    if set(authoritative_tags) != {row["tag"] for row in tag_census.get("tags", [])}:
        raise IntegrityError("tag-count", "capture and tag census differ")
    for row in tag_census["tags"]:
        fact = authoritative_tags[row["tag"]]
        private = f"refs/service-legacy/authoritative-tags/{row['tag']}"
        object_result = subprocess.run(
            ["/usr/bin/git", "rev-parse", private],
            cwd=Path(__file__).resolve().parents[1],
            text=True,
            capture_output=True,
            check=False,
        )
        peeled_result = subprocess.run(
            ["/usr/bin/git", "rev-parse", private + "^{}"],
            cwd=Path(__file__).resolve().parents[1],
            text=True,
            capture_output=True,
            check=False,
        )
        if (
            object_result.returncode
            or peeled_result.returncode
            or object_result.stdout.strip() != fact["object"]
            or peeled_result.stdout.strip() != fact["peeled"]
        ):
            raise IntegrityError("tag-ref", f"authoritative tag differs: {row['tag']}")
        if row["path"] is None:
            continue
        peeled = fact["peeled"]
        result = subprocess.run(
            ["/usr/bin/git", "rev-parse", f"{peeled}:{row['path']}"],
            cwd=Path(__file__).resolve().parents[1],
            text=True,
            capture_output=True,
            check=False,
        )
        if result.returncode or result.stdout.strip() != row["blob"]:
            raise IntegrityError("tag-blob", f"{row['tag']}:{row['path']}")


def _expect_guard(guard: str, action: Any) -> None:
    try:
        action()
    except IntegrityError as exc:
        if exc.guard != guard:
            raise AssertionError(f"expected {guard}, got {exc.guard}: {exc}") from exc
    else:
        raise AssertionError(f"expected {guard} refusal")


def _credential_canaries() -> dict[str, str]:
    header = base64.urlsafe_b64encode(b'{"alg":"none"}').decode().rstrip("=")
    return {
        "aws": "AKIA" + "A" * 16,
        "github": "ghp_" + "B" * 20,
        "google": "AIza" + "C" * 35,
        "jwt": f"{header}.e30.aa",
        "openai_anthropic": "sk-proj-" + "D" * 20,
    }


def _credential_boundary_cases() -> dict[str, tuple[tuple[str, ...], tuple[str, ...]]]:
    header = base64.urlsafe_b64encode(b'{"x":"' + b"A" * 3064 + b'"}').decode()
    if len(header) != 4096:
        raise AssertionError("JWT maximum header control is not 4,096 characters")
    return {
        "openai_anthropic": (
            ("sk-" + "A" * 20, "sk-svcacct-" + "A" * 256),
            (
                "sx-" + "A" * 20,
                "sk-" + "A" * 19,
                "sk-" + "A" * 257,
                "sk-" + "A" * 10 + "!" + "A" * 10,
                "Xsk-" + "A" * 20,
            ),
        ),
        "google": (
            ("AIza" + "A" * 35,),
            (
                "Aiza" + "A" * 35,
                "AIza" + "A" * 34,
                "AIza" + "A" * 36,
                "AIza" + "A" * 17 + "!" + "A" * 17,
                "XAIza" + "A" * 35,
            ),
        ),
        "github": (
            ("ghp_" + "A" * 20, "github_pat_" + "A" * 255),
            (
                "gxp_" + "A" * 20,
                "ghp_" + "A" * 19,
                "github_pat_" + "A" * 256,
                "ghp_" + "A" * 10 + "!" + "A" * 10,
                "Xghp_" + "A" * 20,
            ),
        ),
        "aws": (
            ("AKIA" + "A" * 16,),
            (
                "AKIB" + "A" * 16,
                "AKIA" + "A" * 15,
                "AKIA" + "A" * 17,
                "AKIA" + "A" * 8 + "!" + "A" * 8,
                "XAKIA" + "A" * 16,
            ),
        ),
        "jwt": (
            ("e30.e30.aa", f"{header}.{'A' * 4096}.{'A' * 4096}"),
            (
                "e.e30.aa",
                f"{header}.{'A' * 4097}.aa",
                "YWE.e30.aa",
                "e30.e3!.aa",
                "Xe30.e30.aa",
            ),
        ),
    }


def self_test(fixture: Path, oracle_path: Path) -> None:
    oracle = read_json(oracle_path)
    if (
        sha256_bytes(canonical_bytes(oracle.get("fixtures")))
        != EXPECTED_ROLE_ORACLE_SHA256
    ):
        raise AssertionError("committed role oracle hash changed")
    baseline = read_json(fixture)
    if not isinstance(baseline, dict):
        raise AssertionError("control fixture is not an object")
    for text in (
        '"/home/operator/worktree"',
        "ExecStart=/Users/operator/worktree/bin/sol",
        "file:///home/operator/worktree",
        "file:///Users/operator/worktree",
        "ExecStart=-/home/operator/worktree/bin/sol",
        "ExecStart=+/Users/operator/worktree/bin/sol",
        "ExecStart=@/home/operator/worktree/bin/sol argv0",
        "ExecStart=!/Users/operator/worktree/bin/sol",
        "ExecStart=!!/home/operator/worktree/bin/sol",
        r'"\/home\/operator\/worktree"',
        r'"\/Users\/operator\/worktree"',
        r'"{\"path\":\"\\/home\\/operator\\/worktree\"}"',
        r'"{\"path\":\"\\u002fUsers\\u002foperator\\u002fworktree\"}"',
        r'{"path":"/\u0068ome/operator/worktree"}',
        r'{"path":"/\u0055sers/operator/worktree"}',
        r'"{\"path\":\"/\\u0068ome/operator/worktree\"}"',
        r'"{\"path\":\"/\\u0055sers/operator/worktree\"}"',
        r"ExecStart=\x2fhome\x2foperator/worktree/bin/sol",
        r"ExecStart=\U0000002fhome\U0000002foperator/worktree/bin/sol",
        "file://localhost/home/operator/worktree",
        "file:%2f%2f%2fUsers%2foperator%2fworktree",
        "file:%252f%252f%252fhome%252foperator%252fworktree",
        '"/tmp/.hopper/worktrees/poison"',
    ):
        if not contains_operator_path(text):
            raise AssertionError(f"operator path control was missed: {text}")
    if contains_operator_path(
        '"/opt/solstone-service-legacy-evidence/blob/default/home/.local/bin/sol"'
    ):
        raise AssertionError("synthetic profile HOME was classified as operator state")
    host_tool_control = {
        "build": {
            "bundle": {
                "host_tools": {
                    role: {
                        "selected_path": path,
                        "sha256": sha256_file(Path(path).resolve()),
                    }
                    for role, path in HOST_TOOL_PATHS.items()
                }
            }
        }
    }
    if verified_host_tool_paths(host_tool_control) != set(HOST_TOOL_PATHS.values()):
        raise AssertionError("controlled host-tool inventory changed")
    poisoned_host_tools = json.loads(json.dumps(host_tool_control))
    poisoned_host_tools["build"]["bundle"]["host_tools"]["sh"]["selected_path"] = (
        "/tmp/sh"
    )
    _expect_guard("host-tools", lambda: verified_host_tool_paths(poisoned_host_tools))
    with tempfile.TemporaryDirectory(
        prefix="service-legacy-integrity-test-"
    ) as temporary:
        root = Path(temporary)

        def check(value: dict[str, Any], *, expected: str) -> None:
            path = root / "fixture.json"
            path.write_text(
                json.dumps(value, sort_keys=True, ensure_ascii=False), encoding="utf-8"
            )
            _expect_guard(
                expected,
                lambda: audit_fixture(path, root, allow_legacy_launcher=True),
            )

        poisoned = json.loads(json.dumps(baseline))
        poisoned["unknown"] = "/home/operator/secret"
        check(poisoned, expected="absolute-path")

        poisoned = json.loads(json.dumps(baseline))
        poisoned["inputs"]["env"]["HOME"] = "/home/operator"
        check(poisoned, expected="profile-path")

        poisoned = json.loads(json.dumps(baseline))
        poisoned["raw"]["systemd_unit"] = poisoned["raw"]["systemd_unit"].replace(
            " supervisor", "-evil supervisor", 1
        )
        check(poisoned, expected="launcher")

        expected_python = synthetic_executable(baseline["interpreter_bucket"])

        def replace_artifacts(
            value: dict[str, Any], old: str, new: str
        ) -> dict[str, Any]:
            replaced = json.loads(json.dumps(value))
            replaced["raw"]["systemd_unit"] = replaced["raw"]["systemd_unit"].replace(
                old, new
            )
            plist_value = decode_plist(replaced["raw"]["plist_base64"], "role-poison")

            def replace_strings(node: Any) -> Any:
                if isinstance(node, dict):
                    return {key: replace_strings(child) for key, child in node.items()}
                if isinstance(node, list):
                    return [replace_strings(child) for child in node]
                if isinstance(node, str):
                    return node.replace(old, new)
                return node

            plist_value = replace_strings(plist_value)
            replaced["raw"]["plist_base64"] = base64.b64encode(
                plistlib.dumps(plist_value, fmt=plistlib.FMT_XML, sort_keys=False)
            ).decode("ascii")
            return replaced

        original_launcher = decode_plist(
            baseline["raw"]["plist_base64"], "role-control"
        )["ProgramArguments"][0]
        if original_launcher.endswith("/bin/sol"):
            poisoned = replace_artifacts(
                baseline, original_launcher, original_launcher + "-wrong"
            )
            check(poisoned, expected="launcher")
        wrong_python = expected_python.removesuffix("/python") + "/python-wrong"
        synthetic_fixture = replace_artifacts(
            baseline,
            original_launcher,
            expected_python.removesuffix("/python") + "/sol",
        )
        synthetic_fixture["synthetic_executable"] = wrong_python
        check(synthetic_fixture, expected="absolute-path")

        profile_launcher = f"{baseline['inputs']['env']['HOME']}/.local/bin/sol"
        profile_fixture = replace_artifacts(
            baseline,
            expected_python.removesuffix("/python") + "/sol",
            profile_launcher,
        )
        if profile_fixture != baseline:
            poisoned = replace_artifacts(
                profile_fixture, profile_launcher, profile_launcher + "-wrong"
            )
            check(poisoned, expected="launcher")

        plist = decode_plist(baseline["raw"]["plist_base64"], "control")
        plist["UnknownPath"] = "/mnt/cache/private"
        poisoned = json.loads(json.dumps(baseline))
        poisoned["raw"]["plist_base64"] = base64.b64encode(
            plistlib.dumps(plist, fmt=plistlib.FMT_XML, sort_keys=False)
        ).decode("ascii")
        check(poisoned, expected="absolute-path")

        for field, wrong_suffix in (
            ("StandardOutPath", "health/wrong-out.log"),
            ("StandardErrorPath", "health/wrong-error.log"),
        ):
            plist_poison = decode_plist(baseline["raw"]["plist_base64"], field)
            plist_poison[field] = f"{baseline['inputs']['journal_path']}/{wrong_suffix}"
            poisoned = json.loads(json.dumps(baseline))
            poisoned["raw"]["plist_base64"] = base64.b64encode(
                plistlib.dumps(plist_poison, fmt=plistlib.FMT_XML, sort_keys=False)
            ).decode("ascii")
            check(poisoned, expected="log-path")

        canaries = _credential_canaries()
        if set(canaries) != set(TOKEN_PATTERNS) | {"jwt"}:
            raise AssertionError("credential family denominator changed")
        for family, token in canaries.items():
            for text in (
                token,
                f"before {token} after",
                f"line one\n{token}\nline three",
                f"argv --token={token} --done",
            ):
                if family not in credential_hits(text):
                    raise AssertionError(f"{family} embedded canary was missed")
            plist_poison = {"Unknown": f"plist before {token} after"}
            _expect_guard(
                "credential-pattern",
                lambda value=plist_poison: _audit_credentials({}, value, family),
            )
        negative = {
            "openai_anthropic": "sk-short",
            "google": "AIza" + "A" * 34,
            "github": "ghp_" + "A" * 19,
            "aws": "AKIA" + "A" * 15,
            "jwt": "e30.e30",
        }
        for family, token in negative.items():
            if family in credential_hits(token):
                raise AssertionError(f"{family} false-positive control matched")

        boundaries = _credential_boundary_cases()
        if set(boundaries) != set(canaries):
            raise AssertionError("credential boundary denominator changed")
        for family, (valid, invalid) in boundaries.items():
            for token in valid:
                if family not in credential_hits(token):
                    raise AssertionError(f"{family} valid boundary was missed")
            for token in invalid:
                if family in credential_hits(token):
                    raise AssertionError(f"{family} invalid boundary matched")

        for field in DUMMY_FIELDS:
            _expect_guard(
                "credential-field",
                lambda field=field: _audit_credentials(
                    {field: "not-the-impossible-sentinel"}, {}, "field"
                ),
            )


def compare_lock_split(baseline: Path, root_lock: Path, standalone: Path) -> None:
    def packages(path: Path) -> list[dict[str, Any]]:
        return tomllib.loads(path.read_text(encoding="utf-8"))["package"]

    before = packages(baseline)
    packages(root_lock)
    evidence = [
        package
        for package in before
        if package["name"] == "solstone-core-service-legacy-evidence"
    ]
    if len(evidence) != 1:
        raise IntegrityError("lock-ground", "baseline lacks one evidence package")
    baseline_text = baseline.read_text(encoding="utf-8")
    blocks = baseline_text.split("\n[[package]]\n")
    matching = [
        index
        for index, block in enumerate(blocks[1:], start=1)
        if re.search(
            r'^name = "solstone-core-service-legacy-evidence"$',
            block,
            flags=re.MULTILINE,
        )
    ]
    if len(matching) != 1:
        raise IntegrityError("lock-ground", "baseline evidence block is not unique")
    del blocks[matching[0]]
    expected_root_text = "\n[[package]]\n".join(blocks)
    if root_lock.read_text(encoding="utf-8") != expected_root_text:
        raise IntegrityError("root-lock", "root lock changed beyond evidence removal")
    split = packages(standalone)
    roots = [
        package
        for package in split
        if package["name"] == "solstone-core-service-legacy-evidence"
    ]
    if len(roots) != 1:
        raise IntegrityError("standalone-lock", "standalone lock lacks evidence root")
    expected_direct = {"plist", "serde_json", "sha2"}
    baseline_direct = {
        dependency.split()[0] for dependency in evidence[0].get("dependencies", [])
    }
    if baseline_direct != expected_direct:
        raise IntegrityError(
            "lock-ground",
            f"baseline direct dependencies differ: {sorted(baseline_direct)}",
        )
    root = roots[0]
    direct = {dependency.split()[0] for dependency in root.get("dependencies", [])}
    if direct != expected_direct or root != evidence[0]:
        raise IntegrityError(
            "standalone-lock", "standalone evidence root differs from baseline"
        )

    def identity(package: dict[str, Any]) -> tuple[str, str, str | None]:
        return package["name"], package["version"], package.get("source")

    def dependency_index(value: str, packages_value: list[dict[str, Any]]) -> int:
        fields = value.split()
        candidates = [
            index
            for index, package in enumerate(packages_value)
            if package["name"] == fields[0]
        ]
        if len(fields) >= 2 and re.fullmatch(r"\d+\.\d+\.\d+.*", fields[1]):
            candidates = [
                index
                for index in candidates
                if packages_value[index].get("version") == fields[1]
            ]
        if len(fields) >= 3 and fields[2].startswith("("):
            source = " ".join(fields[2:]).removeprefix("(").removesuffix(")")
            candidates = [
                index
                for index in candidates
                if packages_value[index].get("source") == source
            ]
        if len(candidates) != 1:
            raise IntegrityError(
                "standalone-lock", f"ambiguous dependency identity {value!r}"
            )
        return candidates[0]

    def reachable(packages_value: list[dict[str, Any]], root_index: int) -> set[int]:
        found = {root_index}
        pending = [root_index]
        while pending:
            package = packages_value[pending.pop()]
            for dependency in package.get("dependencies", []):
                index = dependency_index(dependency, packages_value)
                if index not in found:
                    found.add(index)
                    pending.append(index)
        return found

    actual_indices = reachable(split, split.index(root))
    if actual_indices != set(range(len(split))):
        extras = sorted(
            split[index]["name"] for index in set(range(len(split))) - actual_indices
        )
        raise IntegrityError("standalone-lock", f"unreachable packages: {extras}")
    baseline_packages = {identity(package): package for package in before}
    actual_packages = {identity(package): package for package in split}
    if len(baseline_packages) != len(before) or len(actual_packages) != len(split):
        raise IntegrityError("standalone-lock", "duplicate package identity")
    for package_identity, package in actual_packages.items():
        baseline_package = baseline_packages.get(package_identity)
        if baseline_package is None:
            raise IntegrityError(
                "standalone-lock", f"re-resolved dependency {package_identity[:2]}"
            )
        if package.get("checksum") != baseline_package.get("checksum"):
            raise IntegrityError(
                "standalone-lock", f"checksum differs for {package_identity[:2]}"
            )


def lock_self_test() -> None:
    baseline_text = """version = 4

[[package]]
name = "plist"
version = "1.8.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "1111"

[[package]]
name = "serde_json"
version = "1.0.1"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "2222"

[[package]]
name = "sha2"
version = "0.10.9"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "3333"

[[package]]
name = "solstone-core-service-legacy-evidence"
version = "1.0.22"
dependencies = [
 "plist",
 "serde_json",
 "sha2",
]
"""
    evidence_marker = '\n[[package]]\nname = "solstone-core-service-legacy-evidence"'
    root_text = baseline_text[: baseline_text.index(evidence_marker)]
    with tempfile.TemporaryDirectory(prefix="service-legacy-lock-test-") as temporary:
        directory = Path(temporary)
        baseline = directory / "baseline.lock"
        root = directory / "root.lock"
        standalone = directory / "standalone.lock"
        baseline.write_text(baseline_text, encoding="utf-8")
        root.write_text(root_text, encoding="utf-8")
        standalone.write_text(baseline_text, encoding="utf-8")
        compare_lock_split(baseline, root, standalone)

        root.write_text(
            root_text.replace('checksum = "2222"', 'checksum = "ffff"'),
            encoding="utf-8",
        )
        _expect_guard(
            "root-lock", lambda: compare_lock_split(baseline, root, standalone)
        )
        root.write_text(root_text, encoding="utf-8")

        extra = (
            standalone.read_text(encoding="utf-8")
            + """
[[package]]
name = "unrelated"
version = "9.9.9"
"""
        )
        standalone.write_text(extra, encoding="utf-8")
        _expect_guard(
            "standalone-lock", lambda: compare_lock_split(baseline, root, standalone)
        )
        standalone.write_text(
            baseline_text.replace('checksum = "3333"', 'checksum = "ffff"'),
            encoding="utf-8",
        )
        _expect_guard(
            "standalone-lock", lambda: compare_lock_split(baseline, root, standalone)
        )


def write_oracle(raw_root: Path, destination: Path) -> None:
    fixtures = role_oracle(raw_root, allow_legacy_launcher=True)
    payload = {
        "fixtures": fixtures,
        "schema": "service-legacy-path-role-oracle",
        "schema_version": 1,
    }
    destination.write_text(
        json.dumps(payload, indent=2, sort_keys=True, ensure_ascii=False) + "\n",
        encoding="utf-8",
    )
    print(sha256_bytes(canonical_bytes(fixtures)))


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest="command", required=True)
    audit = subparsers.add_parser("audit")
    audit.add_argument("--evidence-root", type=Path, required=True)
    audit.add_argument("--oracle", type=Path)
    write = subparsers.add_parser("write-oracle")
    write.add_argument("--raw-root", type=Path, required=True)
    write.add_argument("--output", type=Path, required=True)
    test = subparsers.add_parser("self-test")
    test.add_argument("--fixture", type=Path, required=True)
    test.add_argument("--oracle", type=Path, required=True)
    locks = subparsers.add_parser("audit-locks")
    baseline_group = locks.add_mutually_exclusive_group(required=True)
    baseline_group.add_argument("--baseline", type=Path)
    baseline_group.add_argument("--baseline-git-object")
    locks.add_argument("--root-lock", type=Path, required=True)
    locks.add_argument("--standalone-lock", type=Path, required=True)
    subparsers.add_parser("lock-self-test")
    subparsers.add_parser("source-closure-self-test")
    args = parser.parse_args()
    if args.command == "audit":
        print(
            json.dumps(
                audit_corpus(
                    args.evidence_root.resolve(),
                    args.oracle.resolve() if args.oracle else None,
                ),
                sort_keys=True,
            )
        )
    elif args.command == "write-oracle":
        write_oracle(args.raw_root.resolve(), args.output.resolve())
    elif args.command == "self-test":
        self_test(args.fixture.resolve(), args.oracle.resolve())
        print("service-legacy integrity self-test passed")
    elif args.command == "audit-locks":
        if args.baseline_git_object:
            result = subprocess.run(
                ["/usr/bin/git", "show", args.baseline_git_object],
                cwd=Path(__file__).resolve().parents[1],
                capture_output=True,
                check=False,
            )
            if result.returncode:
                raise IntegrityError(
                    "lock-ground", f"cannot resolve {args.baseline_git_object}"
                )
            with tempfile.TemporaryDirectory(
                prefix="service-legacy-lock-ground-"
            ) as temporary:
                baseline = Path(temporary) / "Cargo.lock"
                baseline.write_bytes(result.stdout)
                compare_lock_split(
                    baseline,
                    args.root_lock.resolve(),
                    args.standalone_lock.resolve(),
                )
        else:
            compare_lock_split(
                args.baseline.resolve(),
                args.root_lock.resolve(),
                args.standalone_lock.resolve(),
            )
        print("service-legacy lock split is provenance-preserving")
    elif args.command == "lock-self-test":
        lock_self_test()
        print("service-legacy lock split self-test passed")
    else:
        source_closure_self_test()
        print("service-legacy non-owned source self-test passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
