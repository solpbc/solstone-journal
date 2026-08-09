# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Shared release wheel fixture builders."""

from __future__ import annotations

import base64
import hashlib
import re
import struct
import zipfile
from collections.abc import Mapping, Sequence
from pathlib import Path

import scripts.check_wheel_contents as checker
from scripts.build_nvattest_authority import render_nvattest_authority_json
from scripts.release_package_inventory import NativePackage, normalized_distribution
from solstone.think.probe import (
    SOLSTONE_CORE_SPEAKERS_ANALYZE_PLATFORM_TAGS,
)

# Derived rather than written out: the fixture's default tag should track
# the helper's declared coverage. Callers still pass deliberately wrong
# tags for negative tests.
SPEAKERS_ANALYZE_DEFAULT_TAG = SOLSTONE_CORE_SPEAKERS_ANALYZE_PLATFORM_TAGS[
    ("linux", "x86_64")
]

ELF_HEADER_SIZE = 64
ELF_PROGRAM_HEADER_SIZE = 56
NVATTEST_AUTHORITY_BYTES = render_nvattest_authority_json().encode("utf-8")
ROOT_LAUNCHER_BYTES = {
    name: f"#!/bin/sh\necho fixture {name}\n".encode("utf-8")
    for name in checker.ROOT_LAUNCHER_NAMES
}


def record_hash(content: bytes) -> str:
    digest = hashlib.sha256(content).digest()
    encoded = base64.urlsafe_b64encode(digest).decode("ascii").rstrip("=")
    return f"sha256={encoded}"


def _write_member(
    wheel: zipfile.ZipFile,
    name: str,
    content: bytes,
    *,
    mode: int = 0o644,
) -> None:
    info = zipfile.ZipInfo(name)
    info.external_attr = mode << 16
    wheel.writestr(info, content)


def write_support_wheel(path: Path, *, name: str, version: str) -> Path:
    path.mkdir(parents=True, exist_ok=True)
    distribution = re.sub(r"[-.]+", "_", name)
    wheel_path = path / f"{distribution}-{version}-py3-none-any.whl"
    dist_info = f"{distribution}-{version}.dist-info"
    members = {
        f"{dist_info}/METADATA": f"Name: {name}\nVersion: {version}\n".encode("utf-8"),
        f"{dist_info}/WHEEL": b"Wheel-Version: 1.0\n",
    }
    rows = [
        f"{member},{record_hash(content)},{len(content)}"
        for member, content in members.items()
    ]
    rows.append(f"{dist_info}/RECORD,,")
    with zipfile.ZipFile(wheel_path, "w") as wheel:
        for member, content in members.items():
            _write_member(wheel, member, content)
        _write_member(wheel, f"{dist_info}/RECORD", "\n".join(rows).encode("utf-8"))
    return wheel_path


def minimal_elf(
    machine: int, *, program_type: int = 1, dynamic_needed: bool = False
) -> bytes:
    dynamic_offset = ELF_HEADER_SIZE + ELF_PROGRAM_HEADER_SIZE
    dynamic_size = 32 if dynamic_needed else 0
    content = bytearray(dynamic_offset + dynamic_size)
    content[:16] = b"\x7fELF\x02\x01\x01" + b"\0" * 9
    struct.pack_into("<H", content, 16, 2)
    struct.pack_into("<H", content, 18, machine)
    struct.pack_into("<I", content, 20, 1)
    struct.pack_into("<Q", content, 32, ELF_HEADER_SIZE)
    struct.pack_into("<H", content, 52, ELF_HEADER_SIZE)
    struct.pack_into("<H", content, 54, ELF_PROGRAM_HEADER_SIZE)
    struct.pack_into("<H", content, 56, 1)
    struct.pack_into("<I", content, ELF_HEADER_SIZE, program_type)
    if dynamic_needed:
        struct.pack_into("<Q", content, ELF_HEADER_SIZE + 8, dynamic_offset)
        struct.pack_into("<Q", content, ELF_HEADER_SIZE + 32, dynamic_size)
        struct.pack_into("<qQ", content, dynamic_offset, checker.DT_NEEDED, 1)
        struct.pack_into("<qQ", content, dynamic_offset + 16, checker.DT_NULL, 0)
    return bytes(content)


