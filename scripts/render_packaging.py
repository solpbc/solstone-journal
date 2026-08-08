#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Synchronize hand-maintained journal leaf package versions."""

from __future__ import annotations

import argparse
import logging
import re
import sys
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT))

from solstone.think.probe import (  # noqa: E402
    SOLSTONE_CORE_UNSUPPORTED_PLATFORM_MARKER,
    solstone_core_marker_pins,
    solstone_core_speakers_analyze_marker_pins,
    solstone_core_unsupported_platform_pin,
)

ROOT_PYPROJECT = ROOT / "pyproject.toml"
CPU_PYPROJECT = ROOT / "packages" / "solstone-journal" / "pyproject.toml"
CUDA_PYPROJECT = ROOT / "packages" / "solstone-journal-cuda" / "pyproject.toml"
TOMBSTONE_PIN = "solstone-journal-host==0.7.0"
CORE_UNSUPPORTED_TOMBSTONE_OVERRIDE_MARKER = "python_version < '3.12'"
SPEAKERS_ANALYZE_OVERRIDE_MARKER = "python_version < '3.12'"
HOST_PIN_RE = re.compile(r'(?P<quote>")solstone\[journal-host\]==[^"]+(?P=quote)')
CORE_UNSUPPORTED_PIN_RE = re.compile(
    r'(?P<quote>")solstone-core-unsupported-platform==[^";]+; (?P<marker>[^"]+)(?P=quote)'
)
SPEAKERS_ANALYZE_PIN_RE = re.compile(
    r'(?P<quote>")solstone-core-speakers-analyze==[^";]+; (?P<marker>[^"]+)(?P=quote)'
)
VERSION_RE = re.compile(r'(?m)^version = "[^"]+"')
TOMBSTONE_VERSION_RE = re.compile(r'(?m)^TOMBSTONE_VERSION = "[^"]+"')
CARGO_WORKSPACE_VERSION_RE = re.compile(
    r'(?ms)(?P<prefix>^\[workspace\.package\]\n(?:(?!^\[).)*?^version = )"[^"]+"'
)
CARGO_PACKAGE_BLOCK_RE = re.compile(
    r"(?ms)^\[\[package\]\]\n(?:(?!^\[\[package\]\]).)*"
)
CARGO_LOCK_NAME_RE = re.compile(r'(?m)^name = "([^"]+)"$')
CARGO_LOCK_SOURCE_RE = re.compile(r"(?m)^source = ")
CARGO_LOCK_VERSION_RE = re.compile(r'(?m)^version = "[^"]+"$')

LOGGER = logging.getLogger(__name__)


class PackagingRenderError(RuntimeError):
    """Raised when packaging metadata cannot be rendered safely."""


def _read_version(pyproject_text: str) -> str:
    data = tomllib.loads(pyproject_text)
    try:
        version = data["project"]["version"]
    except KeyError as exc:
        raise PackagingRenderError(
            "root pyproject.toml is missing [project] version"
        ) from exc
    if not isinstance(version, str) or not version:
        raise PackagingRenderError("root [project] version must be a non-empty string")
    return version


def _leaf_paths(root: Path) -> tuple[Path, Path]:
    return (
        root / "packages" / "solstone-journal" / "pyproject.toml",
        root / "packages" / "solstone-journal-cuda" / "pyproject.toml",
    )


def _core_leaf_path(root: Path) -> Path:
    return root / "packages" / "solstone-core" / "pyproject.toml"


def _command_leaf_paths(root: Path) -> tuple[Path, Path]:
    return (
        root / "packages" / "solstone-core-sol" / "pyproject.toml",
        root / "packages" / "solstone-core-journal" / "pyproject.toml",
    )


def _speakers_analyze_leaf_path(root: Path) -> Path:
    return root / "packages" / "solstone-core-speakers-analyze" / "pyproject.toml"


def _core_unsupported_tombstone_path(root: Path) -> Path:
    return (
        root / "scripts" / "solstone-core-unsupported-platform-tombstone" / "setup.py"
    )


def _rewrite_leaf(text: str, version: str) -> str:
    text, version_count = VERSION_RE.subn(f'version = "{version}"', text)
    if version_count != 1:
        raise PackagingRenderError(
            f"leaf pyproject must contain exactly one [project].version line; found {version_count}"
        )

    text, pin_count = HOST_PIN_RE.subn(f'"solstone[journal-host]=={version}"', text)
    if pin_count != 1:
        raise PackagingRenderError(
            f"leaf pyproject must contain exactly one solstone[journal-host]== pin; found {pin_count}"
        )
    text = _rewrite_speakers_analyze_pins(
        text,
        version,
        set(solstone_core_speakers_analyze_marker_pins(version)),
        "leaf pyproject",
    )
    text = _rewrite_native_pins(
        text, version, "solstone-core", "journal leaf pyproject"
    )
    text = _rewrite_native_pins(
        text, version, "solstone-core-journal", "journal leaf pyproject"
    )
    return text


