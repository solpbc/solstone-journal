#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Guard the Rust core workspace-owned SPL dependency pin."""

from __future__ import annotations

import argparse
import re
import subprocess
import sys
import tomllib
from collections.abc import Iterator, Mapping
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parent.parent

APPROVED_SOURCE_URL = "https://github.com/solpbc/spl-rust"
SPL_PACKAGES = ("spl-core", "spl-home", "spl-transport")
SPL_PACKAGE_SET = set(SPL_PACKAGES)

DEPENDENCY_KINDS = ("dependencies", "dev-dependencies", "build-dependencies")
MEMBER_OVERRIDE_KEYS = ("git", "tag", "rev", "branch", "version", "path")
WORKSPACE_SELECTOR_KEYS = ("rev", "branch", "version", "path")
CONFIG_REPLACEMENT_KEYS = ("replace-with", "directory", "local-registry", "registry")
CONFIG_PATHS = (
    ".cargo/config.toml",
    ".cargo/config",
    "core/.cargo/config.toml",
    "core/.cargo/config",
)
LOCK_COMMIT_RE = re.compile(r"^[0-9a-f]{40}$")

W001_WORKSPACE_DEPENDENCY_MISSING = (
    "spl-pin W001 workspace dependency missing: {package} is absent from "
    "core/Cargo.toml [workspace.dependencies]; repair by declaring {package} "
    f"from {APPROVED_SOURCE_URL} with the shared tag."
)
W002_WORKSPACE_DEPENDENCY_TABLE = (
    "spl-pin W002 workspace dependency must be a table: {package} is a bare "
    "version string in core/Cargo.toml [workspace.dependencies]; repair by "
    f"using a git dependency table with URL {APPROVED_SOURCE_URL} and the "
    "shared tag."
)
W003_WORKSPACE_SOURCE_URL = (
    "spl-pin W003 workspace source URL mismatch: {package} uses git source "
    f"{{found}}; repair by using {APPROVED_SOURCE_URL}."
)
W004_WORKSPACE_SELECTOR_TAG_ONLY = (
    "spl-pin W004 workspace selector must be tag-only: {package} declares "
    "{keys}; repair by removing rev/branch/version/path selectors and using "
    "only tag in the workspace dependency entry."
)
W005_WORKSPACE_TAG_EMPTY = (
    "spl-pin W005 workspace tag is empty: {package} has an empty tag value; "
    "repair by setting the shared non-empty SPL tag in core/Cargo.toml."
)
W006_WORKSPACE_TAGS_SPLIT = (
    "spl-pin W006 workspace SPL tags split: spl-core, spl-home and spl-transport "
    "use different tags; repair by setting every workspace entry to the same tag."
)
W007_WORKSPACE_ALIAS = (
    "spl-pin W007 workspace dependency alias touches SPL package: {dependency} "
    "resolves to {package}; repair by declaring SPL packages only under "
    "canonical keys spl-core, spl-home and spl-transport without package aliases."
)

M001_MEMBER_OVERRIDE = (
    "spl-pin M001 member SPL dependency overrides workspace pin: {manifest} "
    "{table}.{dependency} resolves to {package} and declares {keys}; repair by "
    "removing git/tag/rev/branch/version/path and using workspace = true for "
    "the SPL dependency."
)

