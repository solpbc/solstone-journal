#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Make conversion-wave Python and dependency retirement an executable claim."""

from __future__ import annotations

import argparse
import re
import subprocess
import sys
import tomllib
from pathlib import Path
from typing import Any, NamedTuple

ROOT = Path(__file__).resolve().parent.parent
DEFAULT_MANIFEST = ROOT / "conversion-retirements.toml"
_DISTRIBUTION_NAME = re.compile(r"^\s*([A-Za-z0-9][A-Za-z0-9._-]*)")
_NORMALIZE_DISTRIBUTION = re.compile(r"[-_.]+")
_DONE = "done"
_KNOWN_STATUSES = frozenset({_DONE, "in_progress"})
# Retirement declarations are identified only by their array position. The
# exact field set prevents an author-supplied process label from re-entering
# the schema under another name.
_WAVE_KEYS = frozenset(
    {
        "status",
        "distribution",
        "python_roots",
        "import_roots",
        "test_only_dependency_locations",
    }
)
_TEST_ONLY_GROUPS = frozenset(
    {
        "dependency-groups.dev",
        "dependency-groups.test",
        "dependency-groups.tests",
    }
)


def _repository_paths_under(root: Path, relative: str) -> list[str] | None:
    """Return repository-visible paths under ``relative``, or None if git cannot say.

    Tracked files and untracked-but-unignored files both count. Ignored build
    output does not: a checkout that removed a directory's sources leaves its
    ``__pycache__`` behind, and that is a property of one operator's disk rather
    than of the repository. Reporting it as an unfinished retirement sends the
    reader to the wave author instead of to their own working tree.
    """
    try:
        result = subprocess.run(
            [
                "git",
                "ls-files",
                "--cached",
                "--others",
                "--exclude-standard",
                "--",
                relative,
            ],
            cwd=root,
            capture_output=True,
            text=True,
            check=False,
        )
    except OSError:
        return None
    if result.returncode != 0:
        return None
    return [line for line in result.stdout.splitlines() if line.strip()]


class CheckResult(NamedTuple):
    ok: bool
    checked_waves: tuple[str, ...]
    violations: tuple[str, ...]


def _normalize_distribution(value: str) -> str:
    return _NORMALIZE_DISTRIBUTION.sub("-", value).lower()


def _require_string_list(value: Any, label: str) -> list[str]:
    if not isinstance(value, list) or any(not isinstance(item, str) for item in value):
        raise ValueError(f"{label} must be an array of strings")
    return list(value)


def _load_manifest(path: Path) -> dict[str, Any]:
    try:
        manifest = tomllib.loads(path.read_text(encoding="utf-8"))
    except (OSError, tomllib.TOMLDecodeError) as exc:
        raise ValueError(f"could not read manifest {path}: {exc}") from exc
    if manifest.get("schema_version") != 2:
        raise ValueError("schema_version must be 2")
    _require_string_list(manifest.get("dependency_files"), "dependency_files")
    _require_string_list(manifest.get("content_roots"), "content_roots")
    exclusions = _require_string_list(
        manifest.get("content_exclusions"), "content_exclusions"
    )
    if any(set(exclusion) & set("*?[]") for exclusion in exclusions):
        raise ValueError("content_exclusions must be exact paths, not glob patterns")
    waves = manifest.get("waves")
    if not isinstance(waves, list) or not waves:
        raise ValueError("waves must contain at least one declaration")
    return manifest


def _tracked_paths(root: Path) -> list[str]:
    completed = subprocess.run(
        ["git", "ls-files", "-z"],
        cwd=root,
        capture_output=True,
        check=False,
    )
    if completed.returncode != 0:
        detail = completed.stderr.decode("utf-8", errors="replace").strip()
        raise ValueError(f"git ls-files failed: {detail}")
    return [
        item.decode("utf-8", errors="surrogateescape")
        for item in completed.stdout.split(b"\0")
        if item
    ]


