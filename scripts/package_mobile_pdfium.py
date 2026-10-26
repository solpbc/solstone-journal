#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc
"""Stage the pinned mobile PDFium library and notices for app assembly."""

import argparse
import hashlib
import json
from pathlib import Path, PurePosixPath
import plistlib
import shutil
import struct
import subprocess
import tarfile
import tempfile

from android_cross_build import acquire, sha256
from mobile_native_build import require_current_pins

ROOT = Path(__file__).resolve().parents[1]
SCHEMA = "solstone.mobile-pdfium.v1"
INSTALL_ID = "@rpath/PDFium.framework/PDFium"
INPUTS = "core/distribution/mobile/inputs.json"
SOURCE_FILES = (
    "scripts/package_mobile_pdfium.py",
    "scripts/android_cross_build.py",
    "scripts/mobile_native_build.py",
    INPUTS,
    "core/crates/solstone-core-distribution/src/pdfium.rs",
    "core/crates/solstone-core-assets/src/lib.rs",
)


def sources():
    return {name: sha256(ROOT / name) for name in SOURCE_FILES}


def pin_for(platform):
    pins = json.loads((ROOT / INPUTS).read_text())["inputs"]
    require_current_pins(pins, ["pdfium"])
    return pins["pdfium-" + platform]


def archive_members(archive, platform):
    library = "lib/libpdfium." + ("so" if platform == "android" else "dylib")
    selected = {}
    with tarfile.open(archive) as bundle:
        for member in bundle:
            name = member.name
            if name != library and name != "LICENSE" and not name.startswith("licenses/"):
                continue
            if member.isdir():
                continue
            relative = PurePosixPath(name)
            if (relative.is_absolute() or ".." in relative.parts
                    or str(relative) != name or not member.isfile() or name in selected):
                raise ValueError("invalid PDFium archive member: " + name)
            with bundle.extractfile(member) as stream:
                selected[name] = stream.read()
    if not {library, "LICENSE", "licenses/pdfium.txt"} <= selected.keys():
        raise ValueError("PDFium library or license payload is missing")
    return library, selected


def inspect_android(data):
    if len(data) < 64 or data[:7] != b"\x7fELF\x02\x01\x01":
        raise ValueError("PDFium must be a little-endian ELF64 library")
    kind, machine = struct.unpack_from("<HH", data, 16)
    offset = struct.unpack_from("<Q", data, 32)[0]
    size, count = struct.unpack_from("<HH", data, 54)
    if kind != 3 or machine != 183 or size != 56 or not count or offset + size * count > len(data):
        raise ValueError("PDFium must be an AArch64 shared library")
    loads = []
    dynamic = None
    for index in range(count):
        row = struct.unpack_from("<IIQQQQQQ", data, offset + index * size)
        tag, _, file_offset, address, _, file_size, _, alignment = row
        if file_offset + file_size > len(data):
            raise ValueError("truncated PDFium ELF segment")
        if tag == 1:
            if alignment < 16384 or file_offset % 16384 != address % 16384:
                raise ValueError("PDFium does not support 16 KiB load pages")
            loads.append((address, file_offset, file_size))
        if tag == 2:
            dynamic = (file_offset, file_size)
    if not loads or dynamic is None or dynamic[1] % 16:
        raise ValueError("PDFium dynamic load closure is missing")
    needed, string_address, string_size = [], None, None
    terminated = False
    for at in range(dynamic[0], dynamic[0] + dynamic[1], 16):
        tag, value = struct.unpack_from("<qQ", data, at)
        if tag == 0:
            terminated = True
            break
        if tag == 1:
            needed.append(value)
        elif tag == 5:
            string_address = value
        elif tag == 10:
            string_size = value
    if not terminated or string_address is None or not string_size or not needed:
        raise ValueError("PDFium ELF dependencies are unavailable")
    tables = [file_offset + string_address - address
              for address, file_offset, length in loads
              if address <= string_address and string_address + string_size <= address + length]
    if len(tables) != 1:
        raise ValueError("PDFium ELF string table is invalid")
    table = data[tables[0]:tables[0] + string_size]
    dependencies = []
    for index in needed:
        end = table.find(b"\0", index)
        if index >= len(table) or end < 0:
            raise ValueError("PDFium ELF dependency is invalid")
        dependencies.append(table[index:end].decode("ascii"))
    if not set(dependencies) <= {"libc.so", "libm.so", "libdl.so", "liblog.so"}:
        raise ValueError("PDFium has an unstaged native dependency")
    return {"architecture": "aarch64", "load_page_bytes": 16384,
            "dependencies": dependencies}