def speakers_analyze_elf(
    machine: int,
    *,
    needed: Sequence[str] = ("libonnxruntime.so.1", "libc.so.6"),
    runpath: str | None = checker.SPEAKERS_ANALYZE_RUNTIME_LINK_CONTRACTS[
        ("linux", "x86_64")
    ].rpath,
    rpath: str | None = None,
    include_interp: bool = True,
    glibc: str = "2.27",
) -> bytes:
    phnum = 3 if include_interp else 2
    phoff = ELF_HEADER_SIZE
    headers_end = ELF_HEADER_SIZE + ELF_PROGRAM_HEADER_SIZE * phnum
    interp = b"/lib64/ld-linux-x86-64.so.2\0"
    interp_offset = headers_end
    dynamic_offset = interp_offset + (len(interp) if include_interp else 0)

    dynstr = bytearray(b"\0")
    needed_offsets: list[int] = []
    for value in needed:
        needed_offsets.append(len(dynstr))
        dynstr.extend(value.encode("utf-8") + b"\0")
    runpath_offset: int | None = None
    if runpath is not None:
        runpath_offset = len(dynstr)
        dynstr.extend(runpath.encode("utf-8") + b"\0")
    rpath_offset: int | None = None
    if rpath is not None:
        rpath_offset = len(dynstr)
        dynstr.extend(rpath.encode("utf-8") + b"\0")

    dynamic_entries = 1 + len(needed_offsets) + int(runpath_offset is not None)
    dynamic_entries += int(rpath_offset is not None) + 1
    dynamic_size = dynamic_entries * 16
    dynstr_offset = dynamic_offset + dynamic_size
    glibc_marker = f"\0GLIBC_{glibc}\0".encode("ascii")
    total_size = dynstr_offset + len(dynstr) + len(glibc_marker)
    base_vaddr = 0x400000
    content = bytearray(total_size)
    content[:16] = b"\x7fELF\x02\x01\x01" + b"\0" * 9
    struct.pack_into("<H", content, 16, 2)
    struct.pack_into("<H", content, 18, machine)
    struct.pack_into("<I", content, 20, 1)
    struct.pack_into("<Q", content, 32, phoff)
    struct.pack_into("<H", content, 52, ELF_HEADER_SIZE)
    struct.pack_into("<H", content, 54, ELF_PROGRAM_HEADER_SIZE)
    struct.pack_into("<H", content, 56, phnum)

    def pack_phdr(index: int, p_type: int, offset: int, size: int, flags: int) -> None:
        phdr = phoff + ELF_PROGRAM_HEADER_SIZE * index
        struct.pack_into("<I", content, phdr, p_type)
        struct.pack_into("<I", content, phdr + 4, flags)
        struct.pack_into("<Q", content, phdr + 8, offset)
        struct.pack_into("<Q", content, phdr + 16, base_vaddr + offset)
        struct.pack_into("<Q", content, phdr + 24, base_vaddr + offset)
        struct.pack_into("<Q", content, phdr + 32, size)
        struct.pack_into("<Q", content, phdr + 40, size)
        struct.pack_into("<Q", content, phdr + 48, 8)

    pack_phdr(0, checker.PT_LOAD, 0, total_size, 5)
    next_index = 1
    if include_interp:
        pack_phdr(next_index, checker.PT_INTERP, interp_offset, len(interp), 4)
        content[interp_offset : interp_offset + len(interp)] = interp
        next_index += 1
    pack_phdr(next_index, checker.PT_DYNAMIC, dynamic_offset, dynamic_size, 6)

    cursor = dynamic_offset
    struct.pack_into(
        "<qQ", content, cursor, checker.DT_STRTAB, base_vaddr + dynstr_offset
    )
    cursor += 16
    for offset in needed_offsets:
        struct.pack_into("<qQ", content, cursor, checker.DT_NEEDED, offset)
        cursor += 16
    if runpath_offset is not None:
        struct.pack_into("<qQ", content, cursor, checker.DT_RUNPATH, runpath_offset)
        cursor += 16
    if rpath_offset is not None:
        struct.pack_into("<qQ", content, cursor, checker.DT_RPATH, rpath_offset)
        cursor += 16
    struct.pack_into("<qQ", content, cursor, checker.DT_NULL, 0)
    content[dynstr_offset : dynstr_offset + len(dynstr)] = dynstr
    marker_offset = dynstr_offset + len(dynstr)
    content[marker_offset : marker_offset + len(glibc_marker)] = glibc_marker
    return bytes(content)


