#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Derive the release package inventory from Cargo and uv workspace metadata."""

from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
import tomllib
from dataclasses import asdict, dataclass
from functools import lru_cache
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parent.parent
TARGET_FAMILIES = frozenset(
    ("core", "describe", "pdf", "speakers-analyze", "vad-analyze", "vulkan-probe")
)


class ReleasePackageInventoryError(RuntimeError):
    """Raised when release package metadata is missing or contradictory."""

    def __init__(self, errors: list[str]) -> None:
        self.errors = tuple(errors)
        super().__init__("; ".join(errors))


@dataclass(frozen=True)
class PythonPackage:
    distribution: str
    version: str
    pyproject: Path
    build_backend: str


@dataclass(frozen=True)
class NativePackage:
    crate: str
    distribution: str
    version: str
    binary: str
    cargo_manifest: Path
    pyproject: Path
    target_family: str
    sdist: bool


@dataclass(frozen=True)
class ReleasePackageInventory:
    root_distribution: str
    root_version: str
    packages: tuple[PythonPackage, ...]
    native_packages: tuple[NativePackage, ...]

    @property
    def workspace_distributions(self) -> tuple[str, ...]:
        return tuple(package.distribution for package in self.packages)

    @property
    def macos_native_packages(self) -> tuple[NativePackage, ...]:
        """Native leaves whose declared coverage includes macOS/arm64."""

        return tuple(
            package
            for package in self.native_packages
            if package.target_family
            in {"core", "describe", "speakers-analyze", "vad-analyze"}
        )


def native_role(package: NativePackage) -> str:
    """Return the stable evidence role for a native package."""

    if package.distribution == "solstone-core":
        return "core"
    if package.target_family == "speakers-analyze":
        return "speakers-analyze"
    return package.distribution


def macos_native_record_name(role: str) -> str:
    """Return the build-host record filename for ``role``."""

    return f"macos-native-{role}.json"


def normalized_distribution(distribution: str) -> str:
    """Return the wheel/sdist filename normalization for a distribution name."""

    return re.sub(r"[-_.]+", "_", distribution).lower()


def _read_toml(path: Path) -> dict[str, Any]:
    try:
        return tomllib.loads(path.read_text(encoding="utf-8"))
    except (OSError, tomllib.TOMLDecodeError) as exc:
        raise ReleasePackageInventoryError([f"{path}: {exc}"]) from exc


def _project_name(path: Path, data: dict[str, Any], errors: list[str]) -> str | None:
    name = data.get("project", {}).get("name")
    if not isinstance(name, str) or not name:
        errors.append(f"{path}: [project].name must be a non-empty string")
        return None
    return name


def _project_version(path: Path, data: dict[str, Any], errors: list[str]) -> str | None:
    version = data.get("project", {}).get("version")
    if not isinstance(version, str) or not version:
        errors.append(f"{path}: [project].version must be a non-empty string")
        return None
    return version


def _workspace_member_pyprojects(
    root: Path, root_data: dict[str, Any], errors: list[str]
) -> tuple[Path, ...]:
    members = (
        root_data.get("tool", {}).get("uv", {}).get("workspace", {}).get("members")
    )
    if not isinstance(members, list) or not members:
        errors.append("pyproject.toml: [tool.uv.workspace].members must be non-empty")
        return ()

    paths: set[Path] = set()
    for pattern in members:
        if not isinstance(pattern, str) or not pattern:
            errors.append(
                "pyproject.toml: [tool.uv.workspace].members entries must be strings"
            )
            continue
        matches = sorted(root.glob(pattern))
        if not matches:
            errors.append(
                f"pyproject.toml: uv workspace member pattern matched nothing: {pattern}"
            )
            continue
        for member in matches:
            pyproject = member / "pyproject.toml"
            if not pyproject.is_file():
                errors.append(f"{member}: uv workspace member has no pyproject.toml")
                continue
            paths.add(pyproject.resolve())
    return tuple(sorted(paths))


