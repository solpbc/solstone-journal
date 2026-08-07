#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc
from __future__ import annotations

import re
import tomllib
from dataclasses import dataclass
from pathlib import Path

try:
    from scripts.build_native_sol_inventory import REPO_ROOT, discover
except ModuleNotFoundError:  # pragma: no cover - direct script execution path.
    from build_native_sol_inventory import REPO_ROOT, discover  # type: ignore[no-redef]

SHARED_CLIENT = REPO_ROOT / "core/crates/solstone-core-sol-client/src"
ALLOWLIST: dict[tuple[str, str], str] = {}
APP_VOCABULARY_PATTERNS = {
    "activities",
    "support",
    "navigate",
    "/app/",
    "/api/health",
}
FORBIDDEN_HTTP_SOURCE_PATTERNS: tuple[tuple[str, re.Pattern[str], str], ...] = (
    (
        "native-http-solstone-python-module-ref",
        re.compile(r"\bsolstone\.(?:apps|think|convey|talent|observe)\.[A-Za-z0-9_.]+"),
        "native HTTP commands must not reference Python server/domain modules",
    ),
    (
        "native-http-solstone-journal-env",
        re.compile(r"\bSOLSTONE_JOURNAL\b"),
        "native HTTP commands must not resolve the journal path",
    ),
    (
        "native-http-journal-path-literal",
        re.compile(r'"[^"]*(?:journal/|chronicle/)[^"]*"'),
        "native HTTP commands must not contain journal path literals",
    ),
    (
        "native-http-direct-std-fs",
        re.compile(r"\bstd::fs::"),
        "native HTTP commands must use HTTP/client seams, not direct filesystem access",
    ),
    (
        "native-http-direct-file-open",
        re.compile(r"\bFile::(?:open|create)\s*\("),
        "native HTTP commands must not open journal files directly",
    ),
    (
        "native-http-direct-open-options",
        re.compile(r"\bOpenOptions::"),
        "native HTTP commands must not open journal files directly",
    ),
    (
        "native-http-path-mutation",
        re.compile(
            r"\b(?:remove_file|remove_dir|create_dir|create_dir_all|rename)\s*\("
        ),
        "native HTTP commands must not mutate filesystem paths",
    ),
)


@dataclass(frozen=True)
class Violation:
    file: str
    kind: str
    detail: str

    @property
    def key(self) -> tuple[str, str]:
        return (self.file, self.kind)


def rel(path: Path) -> str:
    return path.relative_to(REPO_ROOT).as_posix()


def collect_violations() -> list[Violation]:
    violations: list[Violation] = []
    violations.extend(check_no_mirrored_apps_tree())
    violations.extend(check_shared_client_vocab())
    violations.extend(check_authority_adjacency())
    violations.extend(check_native_http_ownership())
    violations.extend(check_packaging_excludes_native_sources())
    return violations


def check_no_mirrored_apps_tree() -> list[Violation]:
    violations: list[Violation] = []
    for path in sorted((REPO_ROOT / "core/crates").glob("*/src/apps")):
        violations.append(
            Violation(
                rel(path),
                "mirrored-app-tree",
                "native app command code must live beside the real product owner",
            )
        )
    return violations


def check_shared_client_vocab() -> list[Violation]:
    violations: list[Violation] = []
    for path in sorted(SHARED_CLIENT.rglob("*.rs")):
        if "/generated/" in path.as_posix():
            continue
        text = path.read_text()
        hits = sorted(pattern for pattern in APP_VOCABULARY_PATTERNS if pattern in text)
        if hits:
            violations.append(
                Violation(
                    rel(path),
                    "shared-client-app-vocabulary",
                    f"shared client source contains app-owned vocabulary: {', '.join(hits)}",
                )
            )
        if re.search(r"\bmatch\s+.*\b(app|verb|command_path)\b", text):
            violations.append(
                Violation(
                    rel(path),
                    "shared-client-switchboard",
                    "shared client source appears to switch on app command identity",
                )
            )
    return violations


def check_authority_adjacency() -> list[Violation]:
    violations: list[Violation] = []
    for path in sorted(REPO_ROOT.glob("core/crates/**/authority.toml")):
        violations.append(
            Violation(
                rel(path),
                "authority-outside-real-owner",
                "native authority files must live under solstone/**/native/",
            )
        )
    for path in sorted((REPO_ROOT / "solstone").glob("**/native/authority.toml")):
        try:
            data = tomllib.loads(path.read_text())
        except tomllib.TOMLDecodeError as error:
            violations.append(
                Violation(
                    rel(path), "malformed-authority", f"cannot parse TOML: {error}"
                )
            )
            continue
        source = data.get("source")
        if not isinstance(source, str) or not source:
            violations.append(
                Violation(
                    rel(path), "authority-missing-source", "source is not declared"
                )
            )
            continue
        if not (path.parent / source).is_file():
            violations.append(
                Violation(
                    rel(path),
                    "authority-missing-source",
                    f"declared source {source!r} does not exist beside authority",
                )
            )
    return violations


def check_native_http_ownership() -> list[Violation]:
    violations: list[Violation] = []
    sources = {
        entry.source
        for entry in discover(REPO_ROOT)
        if (entry.surface == "sol-call" and entry.entry_type == "http")
        or entry.entry_type == "top-level-import"
        or entry.entry_type == "top-level-status"
    }
    for path in sorted(sources):
        text = path.read_text(encoding="utf-8")
        for kind, pattern, detail in FORBIDDEN_HTTP_SOURCE_PATTERNS:
            if pattern.search(text):
                violations.append(Violation(rel(path), kind, detail))
    return violations


def check_packaging_excludes_native_sources() -> list[Violation]:
    manifest = REPO_ROOT / "MANIFEST.in"
    lines = {
        line.strip()
        for line in manifest.read_text().splitlines()
        if line.strip() and not line.strip().startswith("#")
    }
    required = {
        "recursive-exclude solstone *.rs",
        "recursive-exclude solstone authority.toml",
    }
    missing = sorted(required - lines)
    if not missing:
        return []
    return [
        Violation(
            rel(manifest),
            "native-packaging-exclude-missing",
            f"missing native artifact excludes: {', '.join(missing)}",
        )
    ]


def main() -> int:
    violations = collect_violations()
    actual = {violation.key: violation for violation in violations}
    allowed = set(ALLOWLIST)
    unexpected = sorted(set(actual) - allowed)
    stale = sorted(allowed - set(actual))

    if unexpected or stale:
        if unexpected:
            print("native sol architecture violations:")
            for key in unexpected:
                violation = actual[key]
                print(f"- {violation.file}: {violation.kind}: {violation.detail}")
        if stale:
            print("stale native sol architecture allowlist entries:")
            for file, kind in stale:
                print(f"- {file}: {kind}: {ALLOWLIST[(file, kind)]}")
        return 1

    print("native sol architecture ok")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
