# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import io
import tarfile
from collections.abc import Callable
from pathlib import Path

import pytest

from solstone.think.providers import (
    nvattest_install,
    parakeet_install,
    rfdetr_install,
)

Extractor = Callable[[Path, Path], None]


@pytest.mark.parametrize(
    ("extract", "error_type"),
    [
        (rfdetr_install._safe_extract_tarball, rfdetr_install.RfdetrInstallError),
        (
            parakeet_install._safe_extract_tarball,
            parakeet_install.ParakeetProviderError,
        ),
        (
            nvattest_install._safe_extract_nvattest_tarball,
            nvattest_install.NvattestInstallError,
        ),
    ],
)
def test_safe_extract_tarball_rejects_symlink_escape(
    tmp_path: Path,
    extract: Extractor,
    error_type: type[Exception],
) -> None:
    outside = tmp_path / "outside"
    outside.mkdir()
    tarball = tmp_path / "symlink-escape.tar"
    with tarfile.open(tarball, "w") as archive:
        _add_dir(archive, "payload")
        _add_symlink(archive, "payload/lib", str(outside))
        _add_file(archive, "payload/lib/evil.txt", b"owned\n")

    with pytest.raises(error_type) as exc_info:
        extract(tarball, tmp_path / "dest")

    assert getattr(exc_info.value, "reason_code") == "archive_path_traversal"
    assert not (outside / "evil.txt").exists()


@pytest.mark.parametrize(
    ("extract", "error_type"),
    [
        (rfdetr_install._safe_extract_tarball, rfdetr_install.RfdetrInstallError),
        (
            parakeet_install._safe_extract_tarball,
            parakeet_install.ParakeetProviderError,
        ),
        (
            nvattest_install._safe_extract_nvattest_tarball,
            nvattest_install.NvattestInstallError,
        ),
    ],
)
def test_safe_extract_tarball_rejects_hardlink_escape(
    tmp_path: Path,
    extract: Extractor,
    error_type: type[Exception],
) -> None:
    outside = tmp_path / "outside"
    outside.mkdir()
    tarball = tmp_path / "hardlink-escape.tar"
    with tarfile.open(tarball, "w") as archive:
        _add_dir(archive, "payload")
        _add_hardlink(archive, "payload/libnvat.so", str(outside / "target"))

    with pytest.raises(error_type) as exc_info:
        extract(tarball, tmp_path / "dest")

    assert getattr(exc_info.value, "reason_code") == "archive_path_traversal"


@pytest.mark.parametrize(
    "extract",
    [
        rfdetr_install._safe_extract_tarball,
        parakeet_install._safe_extract_tarball,
        nvattest_install._safe_extract_nvattest_tarball,
    ],
)
def test_safe_extract_tarball_allows_internal_relative_nvattest_symlink(
    tmp_path: Path,
    extract: Extractor,
) -> None:
    tarball = tmp_path / "internal-symlink.tar"
    with tarfile.open(tarball, "w") as archive:
        _add_dir(archive, "payload")
        _add_dir(archive, "payload/lib")
        _add_file(archive, "payload/lib/libnvat.so.1", b"library\n")
        _add_symlink(archive, "payload/lib/libnvat.so", "libnvat.so.1")

    dest = tmp_path / "dest"
    extract(tarball, dest)

    link = dest / "payload" / "lib" / "libnvat.so"
    assert link.is_symlink()
    assert link.readlink() == Path("libnvat.so.1")
    assert link.resolve() == dest / "payload" / "lib" / "libnvat.so.1"


def _add_dir(archive: tarfile.TarFile, name: str) -> None:
    info = tarfile.TarInfo(name)
    info.type = tarfile.DIRTYPE
    info.mode = 0o755
    archive.addfile(info)


def _add_file(archive: tarfile.TarFile, name: str, data: bytes) -> None:
    info = tarfile.TarInfo(name)
    info.size = len(data)
    info.mode = 0o644
    archive.addfile(info, io.BytesIO(data))


def _add_symlink(archive: tarfile.TarFile, name: str, linkname: str) -> None:
    info = tarfile.TarInfo(name)
    info.type = tarfile.SYMTYPE
    info.mode = 0o777
    info.linkname = linkname
    archive.addfile(info)


def _add_hardlink(archive: tarfile.TarFile, name: str, linkname: str) -> None:
    info = tarfile.TarInfo(name)
    info.type = tarfile.LNKTYPE
    info.mode = 0o644
    info.linkname = linkname
    archive.addfile(info)