L001_LOCK_PACKAGE_MISSING = (
    "spl-pin L001 lockfile package missing: core/Cargo.lock has no [[package]] "
    "record for {package}; repair by regenerating the lockfile from the "
    "workspace SPL tag pin."
)
L002_LOCK_PACKAGE_DUPLICATED = (
    "spl-pin L002 lockfile package duplicated: core/Cargo.lock has {count} "
    "[[package]] records for {package}; repair by resolving the graph to a "
    "single {package} record from the workspace SPL tag pin."
)
L003_LOCK_SOURCE_MISSING = (
    "spl-pin L003 lockfile source missing: {package} has no source key in "
    "core/Cargo.lock; repair by resolving it from the approved spl-rust git "
    "tag, not a path/workspace package."
)
L004_LOCK_SOURCE_NOT_GIT = (
    "spl-pin L004 lockfile source is not git: {package} source is {source}; "
    f"repair by resolving it from {APPROVED_SOURCE_URL} with the workspace tag."
)
L005_LOCK_GIT_URL = (
    "spl-pin L005 lockfile git URL mismatch: {package} source URL is {url}; "
    f"repair by resolving it from {APPROVED_SOURCE_URL}."
)
L006_LOCK_SELECTOR_TAG = (
    "spl-pin L006 lockfile selector must be tag: {package} source selector is "
    "{selector}; repair by resolving it with tag from the workspace SPL pin."
)
L007_LOCK_TAG_WORKSPACE = (
    "spl-pin L007 lockfile tag disagrees with workspace: {package} lock tag is "
    "{lock_tag}; repair by regenerating core/Cargo.lock so it matches the "
    "workspace SPL tag."
)
L008_LOCK_COMMIT_INVALID = (
    "spl-pin L008 lockfile commit fragment invalid: {package} source commit is "
    "{commit}; repair by regenerating core/Cargo.lock with a git source ending "
    "in a 40-character lowercase hex commit."
)
L009_LOCK_COMMITS_SPLIT = (
    "spl-pin L009 lockfile SPL commits split: spl-core, spl-home and "
    "spl-transport resolve to different commits; repair by regenerating "
    "core/Cargo.lock so every SPL package resolves to the same spl-rust commit."
)

R001_WORKSPACE_PATCH_SOURCE = (
    "spl-pin R001 workspace patch rewrites approved source: core/Cargo.toml "
    f"[patch.{{source}}] targets {APPROVED_SOURCE_URL}; repair by removing the "
    "SPL patch route and using the workspace tag pin."
)
R002_WORKSPACE_PATCH_PACKAGE = (
    "spl-pin R002 workspace patch supplies SPL package: core/Cargo.toml "
    "[patch.{source}] entry {dependency} resolves to {package}; repair by "
    "removing the SPL patch entry and using the workspace tag pin."
)
R003_REPLACE_PACKAGE = (
    "spl-pin R003 legacy replace targets SPL package: core/Cargo.toml "
    "[replace] key {replace_key} names {package}; repair by removing the SPL "
    "replace entry and using the workspace tag pin."
)
R004_CONFIG_SOURCE_REPLACEMENT = (
    "spl-pin R004 Cargo source replacement rewrites approved source: "
    f"{{config_path}} [source.{{source_name}}] targets {APPROVED_SOURCE_URL} "
    "and declares {keys}; repair by removing the source replacement route and "
    "using the workspace tag pin."
)
R005_CONFIG_PATCH_SOURCE = (
    "spl-pin R005 Cargo config patch rewrites approved source: {config_path} "
    f"[patch.{{source}}] targets {APPROVED_SOURCE_URL}; repair by removing the "
    "SPL patch route and using the workspace tag pin."
)
R006_CONFIG_PATCH_PACKAGE = (
    "spl-pin R006 Cargo config patch supplies SPL package: {config_path} "
    "[patch.{source}] entry {dependency} resolves to {package}; repair by "
    "removing the SPL patch entry and using the workspace tag pin."
)
R007_IN_TREE_PACKAGE_COPY = (
    "spl-pin R007 tracked in-tree SPL package copy: {manifest} declares package "
    "name {package}; repair by removing the in-tree SPL implementation copy and "
    "depending on the approved spl-rust tag pin."
)


def load_toml(path: Path) -> dict[str, Any]:
    with path.open("rb") as handle:
        return tomllib.load(handle)


def as_mapping(value: object) -> Mapping[str, Any] | None:
    if isinstance(value, Mapping):
        return value
    return None


def format_value(value: object) -> str:
    if value is None:
        return "<missing>"
    if isinstance(value, str):
        return value
    return repr(value)


def format_keys(keys: list[str]) -> str:
    return ", ".join(keys)


def tracked_cargo_manifests(root: Path) -> list[Path]:
    result = subprocess.run(
        ["git", "ls-files", "--", "*Cargo.toml"],
        cwd=root,
        check=True,
        capture_output=True,
        text=True,
    )
    return [root / line for line in result.stdout.splitlines() if line]