def minimal_macho(cputype: int) -> bytes:
    content = bytearray(32)
    struct.pack_into("<I", content, 0, checker.MH_MAGIC_64)
    struct.pack_into("<I", content, 4, cputype)
    return bytes(content)


def _macho_command_size(size: int) -> int:
    return (size + 7) & ~7


def _macho_rpath_command(path: str) -> bytes:
    encoded = path.encode("utf-8") + b"\0"
    cmdsize = _macho_command_size(12 + len(encoded))
    command = bytearray(cmdsize)
    struct.pack_into("<III", command, 0, checker.LC_RPATH, cmdsize, 12)
    command[12 : 12 + len(encoded)] = encoded
    return bytes(command)


def _macho_load_dylib_command(name: str) -> bytes:
    encoded = name.encode("utf-8") + b"\0"
    cmdsize = _macho_command_size(24 + len(encoded))
    command = bytearray(cmdsize)
    struct.pack_into("<IIIIII", command, 0, checker.LC_LOAD_DYLIB, cmdsize, 24, 0, 0, 0)
    command[24 : 24 + len(encoded)] = encoded
    return bytes(command)


def speakers_analyze_macho(
    *,
    cputype: int = checker.CPU_TYPE_ARM64,
    rpath: str | None = checker.SPEAKERS_ANALYZE_RUNTIME_LINK_CONTRACTS[
        ("darwin", "arm64")
    ].rpath,
    load_dylib: str | None = checker.SPEAKERS_ANALYZE_RUNTIME_LINK_CONTRACTS[
        ("darwin", "arm64")
    ].runtime_load,
) -> bytes:
    commands: list[bytes] = []
    if load_dylib is not None:
        commands.append(_macho_load_dylib_command(load_dylib))
    if rpath is not None:
        commands.append(_macho_rpath_command(rpath))
    sizeofcmds = sum(len(command) for command in commands)
    content = bytearray(32 + sizeofcmds)
    struct.pack_into("<I", content, 0, checker.MH_MAGIC_64)
    struct.pack_into("<I", content, 4, cputype)
    struct.pack_into("<I", content, 12, 2)
    struct.pack_into("<I", content, 16, len(commands))
    struct.pack_into("<I", content, 20, sizeofcmds)
    cursor = 32
    for command in commands:
        content[cursor : cursor + len(command)] = command
        cursor += len(command)
    return bytes(content)


def minimal_fat_macho(cputypes: list[int]) -> bytes:
    content = bytearray(8 + checker.FAT_ARCH_SIZE * len(cputypes))
    struct.pack_into(">I", content, 0, checker.FAT_MAGIC)
    struct.pack_into(">I", content, 4, len(cputypes))
    for index, cputype in enumerate(cputypes):
        struct.pack_into(">I", content, 8 + checker.FAT_ARCH_SIZE * index, cputype)
    return bytes(content)