def inspect_ios(data, expected_id):
    if len(data) < 32 or data[:4] != b"\xcf\xfa\xed\xfe":
        raise ValueError("PDFium must be a little-endian Mach-O64 library")
    cpu, _, kind, count, command_bytes = struct.unpack_from("<IIIII", data, 4)
    if cpu != 0x100000C or kind != 6 or not count or 32 + command_bytes > len(data):
        raise ValueError("PDFium must be an arm64 dynamic library")
    cursor, versions, identifiers, dependencies = 32, [], [], []
    for _ in range(count):
        if cursor + 8 > 32 + command_bytes:
            raise ValueError("truncated PDFium load command")
        tag, length = struct.unpack_from("<II", data, cursor)
        if length < 8 or cursor + length > 32 + command_bytes:
            raise ValueError("invalid PDFium load command length")
        if tag == 0x32:
            if length < 24:
                raise ValueError("PDFium platform version is truncated")
            platform, minimum = struct.unpack_from("<II", data, cursor + 8)
            if platform != 2 or minimum == 0 or minimum > (26 << 16):
                raise ValueError("PDFium requires a supported device iOS platform/version")
            versions.append(minimum)
        if tag in (0xD, 0xC, 0x80000018, 0x8000001F, 0x20, 0x80000023):
            if length < 24:
                raise ValueError("PDFium dylib command is truncated")
            start = struct.unpack_from("<I", data, cursor + 8)[0]
            end = data.find(b"\0", cursor + start, cursor + length)
            if start < 24 or start >= length or end < 0:
                raise ValueError("PDFium dylib name is invalid")
            name = data[cursor + start:end].decode("utf-8")
            (identifiers if tag == 0xD else dependencies).append(name)
        cursor += length
    if cursor != 32 + command_bytes or len(versions) != 1 or identifiers != [expected_id]:
        raise ValueError("PDFium platform or install identity is missing")
    if not dependencies or any(
        not name.startswith(("/System/Library/", "/usr/lib/"))
        or ".." in PurePosixPath(name).parts for name in dependencies
    ):
        raise ValueError("PDFium has an unstaged framework dependency")
    return {"architecture": "arm64", "platform": "ios-device",
            "minimum_os": [versions[0] >> 16, versions[0] >> 8 & 255, versions[0] & 255],
            "install_id": expected_id, "dependencies": dependencies}


def member(path, relative):
    return {"path": relative, "sha256": sha256(path), "size_bytes": path.stat().st_size}


def validate_staged(output, platform):
    output = Path(output)
    record = json.loads((output / "receipt.json").read_text())
    pin = pin_for(platform)
    if (record["schema"] != SCHEMA or record["platform"] != platform
            or record["pin"] != pin or record["sources"] != sources()):
        raise ValueError("PDFium staging receipt does not match the current build inputs")
    archive = Path(record["archive"])
    if not archive.is_file() or sha256(archive) != pin["sha256"]:
        raise ValueError("PDFium staging archive identity changed")
    original, selected = archive_members(archive, platform)
    if record["original_library_sha256"] != hashlib.sha256(selected[original]).hexdigest():
        raise ValueError("PDFium source library identity changed")
    library = "libpdfium.so" if platform == "android" else "PDFium.framework/PDFium"
    expected = {library} | {"notices/" + name for name in selected if name != original}
    if platform == "ios":
        expected.add("PDFium.framework/Info.plist")
    rows = record["members"]
    if len(rows) != len(expected) or {row["path"] for row in rows} != expected:
        raise ValueError("PDFium staging member paths changed")
    for row in rows:
        path = output / row["path"]
        if path.is_symlink() or not path.is_file() or member(path, row["path"]) != row:
            raise ValueError("PDFium staged member changed: " + row["path"])
        name = row["path"].removeprefix("notices/")
        if row["path"].startswith("notices/") and path.read_bytes() != selected[name]:
            raise ValueError("PDFium staged notice differs from its archive")
    if output.is_symlink() or any(p.is_symlink() for p in output.rglob("*")):
        raise ValueError("PDFium staging contains a symlink")
    actual = {p.relative_to(output).as_posix() for p in output.rglob("*") if p.is_file()}
    if actual != expected | {"receipt.json"}:
        raise ValueError("unexpected PDFium staging member")
    data = (output / library).read_bytes()
    inspection = inspect_android(data) if platform == "android" else inspect_ios(data, INSTALL_ID)
    if inspection != record["inspection"]:
        raise ValueError("PDFium staging inspection changed")
    if platform == "android" and data != selected[original]:
        raise ValueError("Android PDFium differs from the pinned library")
    return record