def dependency_identity(name: str, spec: object) -> str:
    table = as_mapping(spec)
    if table is None:
        return name
    package = table.get("package")
    if isinstance(package, str):
        return package
    return name


def selector_keys(spec: object) -> list[str]:
    if isinstance(spec, str):
        return ["version"]
    table = as_mapping(spec)
    if table is None:
        return []
    return [key for key in MEMBER_OVERRIDE_KEYS if key in table]


def iter_dependency_tables(
    manifest: Mapping[str, Any],
) -> Iterator[tuple[str, Mapping[str, Any]]]:
    for kind in DEPENDENCY_KINDS:
        table = as_mapping(manifest.get(kind))
        if table is not None:
            yield kind, table

    target = as_mapping(manifest.get("target"))
    if target is None:
        return
    for cfg in sorted(target):
        target_table = as_mapping(target[cfg])
        if target_table is None:
            continue
        for kind in DEPENDENCY_KINDS:
            table = as_mapping(target_table.get(kind))
            if table is not None:
                yield f"target.{cfg}.{kind}", table


def workspace_selector_findings(spec: Mapping[str, Any]) -> list[str]:
    keys: list[str] = []
    if "tag" not in spec:
        keys.append("missing tag")
    keys.extend(key for key in WORKSPACE_SELECTOR_KEYS if key in spec)
    return keys


def collect_workspace_findings(root: Path) -> tuple[list[str], str | None]:
    manifest = load_toml(root / "core" / "Cargo.toml")
    workspace = as_mapping(manifest.get("workspace")) or {}
    dependencies = as_mapping(workspace.get("dependencies")) or {}
    findings: list[str] = []
    tags: dict[str, str] = {}
    package_valid = dict.fromkeys(SPL_PACKAGES, True)

    alias_findings: list[str] = []
    for dependency in sorted(dependencies):
        spec = dependencies[dependency]
        identity = dependency_identity(dependency, spec)
        if dependency != identity and (
            dependency in SPL_PACKAGE_SET or identity in SPL_PACKAGE_SET
        ):
            alias_findings.append(
                W007_WORKSPACE_ALIAS.format(
                    dependency=dependency,
                    package=identity,
                )
            )
            if dependency in SPL_PACKAGE_SET:
                package_valid[dependency] = False

    for package in SPL_PACKAGES:
        spec = dependencies.get(package)
        if spec is None:
            findings.append(W001_WORKSPACE_DEPENDENCY_MISSING.format(package=package))
            package_valid[package] = False
            continue
        if isinstance(spec, str):
            findings.append(W002_WORKSPACE_DEPENDENCY_TABLE.format(package=package))
            package_valid[package] = False
            continue

        table = as_mapping(spec)
        if table is None:
            findings.append(W002_WORKSPACE_DEPENDENCY_TABLE.format(package=package))
            package_valid[package] = False
            continue

        git_source = table.get("git")
        if git_source != APPROVED_SOURCE_URL:
            findings.append(
                W003_WORKSPACE_SOURCE_URL.format(
                    package=package,
                    found=format_value(git_source),
                )
            )
            package_valid[package] = False

        selector_problems = workspace_selector_findings(table)
        if selector_problems:
            findings.append(
                W004_WORKSPACE_SELECTOR_TAG_ONLY.format(
                    package=package,
                    keys=format_keys(selector_problems),
                )
            )
            package_valid[package] = False

        tag = table.get("tag")
        if "tag" in table and (not isinstance(tag, str) or not tag):
            findings.append(W005_WORKSPACE_TAG_EMPTY.format(package=package))
            package_valid[package] = False
        elif isinstance(tag, str) and tag:
            tags[package] = tag

    if all(package_valid.values()) and len(set(tags.values())) == 1:
        workspace_tag = next(iter(tags.values()))
    else:
        workspace_tag = None

    if len(tags) == len(SPL_PACKAGES) and len(set(tags.values())) > 1:
        findings.append(W006_WORKSPACE_TAGS_SPLIT)
        workspace_tag = None

    findings.extend(alias_findings)
    return findings, workspace_tag