def write_core_wheel(
    path: Path,
    *,
    tag: str = "manylinux_2_17_x86_64.manylinux2014_x86_64",
    executable: bool = True,
    record_ok: bool = True,
    script_names: Sequence[str] | None = None,
    binary: bytes | None = None,
    binaries: Mapping[str, bytes] | None = None,
    extra_members: Mapping[str, bytes] | None = None,
    version: str = "1.2.3",
) -> Path:
    wheel_path = path / f"solstone_core-{version}-py3-none-{tag}.whl"
    if binary is None:
        if "aarch64" in tag:
            binary = minimal_elf(checker.ELF_MACHINE["aarch64"])
        elif "macosx" in tag:
            binary = minimal_macho(checker.CPU_TYPE_ARM64)
        else:
            binary = minimal_elf(checker.ELF_MACHINE["x86_64"])
    if script_names is None:
        script_names = tuple(
            f"solstone_core-{version}.data/scripts/{name}"
            for name in checker.CORE_SCRIPT_NAMES
        )
    members = {
        f"solstone_core-{version}.dist-info/METADATA": (
            f"Name: solstone-core\nVersion: {version}\n".encode()
        ),
        f"solstone_core-{version}.dist-info/WHEEL": b"Wheel-Version: 1.0\n",
        f"solstone_core-{version}.dist-info/sboms/solstone-core.cyclonedx.json": b"{}",
    }
    for script_name in script_names:
        members[script_name] = binaries.get(script_name, binary) if binaries else binary
    if extra_members:
        members.update(extra_members)
    rows = [
        f"{name},{record_hash(content)},{len(content)}"
        for name, content in members.items()
    ]
    rows.append(f"solstone_core-{version}.dist-info/RECORD,,")
    record = "\n".join(rows).encode()
    if not record_ok:
        record = record.replace(b"sha256=", b"sha256=broken", 1)
    with zipfile.ZipFile(wheel_path, "w") as wheel:
        for name, content in members.items():
            mode = (
                0o755
                if Path(name).name in checker.CORE_SCRIPT_NAMES and executable
                else 0o644
            )
            _write_member(wheel, name, content, mode=mode)
        _write_member(wheel, f"solstone_core-{version}.dist-info/RECORD", record)
    return wheel_path


def write_native_binary_wheel(
    path: Path,
    *,
    package: NativePackage,
    tag: str,
    binary: bytes | None = None,
) -> Path:
    path.mkdir(parents=True, exist_ok=True)
    distribution = normalized_distribution(package.distribution)
    wheel_path = path / (f"{distribution}-{package.version}-py3-none-{tag}.whl")
    if binary is None:
        if "macosx" in tag:
            binary = minimal_macho(checker.CPU_TYPE_ARM64)
        elif "aarch64" in tag:
            binary = minimal_elf(checker.ELF_MACHINE["aarch64"])
        else:
            binary = minimal_elf(checker.ELF_MACHINE["x86_64"])
    data_prefix = f"{distribution}-{package.version}.data"
    dist_info = f"{distribution}-{package.version}.dist-info"
    members = {
        f"{data_prefix}/scripts/{package.binary}": binary,
        f"{dist_info}/METADATA": (
            f"Name: {package.distribution}\nVersion: {package.version}\n".encode()
        ),
        f"{dist_info}/WHEEL": b"Wheel-Version: 1.0\n",
        f"{dist_info}/sboms/{package.crate}.cyclonedx.json": b"{}",
    }
    rows = [
        f"{name},{record_hash(content)},{len(content)}"
        for name, content in members.items()
    ]
    rows.append(f"{dist_info}/RECORD,,")
    with zipfile.ZipFile(wheel_path, "w") as wheel:
        for name, content in members.items():
            mode = 0o755 if Path(name).name == package.binary else 0o644
            _write_member(wheel, name, content, mode=mode)
        _write_member(
            wheel,
            f"{dist_info}/RECORD",
            "\n".join(rows).encode("utf-8"),
        )
    return wheel_path


