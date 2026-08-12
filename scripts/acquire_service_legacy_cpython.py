#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Acquire pinned CPython runtimes for service-legacy evidence capture.

This is a hand-run, one-time-per-regeneration capture tool, not a CI
dependency. It downloads content-addressed python-build-standalone archives
into the repository-local ignored cache and writes their committed pin record.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import shutil
import stat
import sys
import tarfile
import tempfile
import urllib.request
from dataclasses import dataclass
from pathlib import Path, PurePosixPath

from service_legacy_paths import evidence_root, python_cache_root

ROOT = Path(__file__).resolve().parents[1]
CACHE_ROOT = python_cache_root()
OUTPUT = evidence_root() / "interpreters.json"
SCHEMA = "service-legacy-cpython-interpreters"
SCHEMA_VERSION = 1
PLATFORM = "linux-x86_64"
DOWNLOADS_DIR = ".downloads"
EXTRACT_DIR = ".extract"
STATE_SCHEMA_VERSION = 1


@dataclass(frozen=True)
class InterpreterPin:
    bucket: str
    declared_floor: str
    pinned_version: str
    pin_rationale: str | None
    release_tag: str
    url: str
    archive_sha256: str
    executable: str
    executable_sha256: str
    inventory_sha256: str


PINS = (
    InterpreterPin(
        bucket="cpython37",
        declared_floor="3.7",
        pinned_version="3.8.19",
        pin_rationale=(
            "python-build-standalone publishes no linux x86_64 install_only artifact "
            "for CPython 3.7; only pgo/debug full-build tar.zst variants exist through "
            "release tag 20200823 (for example, 3.7.7 at 20200418/20200517 and 3.7.9 "
            "at 20200822). All 3.7-bucket blobs are confirmed pure-stdlib (plistlib.dumps "
            "+ common 3.7+ syntax only per source review), so 3.8.19 install_only was "
            "selected as the nearest compatible pinned build. This substitution is deliberate "
            "and documented, not silent."
        ),
        release_tag="20240415",
        url=(
            "https://github.com/astral-sh/python-build-standalone/releases/download/"
            "20240415/cpython-3.8.19%2B20240415-x86_64-unknown-linux-gnu-install_only.tar.gz"
        ),
        archive_sha256="b33feb5ce0d7f9c4aca8621a9d231dfd9d2f6e26eccb56b63f07041ff573d5a5",
        executable="python/bin/python3.8",
        executable_sha256="a48a236a663868ec7cc12c12abc349687ae3ba4de1fed1ad58cb745536dab3dd",
        inventory_sha256="b201e461f249322c261004b8799044bec73d0166a0426ea06d4cb4c496b5514c",
    ),
    InterpreterPin(
        bucket="cpython39",
        declared_floor="3.9",
        pinned_version="3.9.19",
        pin_rationale=None,
        release_tag="20240415",
        url=(
            "https://github.com/astral-sh/python-build-standalone/releases/download/"
            "20240415/cpython-3.9.19%2B20240415-x86_64-unknown-linux-gnu-install_only.tar.gz"
        ),
        archive_sha256="00f698873804863dedc0e2b2c2cc4303b49ab0703af2e5883e11340cb8079d0f",
        executable="python/bin/python3.9",
        executable_sha256="3ce6ce1d62807f1da502adddca916288247a67250f7480699a9d91308c1eaafb",
        inventory_sha256="f018c25a948d20946dce010d13e7804697247ecd27eba7a9ec3a30d9a43cbd3c",
    ),
)


class AcquisitionError(RuntimeError):
    """A pinned interpreter could not be safely acquired or verified."""


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def cache_archive(pin: InterpreterPin) -> Path:
    return CACHE_ROOT / DOWNLOADS_DIR / Path(pin.url).name


def install_root(pin: InterpreterPin) -> Path:
    return CACHE_ROOT / pin.bucket


def state_path(pin: InterpreterPin) -> Path:
    return CACHE_ROOT / f"{pin.bucket}.install.json"


def assert_platform() -> None:
    if sys.platform != "linux" or os.uname().machine != "x86_64":
        raise AcquisitionError(
            "service legacy CPython pins support linux-x86_64 only; "
            f"found {sys.platform}-{os.uname().machine}"
        )