def _cargo_metadata(root: Path, errors: list[str]) -> dict[str, Any]:
    try:
        result = subprocess.run(
            [
                "cargo",
                "metadata",
                "--manifest-path",
                str(root / "core" / "Cargo.toml"),
                "--format-version",
                "1",
                "--no-deps",
                "--locked",
                "--offline",
            ],
            cwd=root,
            capture_output=True,
            text=True,
            check=False,
        )
    except OSError as exc:
        errors.append(f"cargo metadata could not start: {exc}")
        return {}
    if result.returncode != 0:
        detail = (
            result.stderr.strip()
            or result.stdout.strip()
            or f"exit {result.returncode}"
        )
        errors.append(f"cargo metadata failed: {detail}")
        return {}
    try:
        payload = json.loads(result.stdout)
    except json.JSONDecodeError as exc:
        errors.append(f"cargo metadata returned invalid JSON: {exc}")
        return {}
    if not isinstance(payload, dict):
        errors.append("cargo metadata root must be an object")
        return {}
    return payload


def _default_binary(package: dict[str, Any], errors: list[str]) -> str | None:
    manifest_raw = package.get("manifest_path")
    if not isinstance(manifest_raw, str):
        errors.append(f"cargo package {package.get('name')!r} has no manifest_path")
        return None
    expected_source = (Path(manifest_raw).parent / "src" / "main.rs").resolve()
    matches = []
    for target in package.get("targets", []):
        if not isinstance(target, dict) or "bin" not in target.get("kind", []):
            continue
        source_raw = target.get("src_path")
        if (
            isinstance(source_raw, str)
            and Path(source_raw).resolve() == expected_source
        ):
            name = target.get("name")
            if isinstance(name, str) and name:
                matches.append(name)
    if len(matches) > 1:
        errors.append(
            f"{manifest_raw}: multiple default src/main.rs binaries: {sorted(matches)}"
        )
        return None
    return matches[0] if matches else None


