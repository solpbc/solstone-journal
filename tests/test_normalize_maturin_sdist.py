# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import hashlib
import tarfile
import tomllib
from io import BytesIO
from pathlib import Path

import pytest

from scripts.normalize_maturin_sdist import (
    SdistLockError,
    normalize_core_sdist_workspace_lock,
)


def _source_workspace(root: Path) -> None:
    members = (
        "crates/solstone-core",
        "crates/solstone-core-journal",
        "crates/solstone-core-sol",
        "crates/solstone-core-sol-client",
        "crates/solstone-core-sol-client-cli",
        "crates/solstone-core-unused",
    )
    (root / "core").mkdir(parents=True)
    (root / "core" / "Cargo.toml").write_text(
        "[workspace]\nmembers = [\n"
        + "".join(f'    "{member}",\n' for member in members)
        + ']\nresolver = "3"\n',
        encoding="utf-8",
    )
    for member in members:
        path = root / "core" / member
        path.mkdir(parents=True)
        (path / "Cargo.toml").write_text(
            f'[package]\nname = "{Path(member).name}"\nversion = "1.2.3"\n',
            encoding="utf-8",
        )
    main = root / "core" / "crates" / "solstone-core" / "src" / "main.rs"
    main.parent.mkdir(parents=True)
    main.write_text("fn main() {}\n", encoding="utf-8")


def _archive(root: Path, *, pruned_source: bool = False) -> Path:
    archive = root / "dist" / "solstone_core-1.2.3.tar.gz"
    archive.parent.mkdir()
    manifest = (
        "[workspace]\n"
        'members = ["crates/solstone-core", "crates/solstone-core-journal", '
        '"crates/solstone-core-sol", "crates/solstone-core-sol-client", '
        '"crates/solstone-core-sol-client-cli"]\n'
        'resolver = "3"\n'
    ).encode()
    source_line = (
        '\nsource = "registry+https://github.com/rust-lang/crates.io-index"'
        if pruned_source
        else ""
    )
    lock = (
        "version = 4\n\n"
        '[[package]]\nname = "serde"\nversion = "1.0.228"\n'
        'source = "registry+https://github.com/rust-lang/crates.io-index"\n\n'
        '[[package]]\nname = "ureq"\nversion = "3.1.4"\n'
        'source = "registry+https://github.com/rust-lang/crates.io-index"\n\n'
        '[[package]]\nname = "solstone-core"\nversion = "1.2.3"\n'
        'dependencies = ["serde"]\n\n'
        '[[package]]\nname = "solstone-core-journal"\nversion = "1.2.3"\n\n'
        '[[package]]\nname = "solstone-core-sol"\nversion = "1.2.3"\n\n'
        '[[package]]\nname = "solstone-core-sol-client"\nversion = "1.2.3"\n'
        'dependencies = ["ureq"]\n\n'
        '[[package]]\nname = "solstone-core-sol-client-cli"\nversion = "1.2.3"\n'
        'dependencies = ["solstone-core-sol-client"]\n\n'
        f'[[package]]\nname = "solstone-core-unused"\nversion = "1.2.3"{source_line}\n'
    ).encode()
    with tarfile.open(archive, mode="w:gz") as target:
        for name, data in (
            ("solstone_core-1.2.3/PKG-INFO", b"metadata\n"),
            ("solstone_core-1.2.3/core/Cargo.toml", manifest),
            ("solstone_core-1.2.3/core/Cargo.lock", lock),
        ):
            member = tarfile.TarInfo(name)
            member.mode = 0o644
            member.mtime = 1_784_800_000
            member.size = len(data)
            target.addfile(member, BytesIO(data))
    return archive


def _members(archive: Path) -> dict[str, bytes]:
    with tarfile.open(archive, mode="r:gz") as source:
        return {
            member.name: source.extractfile(member).read()  # type: ignore[union-attr]
            for member in source.getmembers()
            if member.isfile()
        }