def verify_file(path: Path, expected_sha256: str, description: str) -> None:
    if not path.is_file():
        raise AcquisitionError(f"{description} is missing: {path}")
    actual_sha256 = sha256_file(path)
    if actual_sha256 != expected_sha256:
        raise AcquisitionError(
            f"{description} SHA-256 mismatch: expected {expected_sha256}, got {actual_sha256}"
        )


def archive_inventory(
    archive: Path,
) -> tuple[dict[str, tuple[str, str | None, int]], set[str]]:
    members: dict[str, tuple[str, str | None, int]] = {}
    directories: set[str] = {"python"}
    try:
        with tarfile.open(archive, mode="r:gz") as tar:
            for member in tar.getmembers():
                name = validate_member_name(member.name)
                if name in members:
                    raise AcquisitionError(f"archive contains duplicate member: {name}")
                if member.isfile():
                    members[name] = ("regular", None, stat.S_IMODE(member.mode))
                elif member.issym():
                    validate_link_target(name, member.linkname)
                    members[name] = (
                        "symlink",
                        member.linkname,
                        stat.S_IMODE(member.mode),
                    )
                else:
                    raise AcquisitionError(
                        f"archive has unsupported member type: {name}"
                    )
                path = PurePosixPath(name)
                for index in range(1, len(path.parts)):
                    directories.add(PurePosixPath(*path.parts[:index]).as_posix())
    except tarfile.TarError as exc:
        raise AcquisitionError(f"unable to read archive inventory: {archive}") from exc
    if not members:
        raise AcquisitionError(f"archive is empty: {archive}")
    return members, directories


def validate_member_name(name: str) -> str:
    path = PurePosixPath(name)
    if (
        not name
        or path.is_absolute()
        or ".." in path.parts
        or not path.parts
        or path.parts[0] != "python"
    ):
        raise AcquisitionError(f"unsafe archive member path: {name!r}")
    return path.as_posix()


def validate_link_target(member_name: str, link_target: str) -> None:
    target = PurePosixPath(link_target)
    if target.is_absolute():
        raise AcquisitionError(f"archive symlink has absolute target: {member_name}")
    resolved = list(PurePosixPath(member_name).parent.parts)
    for part in target.parts:
        if part in ("", "."):
            continue
        if part == "..":
            if len(resolved) <= 1:
                raise AcquisitionError(
                    f"archive symlink escapes payload: {member_name}"
                )
            resolved.pop()
        else:
            resolved.append(part)
    if not resolved or resolved[0] != "python":
        raise AcquisitionError(f"archive symlink escapes payload: {member_name}")


def extract_archive(archive: Path, destination: Path) -> None:
    expected_members, expected_directories = archive_inventory(archive)
    shutil.rmtree(destination, ignore_errors=True)
    destination.mkdir(parents=True, exist_ok=True)
    try:
        with tarfile.open(archive, mode="r:gz") as tar:
            for member in tar.getmembers():
                if not member.isfile():
                    continue
                target = destination / validate_member_name(member.name)
                target.parent.mkdir(parents=True, exist_ok=True)
                source = tar.extractfile(member)
                if source is None:
                    raise AcquisitionError(
                        f"archive regular member has no content: {member.name}"
                    )
                with source, target.open("wb") as handle:
                    shutil.copyfileobj(source, handle)
                target.chmod(stat.S_IMODE(member.mode))
            for member in tar.getmembers():
                if not member.issym():
                    continue
                target = destination / validate_member_name(member.name)
                validate_link_target(member.name, member.linkname)
                target.parent.mkdir(parents=True, exist_ok=True)
                os.symlink(member.linkname, target)
    except (OSError, tarfile.TarError) as exc:
        raise AcquisitionError(f"failed to safely extract {archive}") from exc
    assert_extracted_inventory(destination, expected_members, expected_directories)


def assert_extracted_inventory(
    destination: Path,
    expected_members: dict[str, tuple[str, str | None, int]],
    expected_directories: set[str],
) -> None:
    observed_members: dict[str, tuple[str, str | None, int]] = {}
    observed_directories: set[str] = set()
    for path in sorted(destination.rglob("*")):
        relative = path.relative_to(destination).as_posix()
        mode = path.lstat().st_mode
        if stat.S_ISDIR(mode):
            observed_directories.add(relative)
        elif stat.S_ISREG(mode):
            observed_members[relative] = ("regular", None, stat.S_IMODE(mode))
        elif stat.S_ISLNK(mode):
            observed_members[relative] = (
                "symlink",
                os.readlink(path),
                stat.S_IMODE(mode),
            )
        else:
            raise AcquisitionError(
                f"extracted archive has unsupported member type: {relative}"
            )
    if (
        observed_members != expected_members
        or observed_directories != expected_directories
    ):
        raise AcquisitionError(
            "extracted archive inventory does not match its declared members"
        )


