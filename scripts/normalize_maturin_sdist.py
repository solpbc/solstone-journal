#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Keep a Maturin workspace sdist's lock aligned with its pruned workspace."""

from __future__ import annotations

import copy
import gzip
import io
import os
import re
import stat
import tarfile
import tempfile
import tomllib
from pathlib import Path, PurePosixPath

try:
    from scripts.core_compile_inputs import (
        CoreCompileInputError,
        core_compile_input_sdist_files,
    )
except ModuleNotFoundError:  # pragma: no cover - direct script execution path.
    from core_compile_inputs import (  # type: ignore[no-redef]
        CoreCompileInputError,
        core_compile_input_sdist_files,
    )


class SdistLockError(RuntimeError):
    """The built sdist cannot be normalized without weakening its lock."""


PACKAGE_BLOCK_RE = re.compile(r"(?ms)^\[\[package\]\]\n.*?(?=^\[\[package\]\]\n|\Z)")
DEPENDENCY_RE = re.compile(
    r"^(?P<name>\S+)(?: (?P<version>\S+)(?: \((?P<source>.+)\))?)?$"
)
CORE_SDIST_GLOB_INJECTION_PATTERNS = (
    "solstone/apps/*/native/*",
    "solstone/think/native/**/*",
    "solstone/think/tools/native/**/*",
    "core/crates/solstone-core-sol-client/native/**/*",
)


def _workspace_members(manifest: bytes, *, label: str) -> tuple[str, ...]:
    try:
        data = tomllib.loads(manifest.decode("utf-8"))
    except (UnicodeDecodeError, tomllib.TOMLDecodeError) as exc:
        raise SdistLockError(f"{label} Cargo.toml is invalid: {exc}") from None
    members = data.get("workspace", {}).get("members")
    if not isinstance(members, list) or not members:
        raise SdistLockError(f"{label} Cargo.toml has no workspace members")
    if any(not isinstance(member, str) or not member for member in members):
        raise SdistLockError(f"{label} Cargo.toml has invalid workspace members")
    return tuple(members)


def _package_name(manifest: Path, *, member: str) -> str:
    try:
        data = tomllib.loads(manifest.read_text(encoding="utf-8"))
    except (OSError, UnicodeDecodeError, tomllib.TOMLDecodeError) as exc:
        raise SdistLockError(
            f"source workspace member {member} Cargo.toml is invalid: {exc}"
        ) from None
    name = data.get("package", {}).get("name")
    if not isinstance(name, str) or not name:
        raise SdistLockError(f"source workspace member {member} has no package name")
    return name


def _retain_reachable_lock_packages(
    lock_bytes: bytes,
    *,
    retained_names: frozenset[str],
    pruned_names: frozenset[str],
) -> bytes:
    try:
        text = lock_bytes.decode("utf-8")
        parsed = tomllib.loads(text)
    except (UnicodeDecodeError, tomllib.TOMLDecodeError) as exc:
        raise SdistLockError(f"sdist Cargo.lock is invalid: {exc}") from None

    packages = parsed.get("package")
    if not isinstance(packages, list) or not packages:
        raise SdistLockError("sdist Cargo.lock has no package records")
    matches = list(PACKAGE_BLOCK_RE.finditer(text))
    if len(matches) != len(packages):
        raise SdistLockError("sdist Cargo.lock package records cannot be isolated")

    identities: list[tuple[str, str, str | None]] = []
    by_name: dict[str, list[int]] = {}
    for index, package in enumerate(packages):
        if not isinstance(package, dict):
            raise SdistLockError("sdist Cargo.lock has an invalid package record")
        name = package.get("name")
        version = package.get("version")
        source = package.get("source")
        dependencies = package.get("dependencies", [])
        if (
            not isinstance(name, str)
            or not name
            or not isinstance(version, str)
            or not version
            or (source is not None and not isinstance(source, str))
            or not isinstance(dependencies, list)
            or any(not isinstance(dependency, str) for dependency in dependencies)
        ):
            raise SdistLockError("sdist Cargo.lock has an invalid package record")
        identity = (name, version, source)
        if identity in identities:
            raise SdistLockError(
                f"sdist Cargo.lock repeats package identity {name} {version}"
            )
        identities.append(identity)
        by_name.setdefault(name, []).append(index)

    roots: list[int] = []
    for name in sorted(retained_names):
        candidates = [
            index for index in by_name.get(name, []) if identities[index][2] is None
        ]
        if len(candidates) != 1:
            raise SdistLockError(
                f"sdist Cargo.lock does not identify retained workspace package {name}"
            )
        roots.append(candidates[0])

    for name in sorted(pruned_names):
        candidates = by_name.get(name, [])
        if candidates and not any(identities[index][2] is None for index in candidates):
            raise SdistLockError(
                f"sdist Cargo.lock pruned package {name} is not a workspace package"
            )

    def dependency_index(dependency: str, *, parent: str) -> int:
        match = DEPENDENCY_RE.fullmatch(dependency)
        if match is None:
            raise SdistLockError(
                f"sdist Cargo.lock dependency {dependency!r} from {parent} is invalid"
            )
        candidates = list(by_name.get(match.group("name"), []))
        version = match.group("version")
        source = match.group("source")
        if version is not None:
            candidates = [
                index for index in candidates if identities[index][1] == version
            ]
        if source is not None:
            candidates = [
                index for index in candidates if identities[index][2] == source
            ]
        if len(candidates) != 1:
            raise SdistLockError(
                f"sdist Cargo.lock dependency {dependency!r} from {parent} "
                "does not resolve uniquely"
            )
        return candidates[0]

    reachable: set[int] = set()
    pending = roots[:]
    while pending:
        index = pending.pop()
        if index in reachable:
            continue
        reachable.add(index)
        package = packages[index]
        parent = f"{identities[index][0]} {identities[index][1]}"
        pending.extend(
            dependency_index(dependency, parent=parent)
            for dependency in package.get("dependencies", [])
        )

    if len(reachable) == len(packages):
        return lock_bytes

    pieces: list[str] = []
    cursor = 0
    for index, match in enumerate(matches):
        pieces.append(text[cursor : match.start()])
        if index in reachable:
            pieces.append(match.group(0))
        cursor = match.end()
    pieces.append(text[cursor:])
    rewritten = "".join(pieces)

    try:
        normalized = tomllib.loads(rewritten)
    except tomllib.TOMLDecodeError as exc:
        raise SdistLockError(f"normalized sdist Cargo.lock is invalid: {exc}") from None
    remaining = normalized.get("package", [])
    if len(remaining) != len(reachable):
        raise SdistLockError("normalized sdist Cargo.lock package count changed")
    return rewritten.encode("utf-8")


