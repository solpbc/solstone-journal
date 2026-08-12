#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Normalize raw historical service-generator evidence fixtures.

This is a hand-run evidence-capture tool, not a CI dependency. It parses the
raw plist and ordered systemd-unit fields, replaces only documented parameter
values with tokens, and proves the applicable profile captures converge.
"""

from __future__ import annotations

import base64
import copy
import json
import plistlib
import shutil
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[1]
EVIDENCE_ROOT = ROOT / "core/fixtures/service_legacy_evidence"
RAW_ROOT = EVIDENCE_ROOT / "raw"
NORMALIZED_ROOT = EVIDENCE_ROOT / "normalized"
FOLLOW_CENSUS = EVIDENCE_ROOT / "follow-census.json"
SCHEMA = "service-legacy-normalized-evidence"
SCHEMA_VERSION = 1
PLATFORMS = ("linux", "macos")
BASE_PROFILES = ("default", "spaces_nonascii", "alt_port_path")
KEY_PROFILES = ("keys_present", "keys_absent")
OPTIONAL_API_KEYS = (
    "ANTHROPIC_API_KEY",
    "OPENAI_API_KEY",
    "GOOGLE_API_KEY",
    "REVAI_ACCESS_TOKEN",
    "PLAUD_ACCESS_TOKEN",
)
ENVIRONMENT_TOKENS = {
    "HOME": "<HOME>",
    "PATH": "<PATH>",
    "SOLSTONE_JOURNAL": "<JOURNAL>",
    "_SOLSTONE_JOURNAL_OVERRIDE": "<JOURNAL>",
    **{key: f"<API_KEY:{key}>" for key in OPTIONAL_API_KEYS},
}


class NormalizationError(RuntimeError):
    """A raw fixture does not satisfy the evidence normalization contract."""


def read_json(path: Path) -> dict[str, Any]:
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        raise NormalizationError(f"cannot read raw fixture {path}") from exc
    if not isinstance(value, dict):
        raise NormalizationError(f"raw fixture is not an object: {path}")
    return value


def expected_profiles(index: int) -> tuple[str, ...]:
    return BASE_PROFILES + (KEY_PROFILES if index <= 18 else ())


def plist_from_raw(raw: dict[str, Any], path: Path) -> dict[str, Any]:
    try:
        encoded = raw["raw"]["plist_base64"]
        value = plistlib.loads(base64.b64decode(encoded))
    except (KeyError, TypeError, ValueError, plistlib.InvalidFileException) as exc:
        raise NormalizationError(f"invalid plist payload: {path}") from exc
    if not isinstance(value, dict):
        raise NormalizationError(f"plist is not a dictionary: {path}")
    return value


def journal_token(value: str, journal: str, field: str, path: Path) -> str:
    if not value.startswith(journal):
        raise NormalizationError(f"{field} does not begin with its journal path: {path}")
    return "<JOURNAL>" + value[len(journal) :]


def normalize_plist(raw: dict[str, Any], path: Path) -> dict[str, Any]:
    plist = copy.deepcopy(plist_from_raw(raw, path))
    inputs = raw["inputs"]
    journal = inputs["journal_path"]
    port = inputs["port"]
    environment = plist.get("EnvironmentVariables")
    if not isinstance(environment, dict):
        raise NormalizationError(f"plist lacks EnvironmentVariables dictionary: {path}")
    if environment != inputs.get("env"):
        raise NormalizationError(f"plist environment differs from fixture inputs: {path}")
    for key, token in ENVIRONMENT_TOKENS.items():
        if key in environment:
            environment[key] = token
    argv = plist.get("ProgramArguments")
    if not isinstance(argv, list) or not argv or not isinstance(argv[0], str):
        raise NormalizationError(f"plist lacks ProgramArguments launcher: {path}")
    argv[0] = "<LAUNCHER_BIN>"
    for index, value in enumerate(argv[1:], start=1):
        if value == port or value == str(port):
            argv[index] = "<PORT>"
    for key in ("StandardOutPath", "StandardErrorPath"):
        if key in plist:
            if not isinstance(plist[key], str):
                raise NormalizationError(f"plist {key} is not a path string: {path}")
            plist[key] = journal_token(plist[key], journal, key, path)
    return plist


def normalize_systemd(raw: dict[str, Any], path: Path, launcher: str) -> str:
    try:
        unit = raw["raw"]["systemd_unit"]
        inputs = raw["inputs"]
        journal = inputs["journal_path"]
        port = inputs["port"]
    except KeyError as exc:
        raise NormalizationError(f"systemd fixture lacks required field: {path}") from exc
    if not isinstance(unit, str) or not isinstance(launcher, str):
        raise NormalizationError(f"systemd fixture has non-string content: {path}")
    expected_environment = inputs.get("env")
    if not isinstance(expected_environment, dict):
        raise NormalizationError(f"systemd fixture lacks input environment dictionary: {path}")
    systemd_environment: dict[str, str] = {}
    for line in unit.splitlines():
        if not line.startswith("Environment="):
            continue
        key, separator, value = line.removeprefix("Environment=").partition("=")
        if not separator or key in systemd_environment:
            raise NormalizationError(f"malformed or duplicate systemd Environment line: {path}")
        systemd_environment[key] = value
    if systemd_environment != expected_environment:
        raise NormalizationError(f"systemd environment differs from fixture inputs: {path}")
    normalized_lines: list[str] = []
    for line in unit.splitlines(keepends=True):
        body = line[:-1] if line.endswith("\n") else line
        newline = "\n" if line.endswith("\n") else ""
        if body.startswith("ExecStart="):
            command = body.removeprefix("ExecStart=")
            if not command.startswith(launcher):
                raise NormalizationError(f"ExecStart does not start with plist launcher: {path}")
            command = "<LAUNCHER_BIN>" + command[len(launcher) :]
            port_suffix = f" {port}"
            if command.endswith(port_suffix):
                command = command[: -len(port_suffix)] + " <PORT>"
            body = "ExecStart=" + command
        elif body.startswith("Environment="):
            assignment = body.removeprefix("Environment=")
            key, separator, _value = assignment.partition("=")
            if not separator:
                raise NormalizationError(f"malformed Environment line: {path}")
            token = ENVIRONMENT_TOKENS.get(key)
            if token is not None:
                body = f"Environment={key}={token}"
        elif body.startswith("StandardOutput=append:"):
            body = "StandardOutput=append:" + journal_token(
                body.removeprefix("StandardOutput=append:"), journal, "StandardOutput", path
            )
        elif body.startswith("StandardError=append:"):
            body = "StandardError=append:" + journal_token(
                body.removeprefix("StandardError=append:"), journal, "StandardError", path
            )
        normalized_lines.append(body + newline)
    return "".join(normalized_lines)


def normalized_variant(raw: dict[str, Any], path: Path) -> dict[str, Any]:
    plist = normalize_plist(raw, path)
    return {
        "plist": plist,
        "systemd_unit": normalize_systemd(raw, path, plist_from_raw(raw, path)["ProgramArguments"][0]),
    }


def fixed_projection(raw: dict[str, Any], path: Path) -> dict[str, Any]:
    """Remove only parameter fields from normalized content for fixed-field checks."""
    variant = normalized_variant(raw, path)
    plist = variant["plist"]
    plist.pop("EnvironmentVariables", None)
    return {"plist": plist, "systemd_unit": "\n".join(
        line
        for line in variant["systemd_unit"].splitlines()
        if not line.startswith("Environment=")
    ) + ("\n" if variant["systemd_unit"].endswith("\n") else "")}


def key_state(raw: dict[str, Any], path: Path) -> str:
    environment = plist_from_raw(raw, path).get("EnvironmentVariables")
    if not isinstance(environment, dict):
        raise NormalizationError(f"plist lacks EnvironmentVariables dictionary: {path}")
    present = {key for key in OPTIONAL_API_KEYS if key in environment}
    if not present:
        return "optional_keys_absent"
    if present != set(OPTIONAL_API_KEYS):
        raise NormalizationError(f"partial optional API-key set in fixture: {path}")
    return "optional_keys_present"


def normalize_blob_platform(entry: dict[str, Any], platform: str) -> dict[str, Any]:
    blob = entry["blob"]
    index = entry["index"]
    raw_profiles: dict[str, tuple[dict[str, Any], Path]] = {}
    for profile in expected_profiles(index):
        path = RAW_ROOT / blob / platform / f"{profile}.json"
        raw = read_json(path)
        if raw.get("blob") != blob or raw.get("platform") != platform or raw.get("profile") != profile:
            raise NormalizationError(f"raw fixture identity mismatch: {path}")
        raw_profiles[profile] = (raw, path)
    all_fixed = [fixed_projection(raw, path) for raw, path in raw_profiles.values()]
    if any(value != all_fixed[0] for value in all_fixed[1:]):
        raise NormalizationError(f"fixed fields vary across profiles for {blob}/{platform}")
    groups: dict[str, list[tuple[str, dict[str, Any]]]] = {}
    for profile, (raw, path) in raw_profiles.items():
        state = key_state(raw, path) if index <= 18 else "canonical"
        groups.setdefault(state, []).append((profile, normalized_variant(raw, path)))
    if index <= 18 and set(groups) != {"optional_keys_absent", "optional_keys_present"}:
        raise NormalizationError(f"oldest-family key-state groups are incomplete for {blob}/{platform}")
    variants: dict[str, dict[str, Any]] = {}
    for state, captures in groups.items():
        canonical = captures[0][1]
        if any(value != canonical for _profile, value in captures[1:]):
            profiles = ", ".join(profile for profile, _value in captures)
            raise NormalizationError(f"profiles do not converge for {blob}/{platform}/{state}: {profiles}")
        variants[state] = {
            "profiles": sorted(profile for profile, _value in captures),
            **canonical,
        }
    return {
        "blob": blob,
        "commit": entry["commit"],
        "path": entry["path"],
        "platform": platform,
        "schema": SCHEMA,
        "schema_version": SCHEMA_VERSION,
        "variants": variants,
    }


def write_json(path: Path, value: dict[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, indent=2, sort_keys=True, ensure_ascii=False) + "\n", encoding="utf-8")


def main() -> int:
    entries = read_json(FOLLOW_CENSUS).get("entries")
    if not isinstance(entries, list) or len(entries) != 44:
        raise NormalizationError("follow census must contain exactly 44 entries")
    shutil.rmtree(NORMALIZED_ROOT, ignore_errors=True)
    count = 0
    for entry in entries:
        if not isinstance(entry, dict):
            raise NormalizationError("follow census entry is not an object")
        for platform in PLATFORMS:
            write_json(
                NORMALIZED_ROOT / entry["blob"] / f"{platform}.json",
                normalize_blob_platform(entry, platform),
            )
            count += 1
    print(f"wrote {count} normalized fixtures")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