def tree_fingerprint(root: Path) -> str:
    records: list[dict[str, object]] = []
    for path in sorted(root.rglob("*")):
        relative = path.relative_to(root).as_posix()
        mode = path.lstat().st_mode
        if stat.S_ISDIR(mode):
            records.append({"path": relative, "type": "directory"})
        elif stat.S_ISREG(mode):
            records.append(
                {
                    "mode": stat.S_IMODE(mode),
                    "path": relative,
                    "sha256": sha256_file(path),
                    "type": "regular",
                }
            )
        elif stat.S_ISLNK(mode):
            records.append(
                {
                    "mode": stat.S_IMODE(mode),
                    "path": relative,
                    "target": os.readlink(path),
                    "type": "symlink",
                }
            )
        else:
            raise AcquisitionError(
                f"interpreter tree has unsupported member type: {relative}"
            )
    return hashlib.sha256(
        json.dumps(records, separators=(",", ":"), sort_keys=True).encode("utf-8")
    ).hexdigest()


def state_payload(pin: InterpreterPin) -> dict[str, object]:
    return {
        "archive_sha256": pin.archive_sha256,
        "bucket": pin.bucket,
        "executable": pin.executable,
        "executable_sha256": pin.executable_sha256,
        "inventory_sha256": pin.inventory_sha256,
        "release_tag": pin.release_tag,
        "schema_version": STATE_SCHEMA_VERSION,
        "url": pin.url,
    }


def verify_installed(pin: InterpreterPin) -> bool:
    root = install_root(pin)
    executable = root / pin.executable
    sidecar = state_path(pin)
    try:
        state = json.loads(sidecar.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError):
        return False
    if not isinstance(state, dict):
        return False
    if state != state_payload(pin):
        return False
    verify_file(executable, pin.executable_sha256, f"{pin.bucket} interpreter")
    if tree_fingerprint(root) != pin.inventory_sha256:
        return False
    return True


def self_test() -> None:
    global CACHE_ROOT

    original_cache_root = CACHE_ROOT
    with tempfile.TemporaryDirectory(prefix="service-legacy-cpython-") as temporary:
        CACHE_ROOT = Path(temporary)
        try:
            root = CACHE_ROOT / "controlled"
            executable = root / "python/bin/python"
            stdlib = root / "python/lib/stdlib.py"
            executable.parent.mkdir(parents=True)
            stdlib.parent.mkdir(parents=True)
            executable.write_bytes(b"controlled interpreter\n")
            stdlib.write_bytes(b"controlled stdlib\n")
            inventory_sha256 = tree_fingerprint(root)
            pin = InterpreterPin(
                bucket="controlled",
                declared_floor="3.0",
                pinned_version="3.0.0",
                pin_rationale=None,
                release_tag="controlled",
                url="https://example.invalid/controlled.tar.gz",
                archive_sha256="a" * 64,
                executable="python/bin/python",
                executable_sha256=sha256_file(executable),
                inventory_sha256=inventory_sha256,
            )
            state_path(pin).write_text(json.dumps(state_payload(pin)), encoding="utf-8")
            if not verify_installed(pin):
                raise AcquisitionError("controlled interpreter was not accepted")

            stdlib.write_bytes(b"mutated stdlib\n")
            if verify_installed(pin):
                raise AcquisitionError("mutated cached stdlib was accepted")

            state_path(pin).write_text(
                json.dumps(
                    {
                        **state_payload(pin),
                        "inventory_sha256": tree_fingerprint(root),
                    }
                ),
                encoding="utf-8",
            )
            if verify_installed(pin):
                raise AcquisitionError(
                    "coupled cached tree and sidecar mutation was accepted"
                )

            stdlib.write_bytes(b"controlled stdlib\n")
            state_path(pin).write_text(json.dumps(state_payload(pin)), encoding="utf-8")
            (root / "python/lib/extra.py").write_bytes(b"unexpected extra file\n")
            if verify_installed(pin):
                raise AcquisitionError("extra cached interpreter file was accepted")
        finally:
            CACHE_ROOT = original_cache_root
    print("service-legacy CPython cache self-test passed", file=sys.stderr)