def _dependency_entries(document: dict[str, Any]) -> list[tuple[str, str]]:
    entries: list[tuple[str, str]] = []

    def add(location: str, values: Any) -> None:
        if not isinstance(values, list):
            return
        entries.extend((location, value) for value in values if isinstance(value, str))

    project = document.get("project")
    if isinstance(project, dict):
        add("project.dependencies", project.get("dependencies"))
        optional = project.get("optional-dependencies")
        if isinstance(optional, dict):
            for group, values in optional.items():
                add(f"project.optional-dependencies.{group}", values)

    dependency_groups = document.get("dependency-groups")
    if isinstance(dependency_groups, dict):
        for group, values in dependency_groups.items():
            add(f"dependency-groups.{group}", values)

    build_system = document.get("build-system")
    if isinstance(build_system, dict):
        add("build-system.requires", build_system.get("requires"))

    tool = document.get("tool")
    uv = tool.get("uv") if isinstance(tool, dict) else None
    if isinstance(uv, dict):
        for key in ("constraint-dependencies", "override-dependencies"):
            add(f"tool.uv.{key}", uv.get(key))
    return entries


def _distribution_from_requirement(requirement: str) -> str | None:
    match = _DISTRIBUTION_NAME.match(requirement)
    return _normalize_distribution(match.group(1)) if match else None


def _is_test_only_location(location: str) -> bool:
    _path, separator, group = location.partition(":")
    return separator == ":" and group in _TEST_ONLY_GROUPS


def _dependency_locations(
    root: Path,
    patterns: list[str],
    distribution: str,
) -> tuple[dict[str, list[str]], list[str]]:
    found: dict[str, list[str]] = {}
    errors: list[str] = []
    matched_files: set[Path] = set()
    for pattern in patterns:
        matches = sorted(root.glob(pattern))
        if not matches:
            errors.append(f"dependency file pattern matched nothing: {pattern}")
        matched_files.update(path for path in matches if path.is_file())

    for path in sorted(matched_files):
        relative = path.relative_to(root).as_posix()
        try:
            document = tomllib.loads(path.read_text(encoding="utf-8"))
        except (OSError, tomllib.TOMLDecodeError) as exc:
            errors.append(f"could not parse dependency file {relative}: {exc}")
            continue
        for location, requirement in _dependency_entries(document):
            if _distribution_from_requirement(requirement) != distribution:
                continue
            key = f"{relative}:{location}"
            found.setdefault(key, []).append(requirement)
    return found, errors


def _aliases(distribution: str, import_roots: list[str]) -> tuple[str, ...]:
    normalized = _normalize_distribution(distribution)
    aliases = {
        distribution.lower(),
        normalized,
        normalized.replace("-", "_"),
        *(root.lower() for root in import_roots),
    }
    return tuple(sorted(alias for alias in aliases if alias))


def _excluded(path: str, exclusions: list[str]) -> bool:
    return path in exclusions


def _under_content_root(path: str, content_roots: list[str]) -> bool:
    return any(
        path == root or path.startswith(f"{root.rstrip('/')}/")
        for root in content_roots
    )


def _scan_paths_and_content(
    root: Path,
    paths: list[str],
    *,
    aliases: tuple[str, ...],
    content_roots: list[str],
    exclusions: list[str],
) -> list[str]:
    violations: list[str] = []
    alias_bytes = {alias: alias.encode("utf-8") for alias in aliases}
    for relative in sorted(paths):
        lower_path = relative.lower()
        for alias in aliases:
            if alias in lower_path:
                violations.append(
                    f"retired spelling {alias!r} remains in tracked pathname {relative}"
                )
        if _excluded(relative, exclusions):
            continue
        if not _under_content_root(relative, content_roots):
            continue
        path = root / relative
        if not path.is_file():
            continue
        try:
            data = path.read_bytes().lower()
        except OSError as exc:
            violations.append(f"could not scan {relative}: {exc}")
            continue
        for alias, encoded in alias_bytes.items():
            start = 0
            while (offset := data.find(encoded, start)) >= 0:
                line = data.count(b"\n", 0, offset) + 1
                violations.append(
                    f"retired spelling {alias!r} remains in {relative}:{line}"
                )
                start = offset + len(encoded)
    return violations