def _read_archive(
    archive: Path,
) -> tuple[list[tuple[tarfile.TarInfo, bytes | None]], str, bytes, bytes]:
    try:
        with tarfile.open(archive, mode="r:gz") as source:
            members = source.getmembers()
            entries: list[tuple[tarfile.TarInfo, bytes | None]] = []
            names: set[str] = set()
            roots: set[str] = set()
            cargo_manifest: bytes | None = None
            cargo_lock: bytes | None = None
            for member in members:
                path = PurePosixPath(member.name)
                if path.is_absolute() or ".." in path.parts or len(path.parts) < 2:
                    raise SdistLockError(
                        f"sdist contains unsafe member path {member.name!r}"
                    )
                if member.name in names:
                    raise SdistLockError(f"sdist repeats member path {member.name!r}")
                names.add(member.name)
                roots.add(path.parts[0])
                if not (member.isfile() or member.isdir()):
                    raise SdistLockError(
                        f"sdist contains unsupported member type {member.name!r}"
                    )
                data = None
                if member.isfile():
                    stream = source.extractfile(member)
                    if stream is None:
                        raise SdistLockError(
                            f"sdist regular file is unreadable {member.name!r}"
                        )
                    data = stream.read()
                entries.append((copy.copy(member), data))
            if len(roots) != 1:
                raise SdistLockError("sdist does not have exactly one archive root")
            root = next(iter(roots))
            manifest_name = f"{root}/core/Cargo.toml"
            lock_name = f"{root}/core/Cargo.lock"
            for member, data in entries:
                if member.name == manifest_name:
                    cargo_manifest = data
                elif member.name == lock_name:
                    cargo_lock = data
            if cargo_manifest is None or cargo_lock is None:
                raise SdistLockError(
                    "sdist is missing core/Cargo.toml or core/Cargo.lock"
                )
            return entries, root, cargo_manifest, cargo_lock
    except (OSError, tarfile.TarError) as exc:
        raise SdistLockError(f"sdist archive is unreadable: {exc}") from None


def _globbed_core_sdist_injected_files(root: Path) -> dict[str, bytes]:
    files: dict[str, bytes] = {}
    for pattern in CORE_SDIST_GLOB_INJECTION_PATTERNS:
        for path in sorted(root.glob(pattern)):
            if path.is_dir():
                continue
            if path.is_symlink():
                raise SdistLockError(
                    f"native sol source member is a symlink: {path.relative_to(root)}"
                )
            if not path.is_file():
                raise SdistLockError(
                    f"native sol source member is not a regular file: {path.relative_to(root)}"
                )
            relative = path.relative_to(root).as_posix()
            if relative in files:
                continue
            files[relative] = path.read_bytes()
    return files


def core_sdist_injected_files(root: Path) -> dict[str, bytes]:
    files = _globbed_core_sdist_injected_files(root)
    try:
        compile_inputs = core_compile_input_sdist_files(root)
    except CoreCompileInputError as exc:
        raise SdistLockError(f"core compile input discovery failed: {exc}") from exc
    for relative, content in compile_inputs.items():
        existing = files.get(relative)
        if existing is not None and existing != content:
            raise SdistLockError(
                f"core sdist injected member {relative} has conflicting byte sources"
            )
        files[relative] = content
    return files