def download_archive(pin: InterpreterPin) -> Path:
    archive = cache_archive(pin)
    if archive.exists():
        verify_file(archive, pin.archive_sha256, f"{pin.bucket} archive")
        return archive
    archive.parent.mkdir(parents=True, exist_ok=True)
    temporary = archive.with_name(f"{archive.name}.tmp")
    temporary.unlink(missing_ok=True)
    request = urllib.request.Request(
        pin.url, headers={"User-Agent": "solstone-service-legacy-evidence"}
    )
    try:
        with (
            urllib.request.urlopen(request, timeout=120) as response,
            temporary.open("wb") as handle,
        ):
            shutil.copyfileobj(response, handle)
        verify_file(temporary, pin.archive_sha256, f"{pin.bucket} archive")
        temporary.replace(archive)
    except Exception as exc:
        temporary.unlink(missing_ok=True)
        if isinstance(exc, AcquisitionError):
            raise
        raise AcquisitionError(
            f"failed to download {pin.bucket} archive: {exc}"
        ) from exc
    return archive


def install(pin: InterpreterPin) -> str:
    if verify_installed(pin):
        return "already verified"
    archive = download_archive(pin)
    extraction = CACHE_ROOT / EXTRACT_DIR / pin.bucket
    target = install_root(pin)
    aside = CACHE_ROOT / EXTRACT_DIR / f"{pin.bucket}.aside"
    extract_archive(archive, extraction)
    executable = extraction / pin.executable
    verify_file(executable, pin.executable_sha256, f"{pin.bucket} interpreter")
    inventory_sha256 = tree_fingerprint(extraction)
    if inventory_sha256 != pin.inventory_sha256:
        raise AcquisitionError(
            f"{pin.bucket} interpreter tree SHA-256 mismatch: "
            f"expected {pin.inventory_sha256}, got {inventory_sha256}"
        )
    shutil.rmtree(aside, ignore_errors=True)
    moved_old = False
    try:
        if target.exists() or target.is_symlink():
            target.replace(aside)
            moved_old = True
        extraction.replace(target)
        temporary_state = state_path(pin).with_suffix(".tmp")
        temporary_state.write_text(
            json.dumps(state_payload(pin), indent=2, sort_keys=True) + "\n",
            encoding="utf-8",
        )
        temporary_state.replace(state_path(pin))
    except Exception:
        if target.exists() or target.is_symlink():
            shutil.rmtree(target, ignore_errors=True)
        if moved_old and aside.exists():
            aside.replace(target)
        raise
    finally:
        shutil.rmtree(CACHE_ROOT / EXTRACT_DIR, ignore_errors=True)
    return "installed"


def manifest_bucket(pin: InterpreterPin) -> dict[str, str | None]:
    return {
        "archive_sha256": pin.archive_sha256,
        "declared_floor": pin.declared_floor,
        "executable": pin.executable,
        "executable_sha256": pin.executable_sha256,
        "inventory_sha256": pin.inventory_sha256,
        "pin_rationale": pin.pin_rationale,
        "platform": PLATFORM,
        "pinned_version": pin.pinned_version,
        "release_tag": pin.release_tag,
        "url": pin.url,
    }


def manifest_payload() -> dict[str, object]:
    return {
        "schema": SCHEMA,
        "schema_version": SCHEMA_VERSION,
        "buckets": {pin.bucket: manifest_bucket(pin) for pin in PINS},
    }


def verify_manifest() -> None:
    try:
        actual = json.loads(OUTPUT.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        raise AcquisitionError(
            f"interpreter manifest is unavailable: {OUTPUT}"
        ) from exc
    if actual != manifest_payload():
        raise AcquisitionError("interpreter manifest does not match the declared pins")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--verify",
        action="store_true",
        help="verify the cached runtimes and manifest without downloading",
    )
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()
    if args.self_test:
        self_test()
        return 0
    assert_platform()
    if args.verify:
        for pin in PINS:
            if not verify_installed(pin):
                raise AcquisitionError(f"{pin.bucket} is not installed and verified")
            print(f"{pin.bucket}: verified", file=sys.stderr)
        verify_manifest()
        return 0

    for pin in PINS:
        print(f"{pin.bucket}: {install(pin)}", file=sys.stderr)
    OUTPUT.parent.mkdir(parents=True, exist_ok=True)
    OUTPUT.write_text(json.dumps(manifest_payload(), indent=2) + "\n", encoding="utf-8")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