def _rewrite_core_leaf(text: str, version: str) -> str:
    text, version_count = VERSION_RE.subn(f'version = "{version}"', text)
    if version_count != 1:
        raise PackagingRenderError(
            f"core leaf pyproject must contain exactly one [project].version line; found {version_count}"
        )
    if "solstone[journal-host]==" in text:
        raise PackagingRenderError(
            "core leaf pyproject must not contain a solstone[journal-host]== pin"
        )
    return text


def _rewrite_speakers_analyze_leaf(text: str, version: str) -> str:
    text, version_count = VERSION_RE.subn(f'version = "{version}"', text)
    if version_count != 1:
        raise PackagingRenderError(
            "speakers analyze leaf pyproject must contain exactly one "
            f"[project].version line; found {version_count}"
        )
    if "solstone[journal-host]==" in text:
        raise PackagingRenderError(
            "speakers analyze leaf pyproject must not contain a "
            "solstone[journal-host]== pin"
        )
    return text


def _rewrite_native_pins(
    text: str, version: str, distribution: str, context: str
) -> str:
    pin_re = re.compile(
        rf'(?P<quote>"){re.escape(distribution)}==[^";]+; '
        r'(?P<marker>[^"]+)(?P=quote)'
    )
    expected = {
        pin.replace("solstone-core==", f"{distribution}==", 1)
        for pin in solstone_core_marker_pins(version)
    }
    seen_markers: list[str] = []

    def replacement(match: re.Match[str]) -> str:
        marker = match.group("marker")
        seen_markers.append(marker)
        return (
            f"{match.group('quote')}{distribution}=={version}; "
            f"{marker}{match.group('quote')}"
        )

    rewritten, pin_count = pin_re.subn(replacement, text)
    if pin_count != len(expected):
        raise PackagingRenderError(
            f"{context} must contain exactly {len(expected)} marker-gated "
            f"{distribution}== pins; found {pin_count}"
        )
    actual = {f"{distribution}=={version}; {marker}" for marker in seen_markers}
    if actual != expected:
        raise PackagingRenderError(
            f"{context} {distribution} marker pins must be exactly "
            + ", ".join(sorted(expected))
        )
    return rewritten


def _rewrite_root_core_unsupported_pin(text: str, version: str) -> str:
    expected_base = solstone_core_unsupported_platform_pin(version)
    expected_override = (
        "solstone-core-unsupported-platform=="
        f"{version}; {CORE_UNSUPPORTED_TOMBSTONE_OVERRIDE_MARKER}"
    )
    seen_markers: list[str] = []

    def replacement(match: re.Match[str]) -> str:
        marker = match.group("marker")
        marker = (
            CORE_UNSUPPORTED_TOMBSTONE_OVERRIDE_MARKER
            if marker == CORE_UNSUPPORTED_TOMBSTONE_OVERRIDE_MARKER
            else SOLSTONE_CORE_UNSUPPORTED_PLATFORM_MARKER
        )
        seen_markers.append(marker)
        return (
            f"{match.group('quote')}solstone-core-unsupported-platform=={version}; "
            f"{marker}{match.group('quote')}"
        )

    rewritten, pin_count = CORE_UNSUPPORTED_PIN_RE.subn(replacement, text)
    if pin_count != 2:
        raise PackagingRenderError(
            "root pyproject must contain exactly two marker-gated "
            f"solstone-core-unsupported-platform== pin; found {pin_count}"
        )
    actual = {
        f"solstone-core-unsupported-platform=={version}; {marker}"
        for marker in seen_markers
    }
    if actual != {expected_base, expected_override}:
        raise PackagingRenderError(
            "root pyproject solstone-core-unsupported-platform marker pins must be "
            f"exactly {sorted((expected_base, expected_override))}"
        )
    return rewritten