def _replace_archive_files(
    archive: Path,
    *,
    entries: list[tuple[tarfile.TarInfo, bytes | None]],
    lock_name: str,
    lock_bytes: bytes,
    extra_files: dict[str, bytes],
) -> None:
    archive_stat = archive.stat()
    existing_names = {member.name for member, _data in entries}
    descriptor, temporary_name = tempfile.mkstemp(
        dir=archive.parent, prefix=f".{archive.name}.", suffix=".tmp"
    )
    temporary = Path(temporary_name)
    try:
        with os.fdopen(descriptor, "wb") as raw:
            with gzip.GzipFile(
                filename="", mode="wb", compresslevel=9, fileobj=raw, mtime=0
            ) as compressed:
                with tarfile.open(
                    fileobj=compressed, mode="w", format=tarfile.PAX_FORMAT
                ) as target:
                    for member, original_data in entries:
                        data = (
                            lock_bytes
                            if member.name == lock_name
                            else extra_files.get(member.name, original_data)
                        )
                        if member.isfile():
                            assert data is not None
                            member.size = len(data)
                            target.addfile(member, io.BytesIO(data))
                        else:
                            target.addfile(member)
                    for name, data in sorted(extra_files.items()):
                        if name in existing_names:
                            continue
                        member = tarfile.TarInfo(name)
                        member.mode = 0o644
                        member.mtime = 0
                        member.size = len(data)
                        target.addfile(member, io.BytesIO(data))
            raw.flush()
            os.fsync(raw.fileno())
        os.chmod(temporary, stat.S_IMODE(archive_stat.st_mode))
        os.replace(temporary, archive)
    except Exception:
        temporary.unlink(missing_ok=True)
        raise


def _archive_needs_update(
    *,
    entries: list[tuple[tarfile.TarInfo, bytes | None]],
    lock_name: str,
    lock_bytes: bytes,
    extra_files: dict[str, bytes],
) -> bool:
    for member, original_data in entries:
        if member.name == lock_name and original_data != lock_bytes:
            return True
        if member.name in extra_files and original_data != extra_files[member.name]:
            return True
    existing_files = {member.name for member, _data in entries if member.isfile()}
    return any(name not in existing_files for name in extra_files)


def normalize_core_sdist_workspace_lock(root: Path, archive: Path) -> tuple[str, ...]:
    """Retain the exact locked graph reachable from Maturin's sdist workspace."""

    root = root.resolve()
    if archive.is_symlink():
        raise SdistLockError("core sdist must be a regular file directly under dist")
    archive = archive.resolve()
    expected_parent = (root / "dist").resolve()
    if archive.parent != expected_parent:
        raise SdistLockError("core sdist must be a regular file directly under dist")
    if not archive.is_file():
        raise SdistLockError("core sdist is missing or is not a regular file")

    entries, archive_root, sdist_manifest, lock_bytes = _read_archive(archive)
    source_manifest = (root / "core" / "Cargo.toml").read_bytes()
    source_members = frozenset(
        _workspace_members(source_manifest, label="source workspace")
    )
    sdist_members = frozenset(
        _workspace_members(sdist_manifest, label="sdist workspace")
    )
    if not sdist_members <= source_members:
        raise SdistLockError(
            "sdist workspace members are not a source-workspace subset"
        )
    pruned_members = source_members - sdist_members
    pruned_names = frozenset(
        _package_name(root / "core" / member / "Cargo.toml", member=member)
        for member in pruned_members
    )
    retained_names = frozenset(
        _package_name(root / "core" / member / "Cargo.toml", member=member)
        for member in sdist_members
    )
    if len(pruned_names) != len(pruned_members):
        raise SdistLockError("pruned source workspace package names are not unique")
    if len(retained_names) != len(sdist_members):
        raise SdistLockError("retained source workspace package names are not unique")
    if retained_names & pruned_names:
        raise SdistLockError("source workspace package names are not unique")
    injected_files = {
        f"{archive_root}/{relative}": content
        for relative, content in core_sdist_injected_files(root).items()
    }
    if not pruned_names:
        if _archive_needs_update(
            entries=entries,
            lock_name=f"{archive_root}/core/Cargo.lock",
            lock_bytes=lock_bytes,
            extra_files=injected_files,
        ):
            _replace_archive_files(
                archive,
                entries=entries,
                lock_name=f"{archive_root}/core/Cargo.lock",
                lock_bytes=lock_bytes,
                extra_files=injected_files,
            )
        return ()

    rewritten = _retain_reachable_lock_packages(
        lock_bytes,
        retained_names=retained_names,
        pruned_names=pruned_names,
    )
    if not _archive_needs_update(
        entries=entries,
        lock_name=f"{archive_root}/core/Cargo.lock",
        lock_bytes=rewritten,
        extra_files=injected_files,
    ):
        return ()
    _replace_archive_files(
        archive,
        entries=entries,
        lock_name=f"{archive_root}/core/Cargo.lock",
        lock_bytes=rewritten,
        extra_files=injected_files,
    )
    return tuple(sorted(pruned_names))