def collect_member_findings(root: Path) -> list[str]:
    findings: list[str] = []
    core_root = root / "core"
    workspace_manifest = core_root / "Cargo.toml"
    for manifest_path in sorted(tracked_cargo_manifests(root)):
        if manifest_path == workspace_manifest:
            continue
        if not manifest_path.is_relative_to(core_root):
            continue
        manifest = load_toml(manifest_path)
        rel_manifest = manifest_path.relative_to(root).as_posix()
        for table_name, dependencies in iter_dependency_tables(manifest):
            for dependency in sorted(dependencies):
                spec = dependencies[dependency]
                identity = dependency_identity(dependency, spec)
                if identity not in SPL_PACKAGE_SET:
                    continue
                keys = selector_keys(spec)
                if not keys:
                    continue
                findings.append(
                    M001_MEMBER_OVERRIDE.format(
                        manifest=rel_manifest,
                        table=table_name,
                        dependency=dependency,
                        package=identity,
                        keys=format_keys(keys),
                    )
                )
    return findings


def lock_packages(lock: Mapping[str, Any], package: str) -> list[Mapping[str, Any]]:
    packages = lock.get("package")
    if not isinstance(packages, list):
        return []
    records: list[Mapping[str, Any]] = []
    for record in packages:
        table = as_mapping(record)
        if table is not None and table.get("name") == package:
            records.append(table)
    return records


def parse_git_lock_source(
    source: str,
) -> tuple[str, str | None, str | None, str | None]:
    body = source.removeprefix("git+")
    before_hash, separator, commit = body.partition("#")
    commit_value = commit if separator else None
    url, query_separator, query = before_hash.partition("?")
    if not query_separator:
        return url, None, None, commit_value
    selector, value_separator, value = query.partition("=")
    return url, selector, value if value_separator else "", commit_value


def collect_lockfile_findings(root: Path, workspace_tag: str | None) -> list[str]:
    lock = load_toml(root / "core" / "Cargo.lock")
    findings: list[str] = []
    commits: dict[str, str] = {}

    for package in SPL_PACKAGES:
        records = lock_packages(lock, package)
        if not records:
            findings.append(L001_LOCK_PACKAGE_MISSING.format(package=package))
            continue
        if len(records) > 1:
            findings.append(
                L002_LOCK_PACKAGE_DUPLICATED.format(
                    package=package,
                    count=len(records),
                )
            )
            continue

        source = records[0].get("source")
        if source is None:
            findings.append(L003_LOCK_SOURCE_MISSING.format(package=package))
            continue
        if not isinstance(source, str) or not source.startswith("git+"):
            findings.append(
                L004_LOCK_SOURCE_NOT_GIT.format(
                    package=package,
                    source=format_value(source),
                )
            )
            continue

        url, selector, selector_value, commit = parse_git_lock_source(source)
        if url != APPROVED_SOURCE_URL:
            findings.append(L005_LOCK_GIT_URL.format(package=package, url=url))
        if selector != "tag":
            findings.append(
                L006_LOCK_SELECTOR_TAG.format(
                    package=package,
                    selector=selector or "<none>",
                )
            )
        elif workspace_tag is not None and selector_value != workspace_tag:
            findings.append(
                L007_LOCK_TAG_WORKSPACE.format(
                    package=package,
                    lock_tag=selector_value,
                )
            )

        if commit is None or not LOCK_COMMIT_RE.fullmatch(commit):
            findings.append(
                L008_LOCK_COMMIT_INVALID.format(
                    package=package,
                    commit=commit or "<missing>",
                )
            )
        else:
            commits[package] = commit

    if len(commits) == len(SPL_PACKAGES) and len(set(commits.values())) > 1:
        findings.append(L009_LOCK_COMMITS_SPLIT)

    return findings