def _rewrite_speakers_analyze_pins(
    text: str,
    version: str,
    expected: set[str],
    context: str,
) -> str:
    seen_markers: list[str] = []

    def replacement(match: re.Match[str]) -> str:
        marker = match.group("marker")
        seen_markers.append(marker)
        return (
            f"{match.group('quote')}solstone-core-speakers-analyze=={version}; "
            f"{marker}{match.group('quote')}"
        )

    rewritten, pin_count = SPEAKERS_ANALYZE_PIN_RE.subn(replacement, text)
    if pin_count != len(expected):
        raise PackagingRenderError(
            f"{context} must contain exactly {len(expected)} marker-gated "
            f"solstone-core-speakers-analyze== pin(s); found {pin_count}"
        )
    actual = {
        f"solstone-core-speakers-analyze=={version}; {marker}"
        for marker in seen_markers
    }
    if actual != expected:
        raise PackagingRenderError(
            f"{context} solstone-core-speakers-analyze marker pins must be exactly "
            + ", ".join(sorted(expected))
        )
    return rewritten


def _rewrite_root_speakers_analyze_override_pin(text: str, version: str) -> str:
    expected = {
        f"solstone-core-speakers-analyze=={version}; {SPEAKERS_ANALYZE_OVERRIDE_MARKER}"
    }
    return _rewrite_speakers_analyze_pins(
        text,
        version,
        expected,
        "root [tool.uv].override-dependencies",
    )


def _rewrite_tombstone_setup(text: str, version: str) -> str:
    rewritten, version_count = TOMBSTONE_VERSION_RE.subn(
        f'TOMBSTONE_VERSION = "{version}"', text
    )
    if version_count != 1:
        raise PackagingRenderError(
            "solstone-core unsupported-platform tombstone setup.py must contain "
            f"exactly one TOMBSTONE_VERSION line; found {version_count}"
        )
    return rewritten


def _rewrite_cargo_workspace_manifest(text: str, version: str) -> str:
    def replacement(match: re.Match[str]) -> str:
        return f'{match.group("prefix")}"{version}"'

    text, version_count = CARGO_WORKSPACE_VERSION_RE.subn(replacement, text)
    if version_count != 1:
        raise PackagingRenderError(
            f"core Cargo.toml must contain exactly one [workspace.package].version line; found {version_count}"
        )
    return text


def _cargo_member_package_paths(root: Path) -> dict[str, Path]:
    workspace_path = root / "core" / "Cargo.toml"
    workspace_data = tomllib.loads(workspace_path.read_text(encoding="utf-8"))
    try:
        members = workspace_data["workspace"]["members"]
    except KeyError as exc:
        raise PackagingRenderError(
            "core Cargo.toml is missing workspace.members"
        ) from exc
    if not isinstance(members, list) or not members:
        raise PackagingRenderError(
            "core Cargo.toml workspace.members must be a non-empty list"
        )

    member_paths: dict[str, Path] = {}
    for member in members:
        if not isinstance(member, str) or not member:
            raise PackagingRenderError(
                "core Cargo.toml workspace.members must be strings"
            )
        if any(char in member for char in "*?["):
            raise PackagingRenderError(
                f"core Cargo.toml workspace member must be explicit, not a glob: {member}"
            )
        member_path = Path(member)
        if member_path.is_absolute():
            raise PackagingRenderError(
                f"core Cargo.toml workspace member must be relative: {member}"
            )

        member_manifest = root / "core" / member_path / "Cargo.toml"
        member_data = tomllib.loads(member_manifest.read_text(encoding="utf-8"))
        try:
            name = member_data["package"]["name"]
        except KeyError as exc:
            raise PackagingRenderError(
                f"workspace member {member} is missing [package].name"
            ) from exc
        if not isinstance(name, str) or not name:
            raise PackagingRenderError(
                f"workspace member {member} [package].name must be a non-empty string"
            )
        if name in member_paths:
            raise PackagingRenderError(
                "core Cargo.toml workspace member package names must be unique"
            )
        member_paths[name] = member_path

    return member_paths


def _rewrite_cargo_lock(text: str, version: str, member_names: tuple[str, ...]) -> str:
    member_set = set(member_names)
    seen: set[str] = set()
    rewritten: list[str] = []
    last = 0

    for match in CARGO_PACKAGE_BLOCK_RE.finditer(text):
        block = match.group(0)
        name_matches = CARGO_LOCK_NAME_RE.findall(block)
        if len(name_matches) != 1:
            raise PackagingRenderError(
                f"Cargo.lock package block must contain exactly one name line; found {len(name_matches)}"
            )
        name = name_matches[0]
        if name not in member_set:
            continue
        if name in seen:
            raise PackagingRenderError(
                f"Cargo.lock contains duplicate block for workspace member {name}"
            )
        if CARGO_LOCK_SOURCE_RE.search(block):
            raise PackagingRenderError(
                f"Cargo.lock workspace member block must be source-less: {name}"
            )

        block, version_count = CARGO_LOCK_VERSION_RE.subn(
            f'version = "{version}"', block
        )
        if version_count != 1:
            raise PackagingRenderError(
                f"Cargo.lock workspace member {name} must contain exactly one version line; found {version_count}"
            )

        seen.add(name)
        rewritten.append(text[last : match.start()])
        rewritten.append(block)
        last = match.end()

    missing = sorted(member_set - seen)
    if missing:
        raise PackagingRenderError(
            "Cargo.lock is missing workspace member block(s): " + ", ".join(missing)
        )

    rewritten.append(text[last:])
    return "".join(rewritten)