def write_speakers_analyze_wheel(
    path: Path,
    *,
    tag: str = SPEAKERS_ANALYZE_DEFAULT_TAG,
    version: str = "1.2.3",
    binary: bytes | None = None,
    library: bytes = b"fixture libonnxruntime.so.1 GLIBC_2.27\n",
    license_notice: bytes = b"fixture license\n",
    third_party_notice: bytes = b"fixture third party notice\n",
    executable: bool = True,
    extra_members: Mapping[str, bytes] | None = None,
    omit_member: str | None = None,
    record_ok: bool = True,
) -> Path:
    path.mkdir(parents=True, exist_ok=True)
    wheel_path = path / f"solstone_core_speakers_analyze-{version}-py3-none-{tag}.whl"
    data_prefix = f"solstone_core_speakers_analyze-{version}.data"
    dist_info_prefix = f"solstone_core_speakers_analyze-{version}.dist-info"
    platform_tuple = checker.SPEAKERS_ANALYZE_TAG_PLATFORMS[tag]
    spec = checker.SPEAKERS_ANALYZE_TARGETS[
        checker.SPEAKERS_ANALYZE_PLATFORM_TARGETS[platform_tuple]
    ]
    if binary is None:
        if platform_tuple[0] == "darwin":
            binary = speakers_analyze_macho()
        else:
            machine = (
                checker.ELF_MACHINE["aarch64"]
                if platform_tuple[1] == "aarch64"
                else checker.ELF_MACHINE["x86_64"]
            )
            binary = speakers_analyze_elf(machine)
    members = {
        f"{data_prefix}/{checker.SPEAKERS_ANALYZE_RUNTIME_INSTALL_DIR.as_posix()}/{spec.runtime_staged_name}": library,
        f"{data_prefix}/{checker.SPEAKERS_ANALYZE_NOTICE_INSTALL_DIR.as_posix()}/onnxruntime-LICENSE.txt": license_notice,
        f"{data_prefix}/{checker.SPEAKERS_ANALYZE_NOTICE_INSTALL_DIR.as_posix()}/onnxruntime-ThirdPartyNotices.txt": third_party_notice,
        f"{data_prefix}/scripts/solstone-core-speakers-analyze": binary,
        f"{dist_info_prefix}/METADATA": (
            f"Name: solstone-core-speakers-analyze\nVersion: {version}\n".encode()
        ),
        f"{dist_info_prefix}/WHEEL": b"Wheel-Version: 1.0\n",
        f"{dist_info_prefix}/sboms/solstone-core-speakers-analyze.cyclonedx.json": b"{}",
    }
    if extra_members:
        members.update(extra_members)
    if omit_member is not None:
        members.pop(omit_member, None)
    rows = [
        f"{name},{record_hash(content)},{len(content)}"
        for name, content in members.items()
    ]
    rows.append(f"{dist_info_prefix}/RECORD,,")
    record = "\n".join(rows).encode("utf-8")
    if not record_ok:
        record = record.replace(b"sha256=", b"sha256=broken", 1)
    with zipfile.ZipFile(wheel_path, "w") as wheel:
        for name, content in members.items():
            mode = (
                0o755
                if Path(name).name in checker.SPEAKERS_ANALYZE_SCRIPT_NAMES
                and executable
                else 0o644
            )
            _write_member(wheel, name, content, mode=mode)
        _write_member(wheel, f"{dist_info_prefix}/RECORD", record)
    return wheel_path


def write_platform_base_wheel(
    path: Path,
    *,
    helper_name: str | None = checker.PARAKEET_HELPER_MEMBER,
    helper_binary: bytes | None = None,
    helper_mode: int = 0o755,
    nvattest_authority_bytes: bytes | None = NVATTEST_AUTHORITY_BYTES,
    extra_payload_size: int = 0,
    version: str = "1.2.3",
) -> Path:
    path.mkdir(parents=True, exist_ok=True)
    wheel_path = path / f"solstone-{version}-py3-none-macosx_14_0_arm64.whl"
    if helper_binary is None:
        helper_binary = minimal_macho(checker.CPU_TYPE_ARM64)
    members = {
        f"solstone-{version}.dist-info/METADATA": (
            f"Name: solstone\nVersion: {version}\n".encode()
        ),
        f"solstone-{version}.dist-info/WHEEL": b"Wheel-Version: 1.0\n",
    }
    for launcher_name, content in ROOT_LAUNCHER_BYTES.items():
        members[f"solstone-{version}.data/scripts/{launcher_name}"] = content
    if nvattest_authority_bytes is not None:
        members[checker.NVATTEST_AUTHORITY_MEMBER] = nvattest_authority_bytes
    if helper_name is not None:
        members[helper_name] = helper_binary
    if extra_payload_size:
        members["solstone/observe/transcribe/parakeet_helper/_bin/payload"] = (
            b"x" * extra_payload_size
        )
    rows = [
        f"{name},{record_hash(content)},{len(content)}"
        for name, content in members.items()
    ]
    rows.append(f"solstone-{version}.dist-info/RECORD,,")
    with zipfile.ZipFile(wheel_path, "w") as wheel:
        for name, content in members.items():
            if Path(name).name in checker.ROOT_LAUNCHER_NAMES:
                mode = 0o755
            else:
                mode = helper_mode if name == helper_name else 0o644
            _write_member(wheel, name, content, mode=mode)
        _write_member(
            wheel,
            f"solstone-{version}.dist-info/RECORD",
            "\n".join(rows).encode("utf-8"),
        )
    return wheel_path