def collect_patch_findings(
    patch: Mapping[str, Any],
    *,
    source_message: str,
    package_message: str,
    config_path: str | None = None,
) -> list[str]:
    findings: list[str] = []
    for source in sorted(patch):
        entries = as_mapping(patch[source])
        if source == APPROVED_SOURCE_URL:
            if config_path is None:
                findings.append(source_message.format(source=source))
            else:
                findings.append(
                    source_message.format(config_path=config_path, source=source)
                )
        if entries is None:
            continue
        for dependency in sorted(entries):
            spec = entries[dependency]
            identity = dependency_identity(dependency, spec)
            if identity not in SPL_PACKAGE_SET:
                continue
            if config_path is None:
                findings.append(
                    package_message.format(
                        source=source,
                        dependency=dependency,
                        package=identity,
                    )
                )
            else:
                findings.append(
                    package_message.format(
                        config_path=config_path,
                        source=source,
                        dependency=dependency,
                        package=identity,
                    )
                )
    return findings


def source_name_url(name: str, table: Mapping[str, Any]) -> str | None:
    git = table.get("git")
    if isinstance(git, str):
        return git
    if name.startswith("https://") or name.startswith("http://"):
        return name
    return None


def collect_config_findings(root: Path, config_path: Path) -> list[str]:
    config = load_toml(config_path)
    rel_config = config_path.relative_to(root).as_posix()
    findings: list[str] = []

    patch = as_mapping(config.get("patch"))
    if patch is not None:
        findings.extend(
            collect_patch_findings(
                patch,
                source_message=R005_CONFIG_PATCH_SOURCE,
                package_message=R006_CONFIG_PATCH_PACKAGE,
                config_path=rel_config,
            )
        )

    sources = as_mapping(config.get("source"))
    if sources is None:
        return findings
    for source_name in sorted(sources):
        source_table = as_mapping(sources[source_name])
        if source_table is None:
            continue
        url = source_name_url(source_name, source_table)
        keys = [key for key in CONFIG_REPLACEMENT_KEYS if key in source_table]
        if url == APPROVED_SOURCE_URL and keys:
            findings.append(
                R004_CONFIG_SOURCE_REPLACEMENT.format(
                    config_path=rel_config,
                    source_name=source_name,
                    keys=format_keys(keys),
                )
            )
    return findings


def collect_local_route_findings(root: Path) -> list[str]:
    findings: list[str] = []
    workspace_manifest = load_toml(root / "core" / "Cargo.toml")

    patch = as_mapping(workspace_manifest.get("patch"))
    if patch is not None:
        findings.extend(
            collect_patch_findings(
                patch,
                source_message=R001_WORKSPACE_PATCH_SOURCE,
                package_message=R002_WORKSPACE_PATCH_PACKAGE,
            )
        )

    replace = as_mapping(workspace_manifest.get("replace"))
    if replace is not None:
        for replace_key in sorted(replace):
            package = str(replace_key).split(":", 1)[0]
            if package in SPL_PACKAGE_SET:
                findings.append(
                    R003_REPLACE_PACKAGE.format(
                        replace_key=replace_key,
                        package=package,
                    )
                )

    for relative_config in CONFIG_PATHS:
        config_path = root / relative_config
        if config_path.exists():
            findings.extend(collect_config_findings(root, config_path))

    for manifest_path in sorted(tracked_cargo_manifests(root)):
        manifest = load_toml(manifest_path)
        package = as_mapping(manifest.get("package"))
        package_name = package.get("name") if package is not None else None
        if package_name in SPL_PACKAGE_SET:
            findings.append(
                R007_IN_TREE_PACKAGE_COPY.format(
                    manifest=manifest_path.relative_to(root).as_posix(),
                    package=package_name,
                )
            )

    return findings


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="SPL dependency pin guard")
    parser.add_argument(
        "--root",
        type=Path,
        default=ROOT,
        help="Repository root to scan (defaults to the checkout root).",
    )
    args = parser.parse_args(argv)

    root = args.root.resolve()
    workspace_findings, workspace_tag = collect_workspace_findings(root)
    findings = [
        *workspace_findings,
        *collect_member_findings(root),
        *collect_lockfile_findings(root, workspace_tag),
        *collect_local_route_findings(root),
    ]

    for finding in findings:
        print(finding, file=sys.stderr)

    return 1 if findings else 0


if __name__ == "__main__":
    raise SystemExit(main())