def test_normalizer_retains_now_reachable_sol_workspace_records(
    tmp_path: Path,
) -> None:
    _source_workspace(tmp_path)
    archive = _archive(tmp_path)
    before = _members(archive)

    removed = normalize_core_sdist_workspace_lock(tmp_path, archive)

    assert removed == ("solstone-core-unused",)
    after = _members(archive)
    assert after.keys() == before.keys()
    assert (
        after["solstone_core-1.2.3/PKG-INFO"] == before["solstone_core-1.2.3/PKG-INFO"]
    )
    assert (
        after["solstone_core-1.2.3/core/Cargo.toml"]
        == before["solstone_core-1.2.3/core/Cargo.toml"]
    )
    packages = tomllib.loads(after["solstone_core-1.2.3/core/Cargo.lock"].decode())[
        "package"
    ]
    assert [package["name"] for package in packages] == [
        "serde",
        "ureq",
        "solstone-core",
        "solstone-core-journal",
        "solstone-core-sol",
        "solstone-core-sol-client",
        "solstone-core-sol-client-cli",
    ]

    digest = hashlib.sha256(archive.read_bytes()).hexdigest()
    assert normalize_core_sdist_workspace_lock(tmp_path, archive) == ()
    assert hashlib.sha256(archive.read_bytes()).hexdigest() == digest


def test_normalizer_refuses_to_remove_registry_package_record(tmp_path: Path) -> None:
    _source_workspace(tmp_path)
    archive = _archive(tmp_path, pruned_source=True)

    with pytest.raises(SdistLockError, match="is not a workspace package"):
        normalize_core_sdist_workspace_lock(tmp_path, archive)


def test_normalizer_injects_native_sol_sources(tmp_path: Path) -> None:
    _source_workspace(tmp_path)
    authority = tmp_path / "solstone" / "apps" / "sample" / "native" / "authority.toml"
    authority.parent.mkdir(parents=True)
    authority.write_text("# native authority\n", encoding="utf-8")
    source = (
        tmp_path
        / "core"
        / "crates"
        / "solstone-core-sol-client"
        / "native"
        / "apps"
        / "sample"
        / "command.rs"
    )
    source.parent.mkdir(parents=True)
    source.write_text("// native source\n", encoding="utf-8")
    archive = _archive(tmp_path)

    normalize_core_sdist_workspace_lock(tmp_path, archive)

    after = _members(archive)
    assert (
        after[
            "solstone_core-1.2.3/core/crates/solstone-core-sol-client/native/apps/sample/command.rs"
        ]
        == b"// native source\n"
    )
    assert (
        after["solstone_core-1.2.3/solstone/apps/sample/native/authority.toml"]
        == b"# native authority\n"
    )

    digest = hashlib.sha256(archive.read_bytes()).hexdigest()
    assert normalize_core_sdist_workspace_lock(tmp_path, archive) == ()
    assert hashlib.sha256(archive.read_bytes()).hexdigest() == digest


def test_normalizer_injects_derived_compile_inputs(tmp_path: Path) -> None:
    _source_workspace(tmp_path)
    main = tmp_path / "core" / "crates" / "solstone-core" / "src" / "main.rs"
    main.write_text(
        'const CONTRACT: &str = include_str!("../../../fixtures/sample-contract.json");\n'
        "fn main() {}\n",
        encoding="utf-8",
    )
    asset = tmp_path / "core" / "fixtures" / "sample-contract.json"
    asset.parent.mkdir(parents=True)
    asset.write_text('{"ok":true}\n', encoding="utf-8")
    archive = _archive(tmp_path)

    normalize_core_sdist_workspace_lock(tmp_path, archive)

    after = _members(archive)
    assert (
        after["solstone_core-1.2.3/core/fixtures/sample-contract.json"]
        == b'{"ok":true}\n'
    )

    digest = hashlib.sha256(archive.read_bytes()).hexdigest()
    assert normalize_core_sdist_workspace_lock(tmp_path, archive) == ()
    assert hashlib.sha256(archive.read_bytes()).hexdigest() == digest