def _render_cargo(root: Path, version: str) -> dict[Path, str]:
    cargo_manifest = root / "core" / "Cargo.toml"
    cargo_lock = root / "core" / "Cargo.lock"
    member_paths = _cargo_member_package_paths(root)
    return {
        cargo_manifest: _rewrite_cargo_workspace_manifest(
            cargo_manifest.read_text(encoding="utf-8"), version
        ),
        cargo_lock: _rewrite_cargo_lock(
            cargo_lock.read_text(encoding="utf-8"), version, tuple(member_paths)
        ),
    }


def _write_if_changed(path: Path, content: str) -> None:
    old_content = path.read_text(encoding="utf-8") if path.exists() else None
    if old_content == content:
        LOGGER.info("%s already up to date", path)
        return
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(content, encoding="utf-8")
    LOGGER.info("wrote %s", path)


def _drifted(path: Path, expected: str) -> bool:
    try:
        current = path.read_text(encoding="utf-8")
    except FileNotFoundError:
        return True
    return current != expected


def _check_root_tombstones(root: Path) -> list[str]:
    data = tomllib.loads((root / "pyproject.toml").read_text(encoding="utf-8"))
    extras = data.get("project", {}).get("optional-dependencies", {})
    errors = []
    for name in ("journal", "journal-cuda"):
        if extras.get(name) != [TOMBSTONE_PIN]:
            errors.append(
                f"[project.optional-dependencies].{name} must be exactly [{TOMBSTONE_PIN!r}]"
            )
    return errors


def render(root: Path = ROOT) -> dict[Path, str]:
    root = Path(root)
    root_text = (root / "pyproject.toml").read_text(encoding="utf-8")
    version = _read_version(root_text)
    root_text = _rewrite_native_pins(
        root_text, version, "solstone-core-sol", "root pyproject"
    )
    root_text = _rewrite_root_core_unsupported_pin(root_text, version)
    root_text = _rewrite_root_speakers_analyze_override_pin(root_text, version)
    expected = {
        root / "pyproject.toml": root_text,
        _core_leaf_path(root): _rewrite_core_leaf(
            _core_leaf_path(root).read_text(encoding="utf-8"), version
        ),
        _speakers_analyze_leaf_path(root): _rewrite_speakers_analyze_leaf(
            _speakers_analyze_leaf_path(root).read_text(encoding="utf-8"), version
        ),
        _core_unsupported_tombstone_path(root): _rewrite_tombstone_setup(
            _core_unsupported_tombstone_path(root).read_text(encoding="utf-8"),
            version,
        ),
    }
    expected.update(
        {
            path: _rewrite_core_leaf(path.read_text(encoding="utf-8"), version)
            for path in _command_leaf_paths(root)
        }
    )
    expected.update(
        {
            path: _rewrite_leaf(path.read_text(encoding="utf-8"), version)
            for path in _leaf_paths(root)
        }
    )
    expected.update(_render_cargo(root, version))
    return expected


def check(root: Path = ROOT) -> int:
    root = Path(root)
    try:
        expected = render(root)
        tombstone_errors = _check_root_tombstones(root)
    except (OSError, tomllib.TOMLDecodeError, PackagingRenderError) as exc:
        print(f"packaging metadata check failed: {exc}")
        return 1

    drifted = [
        str(path.relative_to(root))
        for path, content in expected.items()
        if _drifted(path, content)
    ]
    if drifted or tombstone_errors:
        print("packaging metadata is stale; run python3 scripts/render_packaging.py")
        for path in drifted:
            print(f"  drifted: {path}")
        for error in tombstone_errors:
            print(f"  error: {error}")
        return 1
    print("packaging metadata is up to date")
    return 0


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--check",
        action="store_true",
        help="fail if generated packaging metadata is not up to date",
    )
    args = parser.parse_args(argv)
    logging.basicConfig(level=logging.INFO, format="%(levelname)s: %(message)s")

    if args.check:
        return check()

    try:
        for path, content in render().items():
            _write_if_changed(path, content)
    except (OSError, tomllib.TOMLDecodeError, PackagingRenderError) as exc:
        LOGGER.error("%s", exc)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