def stage(platform, cache, output):
    cache, output = Path(cache).resolve(), Path(output).resolve()
    cache.mkdir(parents=True, exist_ok=True)
    if output.exists():
        return validate_staged(output, platform)
    pin = pin_for(platform)
    archive_pin = {**pin, "filename": "pdfium-" + platform + "-" + pin["sha256"] + ".tar.gz"}
    archive = acquire(archive_pin, cache)
    original, selected = archive_members(archive, platform)
    inspection = (inspect_android(selected[original]) if platform == "android"
                  else inspect_ios(selected[original], "./libpdfium.dylib"))
    output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix=output.name + ".staging-", dir=output.parent) as temporary:
        staging = Path(temporary)
        library = "libpdfium.so" if platform == "android" else "PDFium.framework/PDFium"
        binary = staging / library
        binary.parent.mkdir(parents=True, exist_ok=True)
        binary.write_bytes(selected[original])
        binary.chmod(0o755)
        paths = [library]
        for name, data in selected.items():
            if name == original:
                continue
            relative = "notices/" + name
            destination = staging / relative
            destination.parent.mkdir(parents=True, exist_ok=True)
            destination.write_bytes(data)
            paths.append(relative)
        commands = []
        if platform == "ios":
            subprocess.run(["install_name_tool", "-id", INSTALL_ID, str(binary)], check=True)
            commands.append(["install_name_tool", "-id", INSTALL_ID, library])
            inspection = inspect_ios(binary.read_bytes(), INSTALL_ID)
            info = {
                "CFBundleIdentifier": "app.solstone.native.pdfium", "CFBundleName": "PDFium",
                "CFBundleExecutable": "PDFium", "CFBundlePackageType": "FMWK",
                "CFBundleVersion": "1", "CFBundleShortVersionString": "1.0",
                "MinimumOSVersion": ".".join(map(str, inspection["minimum_os"])),
                "CFBundleSupportedPlatforms": ["iPhoneOS"],
            }
            relative = "PDFium.framework/Info.plist"
            (staging / relative).write_bytes(plistlib.dumps(info))
            paths.append(relative)
        record = {
            "schema": SCHEMA, "platform": platform, "pin": pin, "sources": sources(),
            "archive": str(archive),
            "original_library_sha256": hashlib.sha256(selected[original]).hexdigest(),
            "inspection": inspection, "transformations": commands,
            "members": [member(staging / name, name) for name in sorted(paths)],
        }
        (staging / "receipt.json").write_text(json.dumps(record, indent=2) + "\n")
        validate_staged(staging, platform)
        staging.rename(output)
    return record


def android_members(output, runtime):
    output, runtime = Path(output), Path(runtime)
    record = validate_staged(output, "android")
    library = runtime / "lib/arm64-v8a/libpdfium.so"
    if library.is_symlink() or not library.is_file() or library.read_bytes() != (output / "libpdfium.so").read_bytes():
        raise ValueError("Android app does not contain the admitted PDFium library")
    return [(output / row["path"], "assets/pdfium-notices/" + row["path"].removeprefix("notices/"))
            for row in record["members"] if row["path"].startswith("notices/")]


def verify_android_archive(output, archive):
    record = validate_staged(output, "android")
    names = archive.namelist()
    for row in record["members"]:
        name = ("lib/arm64-v8a/libpdfium.so" if row["path"] == "libpdfium.so"
                else "assets/pdfium-notices/" + row["path"].removeprefix("notices/"))
        if names.count(name) != 1:
            raise ValueError("PDFium package member missing or duplicated: " + name)
        with archive.open(name) as stream:
            if hashlib.file_digest(stream, "sha256").hexdigest() != row["sha256"]:
                raise ValueError("PDFium package member changed: " + name)


def embed_ios(output, app):
    output, app = Path(output), Path(app).resolve(strict=True)
    record = validate_staged(output, "ios")
    # These generated subtrees belong to this packager. Replace them so an older
    # framework signature or notice cannot survive a new assembly.
    destinations = [app / "Frameworks/PDFium.framework", app / "PDFiumNotices"]
    for destination in destinations:
        if any(parent.is_symlink() for parent in [destination, *destination.parents]
               if parent.is_relative_to(app)):
            raise ValueError("PDFium app destination contains a symlink")
        if destination.exists():
            shutil.rmtree(destination)
    for row in record["members"]:
        relative = row["path"]
        target = app / ("PDFiumNotices/" + relative.removeprefix("notices/")
                        if relative.startswith("notices/") else "Frameworks/" + relative)
        if target.is_symlink():
            raise ValueError("PDFium app destination is a symlink")
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(output / relative, target)
        target.chmod(0o755 if relative == "PDFium.framework/PDFium" else 0o644)
        if sha256(target) != row["sha256"]:
            raise ValueError("PDFium changed during app embedding")
    return record


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--platform", required=True, choices=("android", "ios"))
    parser.add_argument("--cache-dir", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    record = stage(args.platform, args.cache_dir, args.output)
    print(json.dumps({"platform": args.platform, "output": str(args.output),
                      "archive_sha256": record["pin"]["sha256"]}))


if __name__ == "__main__":
    main()