def check_repository(
    root: Path,
    manifest_path: Path,
    *,
    tracked_paths: list[str] | None = None,
) -> CheckResult:
    root = root.resolve()
    try:
        manifest = _load_manifest(manifest_path)
        paths = (
            list(tracked_paths) if tracked_paths is not None else _tracked_paths(root)
        )
    except ValueError as exc:
        return CheckResult(False, (), (str(exc),))

    dependency_files = _require_string_list(
        manifest["dependency_files"], "dependency_files"
    )
    content_roots = _require_string_list(manifest["content_roots"], "content_roots")
    exclusions = _require_string_list(
        manifest["content_exclusions"], "content_exclusions"
    )
    checked: list[str] = []
    violations: list[str] = []
    tracked_set = set(paths)

    for exclusion in exclusions:
        if exclusion not in tracked_set:
            violations.append(f"content exclusion is stale or untracked: {exclusion}")
    for content_root in content_roots:
        if not any(
            path == content_root or path.startswith(f"{content_root.rstrip('/')}/")
            for path in paths
        ):
            violations.append(f"content root matched no tracked path: {content_root}")

    for index, raw_wave in enumerate(manifest["waves"]):
        label = f"waves[{index}]"
        if not isinstance(raw_wave, dict):
            violations.append(f"{label} must be a table")
            continue
        unexpected_keys = sorted(raw_wave.keys() - _WAVE_KEYS)
        if unexpected_keys:
            violations.append(
                f"{label} has unsupported fields: {', '.join(unexpected_keys)}"
            )
            continue
        status = raw_wave.get("status")
        distribution = raw_wave.get("distribution")
        if status not in _KNOWN_STATUSES:
            violations.append(
                f"{label}.status must be one of {sorted(_KNOWN_STATUSES)}"
            )
            continue
        if not isinstance(distribution, str) or not distribution:
            violations.append(f"{label}.distribution must be a non-empty string")
            continue
        try:
            python_roots = _require_string_list(
                raw_wave.get("python_roots"), f"{label}.python_roots"
            )
            import_roots = _require_string_list(
                raw_wave.get("import_roots"), f"{label}.import_roots"
            )
            test_only_locations = set(
                _require_string_list(
                    raw_wave.get("test_only_dependency_locations"),
                    f"{label}.test_only_dependency_locations",
                )
            )
        except ValueError as exc:
            violations.append(str(exc))
            continue
        if not python_roots and not import_roots:
            violations.append(
                f"{label}: declare at least one Python path or import root"
            )
            continue
        if any(not value for value in [*python_roots, *import_roots]):
            violations.append(
                f"{label}: Python paths and import roots must not be empty"
            )
            continue
        if status != _DONE:
            continue
        invalid_test_locations = sorted(
            location
            for location in test_only_locations
            if not _is_test_only_location(location)
        )
        if invalid_test_locations:
            violations.extend(
                f"{label}: test-only dependency exception is not a test group: "
                f"{location}"
                for location in invalid_test_locations
            )
            continue

        checked.append(label)
        for python_root in python_roots:
            survivors = _repository_paths_under(root, python_root)
            if survivors is None:
                # git could not answer; fall back to the on-disk test rather than
                # reporting a retirement we cannot prove.
                if (root / python_root).exists():
                    violations.append(
                        f"{label}: declared Python root still exists: {python_root}"
                    )
                continue
            if survivors:
                violations.append(
                    f"{label}: declared Python root still exists: {python_root} "
                    f"({len(survivors)} file(s) in the repository, e.g. "
                    f"{survivors[0]})"
                )

        normalized_distribution = _normalize_distribution(distribution)
        locations, dependency_errors = _dependency_locations(
            root,
            dependency_files,
            normalized_distribution,
        )
        violations.extend(f"{label}: {item}" for item in dependency_errors)
        for location, requirements in sorted(locations.items()):
            if location in test_only_locations:
                continue
            for requirement in requirements:
                violations.append(
                    f"{label}: retired dependency remains at {location}: {requirement}"
                )
        stale_allowances = test_only_locations - locations.keys()
        violations.extend(
            f"{label}: allowed dependency location is stale: {location}"
            for location in sorted(stale_allowances)
        )

        aliases = _aliases(distribution, import_roots)
        for item in _scan_paths_and_content(
            root,
            paths,
            aliases=aliases,
            content_roots=content_roots,
            exclusions=exclusions,
        ):
            violations.append(f"{label}: {item}")

    return CheckResult(not violations, tuple(checked), tuple(violations))


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description="Prove completed conversion waves retired Python and packages."
    )
    parser.add_argument("--root", type=Path, default=ROOT)
    parser.add_argument("--manifest", type=Path, default=DEFAULT_MANIFEST)
    args = parser.parse_args(argv)

    result = check_repository(args.root, args.manifest)
    if not result.ok:
        print("conversion-retirements: FAIL", file=sys.stderr)
        for violation in result.violations:
            print(f"- {violation}", file=sys.stderr)
        return 1
    waves = ", ".join(result.checked_waves)
    print(f"conversion-retirements: pass ({waves})")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