@lru_cache(maxsize=None)
def load_release_package_inventory(root: Path = ROOT) -> ReleasePackageInventory:
    """Load and validate the derived package inventory for ``root``."""

    root = root.resolve()
    errors: list[str] = []
    root_pyproject = root / "pyproject.toml"
    root_data = _read_toml(root_pyproject)
    root_distribution = _project_name(root_pyproject, root_data, errors) or ""
    root_version = _project_version(root_pyproject, root_data, errors) or ""
    member_pyprojects = _workspace_member_pyprojects(root, root_data, errors)

    packages: list[PythonPackage] = []
    native_by_manifest: dict[Path, tuple[PythonPackage, dict[str, Any]]] = {}
    for pyproject in member_pyprojects:
        data = _read_toml(pyproject)
        distribution = _project_name(pyproject, data, errors)
        version = _project_version(pyproject, data, errors)
        backend = data.get("build-system", {}).get("build-backend")
        if distribution is None or version is None:
            continue
        if not isinstance(backend, str) or not backend:
            errors.append(f"{pyproject}: [build-system].build-backend is required")
            continue
        package = PythonPackage(
            distribution=distribution,
            version=version,
            pyproject=pyproject,
            build_backend=backend,
        )
        packages.append(package)
        if backend != "maturin":
            continue
        maturin = data.get("tool", {}).get("maturin", {})
        manifest_raw = maturin.get("manifest-path")
        if not isinstance(manifest_raw, str) or not manifest_raw:
            errors.append(f"{pyproject}: [tool.maturin].manifest-path is required")
            continue
        manifest = (pyproject.parent / manifest_raw).resolve()
        if manifest in native_by_manifest:
            other = native_by_manifest[manifest][0].pyproject
            errors.append(
                f"{manifest}: packaged by more than one pyproject: {other}, {pyproject}"
            )
            continue
        native_by_manifest[manifest] = (package, data)

    actual_member_names = {package.distribution for package in packages}
    sources = root_data.get("tool", {}).get("uv", {}).get("sources", {})
    if not isinstance(sources, dict):
        errors.append("pyproject.toml: [tool.uv.sources] must be a table")
        sources = {}
    for distribution in sorted(actual_member_names):
        source = sources.get(distribution)
        if source != {"workspace": True}:
            errors.append(
                "pyproject.toml: [tool.uv.sources]."
                f"{distribution} must be exactly {{ workspace = true }}"
            )
    extra_sources = sorted(
        name
        for name, source in sources.items()
        if source == {"workspace": True} and name not in actual_member_names
    )
    if extra_sources:
        errors.append(
            "pyproject.toml: workspace sources without member packages: "
            + ", ".join(extra_sources)
        )

    cargo = _cargo_metadata(root, errors)
    native_packages: list[NativePackage] = []
    seen_packaging_manifests: set[Path] = set()
    for cargo_package in cargo.get("packages", []):
        if not isinstance(cargo_package, dict):
            errors.append("cargo metadata packages entries must be objects")
            continue
        binary = _default_binary(cargo_package, errors)
        if binary is None:
            continue
        manifest_raw = cargo_package.get("manifest_path")
        crate = cargo_package.get("name")
        if not isinstance(manifest_raw, str) or not isinstance(crate, str):
            continue
        manifest = Path(manifest_raw).resolve()
        metadata = cargo_package.get("metadata")
        release_metadata = (
            metadata.get("solstone-release", {}) if isinstance(metadata, dict) else {}
        )
        skip = (
            release_metadata.get("skip") if isinstance(release_metadata, dict) else None
        )
        if skip is not None:
            if not isinstance(skip, str) or not skip.strip():
                errors.append(
                    f"{manifest}: package.metadata.solstone-release.skip must be a reason"
                )
            if manifest in native_by_manifest:
                errors.append(
                    f"{manifest}: skipped release binary is still mapped by a packaging leaf"
                )
            continue

        packaging = native_by_manifest.get(manifest)
        if packaging is None:
            errors.append(
                f"{manifest}: default binary {binary!r} has no maturin workspace package; "
                "package it or add a reasoned [package.metadata.solstone-release] skip"
            )
            continue
        package, package_data = packaging
        release = package_data.get("tool", {}).get("solstone-release", {})
        family = release.get("target-family") if isinstance(release, dict) else None
        if family not in TARGET_FAMILIES:
            errors.append(
                f"{package.pyproject}: [tool.solstone-release].target-family must be one of "
                + ", ".join(sorted(TARGET_FAMILIES))
            )
            continue
        sdist = release.get("sdist", False)
        if not isinstance(sdist, bool):
            errors.append(
                f"{package.pyproject}: [tool.solstone-release].sdist must be boolean"
            )
            continue
        seen_packaging_manifests.add(manifest)
        native_packages.append(
            NativePackage(
                crate=crate,
                distribution=package.distribution,
                version=package.version,
                binary=binary,
                cargo_manifest=manifest,
                pyproject=package.pyproject,
                target_family=family,
                sdist=sdist,
            )
        )

    stale_packaging = sorted(set(native_by_manifest) - seen_packaging_manifests)
    for manifest in stale_packaging:
        errors.append(
            f"{native_by_manifest[manifest][0].pyproject}: maturin manifest does not map "
            "a required default Cargo binary"
        )

    if errors:
        raise ReleasePackageInventoryError(errors)
    return ReleasePackageInventory(
        root_distribution=root_distribution,
        root_version=root_version,
        packages=tuple(sorted(packages, key=lambda item: item.distribution)),
        native_packages=tuple(
            sorted(native_packages, key=lambda item: item.distribution)
        ),
    )


def _json_payload(inventory: ReleasePackageInventory, root: Path) -> dict[str, Any]:
    def relative(payload: dict[str, Any], *keys: str) -> dict[str, Any]:
        for key in keys:
            payload[key] = str(Path(payload[key]).relative_to(root))
        return payload

    return {
        "root_distribution": inventory.root_distribution,
        "root_version": inventory.root_version,
        "packages": [
            relative(asdict(package), "pyproject") for package in inventory.packages
        ],
        "native_packages": [
            relative(asdict(package), "cargo_manifest", "pyproject")
            for package in inventory.native_packages
        ],
    }


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--root", type=Path, default=ROOT)
    parser.add_argument("--json", action="store_true")
    args = parser.parse_args(argv)
    root = args.root.resolve()
    try:
        inventory = load_release_package_inventory(root)
    except ReleasePackageInventoryError as exc:
        print("release package inventory is invalid", file=sys.stderr)
        for error in exc.errors:
            print(f"  ERROR: {error}", file=sys.stderr)
        return 1
    if args.json:
        print(json.dumps(_json_payload(inventory, root), sort_keys=True))
    else:
        print(
            "release package inventory is complete: "
            f"{len(inventory.native_packages)} required native packages"
        )
        for package in inventory.native_packages:
            print(
                f"  {package.crate} -> {package.distribution} "
                f"({package.target_family}, {package.binary})"
            )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
