#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Check the solstone/model wheel split after a workspace build."""

from __future__ import annotations

import argparse
import ast
import base64
import hashlib
import re
import struct
import sys
import tarfile
import tomllib
import zipfile
from pathlib import Path
from typing import Literal, NamedTuple, Sequence

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT))

from scripts.stage_speakers_analyze_runtime import (
    NOTICE_INSTALL_DIR as SPEAKERS_ANALYZE_NOTICE_INSTALL_DIR,
)
from scripts.stage_speakers_analyze_runtime import (
    RUNTIME_INSTALL_DIR as SPEAKERS_ANALYZE_RUNTIME_INSTALL_DIR,
)
from scripts.stage_speakers_analyze_runtime import (
    TARGETS as SPEAKERS_ANALYZE_TARGETS,
)
from solstone.think.probe import (
    SOLSTONE_CORE_COVERED_PLATFORMS,
    SOLSTONE_CORE_PLATFORM_TAGS,
    SOLSTONE_CORE_SPEAKERS_ANALYZE_COVERED_PLATFORMS,
    SOLSTONE_CORE_SPEAKERS_ANALYZE_PLATFORM_TAGS,
    CorePlatform,
    current_solstone_core_platform,
    is_solstone_core_covered_platform,
    normalize_solstone_core_machine,
)

EXPECTED_MODEL_SHA256 = {
    "silero_vad_v6.onnx": (
        "4cbf549b8326f60f80f2536d9eefeb450a9abe83365a098031c89719f1be17d2"
    ),
    "pyannote-segmentation-3.0.onnx": (
        "057ee564753071c0b09b5b611648b50ac188d50846bff5f01e9f7bbf1591ea25"
    ),
    "wespeaker-resnet34-256.onnx": (
        "5ef208a9da1453335308a6b6f4e6dfbd7e183a38b604de0a57664f45d257fe94"
    ),
}
MAX_BASE_WHEEL_BYTES = 4 * 1024 * 1024
MAX_BASE_PLATFORM_WHEEL_BYTES = 6 * 1024 * 1024
MAX_CORE_WHEEL_BYTES = 30 * 1024 * 1024
MAX_SPEAKERS_ANALYZE_WHEEL_BYTES = 30 * 1024 * 1024
MAX_DESCRIBE_WHEEL_BYTES = 30 * 1024 * 1024
PARAKEET_HELPER_MEMBER = (
    "solstone/observe/transcribe/parakeet_helper/_bin/parakeet-helper"
)
NVATTEST_AUTHORITY_MEMBER = "solstone/think/providers/nvattest_authority_v1.json"
ROOT_LAUNCHER_NAMES = ("sol", "solstone")
CORE_SCRIPT_NAMES = ("solstone-core",)
SPEAKERS_ANALYZE_SCRIPT_NAMES = ("solstone-core-speakers-analyze",)
DESCRIBE_SCRIPT_NAME = "solstone-core-describe"
ELF_MAGIC = b"\x7fELF"
ELF_CLASS_64 = 2
ELF_DATA_LITTLE_ENDIAN = 1
ELF_MACHINE = {
    "x86_64": 0x003E,
    "aarch64": 0x00B7,
}
PT_DYNAMIC = 2
PT_INTERP = 3
PT_LOAD = 1
DT_NULL = 0
DT_NEEDED = 1
DT_STRTAB = 5
DT_STRSZ = 10
DT_RPATH = 15
DT_RUNPATH = 29
MH_MAGIC_64 = 0xFEEDFACF
FAT_MAGIC = 0xCAFEBABE
FAT_CIGAM = 0xBEBAFECA
FAT_MAGIC_64 = 0xCAFEBABF
FAT_CIGAM_64 = 0xBFBAFECA
FAT_ARCH_SIZE = 20
FAT_ARCH_64_SIZE = 32
CPU_TYPE_ARM64 = 0x0100000C
LC_LOAD_DYLIB = 0x0000000C
LC_RPATH = 0x8000001C
ReleaseScope = Literal["linux", "all-hosts"]
ModelsDecision = Literal["publish", "skip"]
CORE_REQUIRED_SDIST_MEMBERS = {
    "core/Cargo.lock",
    "core/Cargo.toml",
    "core/crates/solstone-core/Cargo.toml",
    "core/crates/solstone-core/src/main.rs",
    "core/crates/solstone-core-cli/Cargo.toml",
    "core/crates/solstone-core-cli/src/lib.rs",
    "core/crates/solstone-core-journal/Cargo.toml",
    "core/crates/solstone-core-journal/src/lib.rs",
    "core/crates/solstone-core-sol/Cargo.toml",
    "core/crates/solstone-core-sol/src/generated/journal_host_commands.rs",
    "core/crates/solstone-core-sol/src/generated/mod.rs",
    "core/crates/solstone-core-sol/src/lib.rs",
    "core/crates/solstone-core-sol/src/skills.rs",
    "core/crates/solstone-core-sol-client/Cargo.toml",
    "core/crates/solstone-core-sol-client/src/aggregate.rs",
    "core/crates/solstone-core-sol-client/src/command.rs",
    "core/crates/solstone-core-sol-client/src/decode.rs",
    "core/crates/solstone-core-sol-client/src/error.rs",
    "core/crates/solstone-core-sol-client/src/generated/inventory.rs",
    "core/crates/solstone-core-sol-client/src/generated/mod.rs",
    "core/crates/solstone-core-sol-client/src/json_format.rs",
    "core/crates/solstone-core-sol-client/src/lib.rs",
    "core/crates/solstone-core-sol-client/src/pagination.rs",
    "core/crates/solstone-core-sol-client/src/port.rs",
    "core/crates/solstone-core-sol-client/src/seam.rs",
    "core/crates/solstone-core-sol-client/src/sse.rs",
    "core/crates/solstone-core-sol-client/src/transport.rs",
    "core/crates/solstone-core-sol-client-cli/Cargo.toml",
    "core/crates/solstone-core-sol-client-cli/src/help.rs",
    "core/crates/solstone-core-sol-client-cli/src/lib.rs",
    "core/fixtures/native-sol/root-contract-v1.json",
    "core/fixtures/native-sol/cli-boundary-v1.json",
    "solstone/apps/activities/native/authority.toml",
    "solstone/apps/activities/native/command.rs",
    "solstone/apps/awareness/native/authority.toml",
    "solstone/apps/awareness/native/command.rs",
    "solstone/apps/body/native/authority.toml",
    "solstone/apps/body/native/command.rs",
    "solstone/apps/chat/native/authority.toml",
    "solstone/apps/chat/native/command.rs",
    "solstone/apps/entities/native/authority.toml",
    "solstone/apps/entities/native/command.rs",
    "solstone/apps/facets/native/authority.toml",
    "solstone/apps/facets/native/command.rs",
    "solstone/apps/import/native/authority.toml",
    "solstone/apps/import/native/command.rs",
    "solstone/apps/network/native/authority.toml",
    "solstone/apps/network/native/command.rs",
    "solstone/apps/settings/native/authority.toml",
    "solstone/apps/settings/native/command.rs",
    "solstone/apps/sol/native/authority.toml",
    "solstone/apps/sol/native/command.rs",
    "solstone/apps/speakers/native/authority.toml",
    "solstone/apps/speakers/native/command.rs",
    "solstone/apps/support/native/authority.toml",
    "solstone/apps/support/native/command.rs",
    "solstone/apps/thinking/native/authority.toml",
    "solstone/apps/thinking/native/command.rs",
    "solstone/apps/transcripts/native/authority.toml",
    "solstone/apps/transcripts/native/command.rs",
    "solstone/think/native/chat/authority.toml",
    "solstone/think/native/chat/command.rs",
    "solstone/think/native/import/authority.toml",
    "solstone/think/native/import/command.rs",
    "solstone/think/native/moved/authority.toml",
    "solstone/think/native/moved/command.rs",
    "solstone/think/tools/native/health/authority.toml",
    "solstone/think/tools/native/health/command.rs",
    "solstone/think/tools/native/ledger/authority.toml",
    "solstone/think/tools/native/ledger/command.rs",
    "solstone/think/tools/native/profile/authority.toml",
    "solstone/think/tools/native/profile/command.rs",
}
CORE_TAG_PLATFORMS = {
    tag: platform for platform, tag in SOLSTONE_CORE_PLATFORM_TAGS.items()
}
SPEAKERS_ANALYZE_TAG_PLATFORMS = {
    tag: platform
    for platform, tag in SOLSTONE_CORE_SPEAKERS_ANALYZE_PLATFORM_TAGS.items()
}
SPEAKERS_ANALYZE_PLATFORM_TARGETS = {
    ("linux", "x86_64"): "linux-x86_64",
    ("linux", "aarch64"): "linux-aarch64",
    ("darwin", "arm64"): "macos-arm64",
}


class SpeakersAnalyzeRuntimeLinkContract(NamedTuple):
    rpath: str
    runtime_load: str


def _speakers_analyze_runtime_load(platform_tuple: CorePlatform) -> str:
    target_key = SPEAKERS_ANALYZE_PLATFORM_TARGETS[platform_tuple]
    staged_name = SPEAKERS_ANALYZE_TARGETS[target_key].runtime_staged_name
    if platform_tuple[0] == "darwin":
        return f"@rpath/{staged_name}"
    return staged_name


SPEAKERS_ANALYZE_RUNTIME_LINK_CONTRACTS: dict[
    CorePlatform, SpeakersAnalyzeRuntimeLinkContract
] = {
    ("linux", "x86_64"): SpeakersAnalyzeRuntimeLinkContract(
        "$ORIGIN/../lib/solstone-core-speakers-analyze",
        _speakers_analyze_runtime_load(("linux", "x86_64")),
    ),
    ("linux", "aarch64"): SpeakersAnalyzeRuntimeLinkContract(
        "$ORIGIN/../lib/solstone-core-speakers-analyze",
        _speakers_analyze_runtime_load(("linux", "aarch64")),
    ),
    ("darwin", "arm64"): SpeakersAnalyzeRuntimeLinkContract(
        "@loader_path/../lib/solstone-core-speakers-analyze",
        _speakers_analyze_runtime_load(("darwin", "arm64")),
    ),
}
SPEAKERS_ANALYZE_FORBIDDEN_PROVIDER_RE = re.compile(
    r"providers_(?:cuda|tensorrt|shared)", re.IGNORECASE
)
FORBIDDEN_PRODUCTION_IMPORT_EXACT = {
    "kaldi_native_fbank",
    "solstone.observe.model_assets",
    "solstone.observe.transcribe.diarize",
    "solstone.observe.transcribe.overlap",
    "solstone.observe.transcribe.speakers_analyze_seam",
    "solstone.think.speakers_analyze_handshake",
    "solstone.think.speakers_analyze_runtime",
}
FORBIDDEN_PRODUCTION_IMPORT_PREFIXES = ("tests.speaker_oracle", "sklearn")


def _is_base_wheel(path: Path) -> bool:
    return bool(re.match(r"solstone-\d", path.name))


def forbidden_production_imports(
    root: Path,
    *,
    forbidden_exact: set[str] = FORBIDDEN_PRODUCTION_IMPORT_EXACT,
    forbidden_prefixes: tuple[str, ...] = FORBIDDEN_PRODUCTION_IMPORT_PREFIXES,
) -> list[str]:
    source_root = root / "solstone" if (root / "solstone").is_dir() else root
    violations: list[str] = []
    for path in sorted(source_root.rglob("*.py")):
        tree = ast.parse(path.read_text(encoding="utf-8"), filename=str(path))
        for node in ast.walk(tree):
            module_names: list[str] = []
            if isinstance(node, ast.Import):
                module_names = [alias.name for alias in node.names]
            elif isinstance(node, ast.ImportFrom) and node.module:
                module_names = [node.module]
            for module_name in module_names:
                if module_name in forbidden_exact or module_name.startswith(
                    forbidden_prefixes
                ):
                    violations.append(f"{path.relative_to(root)} imports {module_name}")
    return violations


def _is_models_wheel(path: Path) -> bool:
    return path.name.startswith("solstone_journal_models-")


def _is_core_wheel(path: Path) -> bool:
    return path.name.startswith("solstone_core-") and path.name.endswith(".whl")


def _is_speakers_analyze_wheel(path: Path) -> bool:
    return path.name.startswith(
        "solstone_core_speakers_analyze-"
    ) and path.name.endswith(".whl")


def _is_describe_wheel(path: Path) -> bool:
    return path.name.startswith("solstone_core_describe-") and path.name.endswith(
        ".whl"
    )


def _is_core_sdist(path: Path) -> bool:
    return path.name.startswith("solstone_core-") and path.name.endswith(".tar.gz")


def _project_version(path: Path) -> str:
    data = tomllib.loads(path.read_text(encoding="utf-8"))
    version = data.get("project", {}).get("version")
    if not isinstance(version, str) or not version:
        raise ValueError(f"{path}: missing [project].version")
    return version


def _release_version() -> str:
    return _project_version(ROOT / "pyproject.toml")


def _models_version() -> str:
    return _project_version(
        ROOT / "packages" / "solstone-journal-models" / "pyproject.toml"
    )


def _core_wheel_tag(path: Path) -> str:
    stem = path.name.removesuffix(".whl")
    return stem.split("-")[-1]


def _wheel_version_from_name(path: Path, distribution: str) -> str:
    stem = path.name.removesuffix(".whl")
    prefix = f"{distribution}-"
    if not stem.startswith(prefix):
        raise ValueError(f"{path.name}: expected {distribution} wheel")
    return stem.removeprefix(prefix).split("-", 1)[0]


def _parse_core_platform(value: str) -> CorePlatform:
    try:
        system, machine = value.split("/", 1)
    except ValueError as exc:
        raise argparse.ArgumentTypeError(
            f"expected platform as system/machine, got {value!r}"
        ) from exc
    platform_tuple: CorePlatform = (
        "darwin" if system == "darwin" else "linux",
        normalize_solstone_core_machine(system, machine),
    )
    if platform_tuple not in SOLSTONE_CORE_PLATFORM_TAGS:
        supported = ", ".join(
            f"{system}/{machine}" for system, machine in SOLSTONE_CORE_PLATFORM_TAGS
        )
        raise argparse.ArgumentTypeError(
            f"unsupported solstone-core platform {value!r}; supported: {supported}"
        )
    return platform_tuple


def _onnx_members(path: Path) -> list[str]:
    with zipfile.ZipFile(path) as wheel:
        return [name for name in wheel.namelist() if name.endswith(".onnx")]


def _failure(
    subject: str,
    error: str,
    *,
    expected: str,
    actual: str,
    repair: str,
) -> str:
    return (
        f"{subject}: {error}; expected: {expected}; actual: {actual}; "
        f"repair command: {repair}"
    )


def _core_rebuild_command(platform_tuple: CorePlatform | None) -> str:
    if platform_tuple is not None and platform_tuple[0] == "linux":
        return "bash scripts/release.sh --dry-run-linux"
    return "bash scripts/release.sh --candidate"


def _speakers_analyze_rebuild_command(platform_tuple: CorePlatform | None) -> str:
    if platform_tuple is None:
        return "bash scripts/release.sh --candidate"
    if platform_tuple[0] == "darwin":
        return "make wheel-macos"
    if platform_tuple[1] == "aarch64":
        return "make wheel-speakers-analyze-linux-aarch64"
    return "make wheel-speakers-analyze-linux-x86_64"


def check_base_wheel(path: Path, max_bytes: int) -> list[str]:
    errors: list[str] = []
    platform_wheel = not path.name.endswith("-any.whl")
    if platform_wheel:
        # platform base wheels bundle the notarized parakeet helper binary
        max_bytes = max(max_bytes, MAX_BASE_PLATFORM_WHEEL_BYTES)
    size = path.stat().st_size
    if size > max_bytes:
        errors.append(f"{path.name}: base wheel is {size} bytes; max is {max_bytes}")
    onnx_members = _onnx_members(path)
    if onnx_members:
        errors.append(f"{path.name}: base wheel contains ONNX members: {onnx_members}")
    with zipfile.ZipFile(path) as wheel:
        for member in wheel.namelist():
            if "tests" in member.split("/"):
                errors.append(f"{path.name}: base wheel ships test path {member}")
        errors.extend(_check_base_root_launchers(path, wheel))
        errors.extend(_check_record(path, wheel))
        if platform_wheel:
            errors.extend(_check_base_platform_helper(path, wheel))
    return errors


def _check_nvattest_authority_member(base_wheels: list[Path]) -> list[str]:
    errors: list[str] = []
    expected_digest: str | None = None
    expected_wheel: str | None = None
    for path in base_wheels:
        with zipfile.ZipFile(path) as wheel:
            if NVATTEST_AUTHORITY_MEMBER not in wheel.namelist():
                errors.append(
                    f"{path.name}: missing nvattest authority member "
                    f"{NVATTEST_AUTHORITY_MEMBER}"
                )
                continue
            digest = hashlib.sha256(wheel.read(NVATTEST_AUTHORITY_MEMBER)).hexdigest()
        if expected_digest is None:
            expected_digest = digest
            expected_wheel = path.name
            continue
        if digest != expected_digest:
            errors.append(
                f"{path.name}: nvattest authority member digest mismatch; "
                f"expected {expected_digest} from {expected_wheel}, actual {digest}"
            )
    return errors


def check_models_wheel(path: Path, expected: dict[str, str]) -> list[str]:
    errors: list[str] = []
    with zipfile.ZipFile(path) as wheel:
        onnx_members = [name for name in wheel.namelist() if name.endswith(".onnx")]
        basenames = [Path(name).name for name in onnx_members]
        expected_names = set(expected)
        found_names = set(basenames)
        if len(onnx_members) != len(expected) or found_names != expected_names:
            errors.append(
                f"{path.name}: expected ONNX basenames {sorted(expected_names)}, "
                f"found {sorted(basenames)}"
            )
        for member, basename in zip(onnx_members, basenames):
            expected_sha256 = expected.get(basename)
            if expected_sha256 is None:
                continue
            actual_sha256 = hashlib.sha256(wheel.read(member)).hexdigest()
            if actual_sha256 != expected_sha256:
                errors.append(
                    f"{path.name}: {basename} sha256 mismatch; "
                    f"expected {expected_sha256}, actual {actual_sha256}"
                )
    return errors


def _record_hash(content: bytes) -> str:
    digest = hashlib.sha256(content).digest()
    encoded = base64.urlsafe_b64encode(digest).decode("ascii").rstrip("=")
    return f"sha256={encoded}"


def _check_record(path: Path, wheel: zipfile.ZipFile) -> list[str]:
    errors: list[str] = []
    names = set(wheel.namelist())
    record_names = [name for name in names if name.endswith(".dist-info/RECORD")]
    if len(record_names) != 1:
        return [f"{path.name}: expected exactly one RECORD, found {len(record_names)}"]

    record_name = record_names[0]
    rows = wheel.read(record_name).decode("utf-8").splitlines()
    seen: set[str] = set()
    for row in rows:
        columns = row.split(",")
        if len(columns) != 3:
            errors.append(f"{path.name}: malformed RECORD row: {row!r}")
            continue
        member, expected_hash, expected_size = columns
        if member not in names:
            errors.append(f"{path.name}: RECORD references missing member {member}")
            continue
        seen.add(member)
        if member == record_name:
            if expected_hash or expected_size:
                errors.append(
                    f"{path.name}: RECORD row for RECORD must have empty hash/size"
                )
            continue
        content = wheel.read(member)
        if expected_hash != _record_hash(content):
            errors.append(f"{path.name}: RECORD hash mismatch for {member}")
        if expected_size != str(len(content)):
            errors.append(f"{path.name}: RECORD size mismatch for {member}")

    missing = sorted(names - seen)
    if missing:
        errors.append(f"{path.name}: members missing from RECORD: {missing}")
    return errors


def _check_record_member(
    path: Path,
    wheel: zipfile.ZipFile,
    member_name: str,
    *,
    repair: str,
) -> list[str]:
    names = set(wheel.namelist())
    record_names = [name for name in names if name.endswith(".dist-info/RECORD")]
    if len(record_names) != 1:
        return [
            _failure(
                path.name,
                "platform base wheel RECORD count is wrong",
                expected="exactly one .dist-info/RECORD",
                actual=str(len(record_names)),
                repair=repair,
            )
        ]
    record_name = record_names[0]
    rows = wheel.read(record_name).decode("utf-8").splitlines()
    for row in rows:
        columns = row.split(",")
        if len(columns) != 3:
            continue
        member, expected_hash, expected_size = columns
        if member != member_name:
            continue
        content = wheel.read(member_name)
        errors: list[str] = []
        if expected_hash != _record_hash(content):
            errors.append(
                _failure(
                    path.name,
                    "parakeet helper RECORD hash mismatch",
                    expected=_record_hash(content),
                    actual=expected_hash,
                    repair=repair,
                )
            )
        if expected_size != str(len(content)):
            errors.append(
                _failure(
                    path.name,
                    "parakeet helper RECORD size mismatch",
                    expected=str(len(content)),
                    actual=expected_size,
                    repair=repair,
                )
            )
        return errors
    return [
        _failure(
            path.name,
            "parakeet helper is missing from RECORD",
            expected=f"RECORD row for {member_name}",
            actual="missing",
            repair=repair,
        )
    ]


def _check_elf_dynamic_entries(
    wheel_name: str,
    content: bytes,
    *,
    program_offset: int,
    program_size: int,
    repair: str,
) -> list[str]:
    errors: list[str] = []
    offset = program_offset
    end = program_offset + program_size
    while offset + 16 <= end and offset + 16 <= len(content):
        tag = struct.unpack_from("<q", content, offset)[0]
        if tag == DT_NULL:
            break
        if tag == DT_NEEDED:
            errors.append(
                _failure(
                    wheel_name,
                    "solstone-core ELF binary has DT_NEEDED",
                    expected="no DT_NEEDED entries",
                    actual="DT_NEEDED present",
                    repair=repair,
                )
            )
            break
        offset += 16
    return errors


def _read_elf_c_string(content: bytes, offset: int) -> str | None:
    if offset < 0 or offset >= len(content):
        return None
    end = content.find(b"\0", offset)
    if end < 0:
        return None
    try:
        return content[offset:end].decode("utf-8")
    except UnicodeDecodeError:
        return None


def _elf_vaddr_to_offset(
    vaddr: int, load_segments: Sequence[tuple[int, int, int]]
) -> int | None:
    for segment_vaddr, segment_offset, segment_size in load_segments:
        if segment_vaddr <= vaddr < segment_vaddr + segment_size:
            return segment_offset + (vaddr - segment_vaddr)
    return None


def _elf_dynamic_strings(content: bytes) -> tuple[list[str], str | None, str | None]:
    if len(content) < 64 or content[:4] != ELF_MAGIC:
        return [], None, None
    phoff = struct.unpack_from("<Q", content, 32)[0]
    phentsize = struct.unpack_from("<H", content, 54)[0]
    phnum = struct.unpack_from("<H", content, 56)[0]
    if phentsize < 56 or phoff + phentsize * phnum > len(content):
        return [], None, None

    load_segments: list[tuple[int, int, int]] = []
    dynamic_offset: int | None = None
    dynamic_size = 0
    for index in range(phnum):
        offset = phoff + phentsize * index
        p_type = struct.unpack_from("<I", content, offset)[0]
        p_offset = struct.unpack_from("<Q", content, offset + 8)[0]
        p_vaddr = struct.unpack_from("<Q", content, offset + 16)[0]
        p_filesz = struct.unpack_from("<Q", content, offset + 32)[0]
        if p_type == PT_LOAD:
            load_segments.append((p_vaddr, p_offset, p_filesz))
        elif p_type == PT_DYNAMIC:
            dynamic_offset = p_offset
            dynamic_size = p_filesz
    if dynamic_offset is None:
        return [], None, None

    strtab_vaddr: int | None = None
    needed_offsets: list[int] = []
    runpath_offset: int | None = None
    rpath_offset: int | None = None
    offset = dynamic_offset
    end = dynamic_offset + dynamic_size
    while offset + 16 <= end and offset + 16 <= len(content):
        tag, value = struct.unpack_from("<qQ", content, offset)
        if tag == DT_NULL:
            break
        if tag == DT_STRTAB:
            strtab_vaddr = value
        elif tag == DT_NEEDED:
            needed_offsets.append(value)
        elif tag == DT_RUNPATH:
            runpath_offset = value
        elif tag == DT_RPATH:
            rpath_offset = value
        offset += 16
    if strtab_vaddr is None:
        return [], None, None

    strtab_offset = _elf_vaddr_to_offset(strtab_vaddr, load_segments)
    if strtab_offset is None:
        return [], None, None
    needed = [
        value
        for value in (
            _read_elf_c_string(content, strtab_offset + needed_offset)
            for needed_offset in needed_offsets
        )
        if value is not None
    ]
    runpath = (
        _read_elf_c_string(content, strtab_offset + runpath_offset)
        if runpath_offset is not None
        else None
    )
    rpath = (
        _read_elf_c_string(content, strtab_offset + rpath_offset)
        if rpath_offset is not None
        else None
    )
    return needed, runpath, rpath


def _has_program_header(content: bytes, header_type: int) -> bool:
    if len(content) < 64 or content[:4] != ELF_MAGIC:
        return False
    phoff = struct.unpack_from("<Q", content, 32)[0]
    phentsize = struct.unpack_from("<H", content, 54)[0]
    phnum = struct.unpack_from("<H", content, 56)[0]
    if phentsize < 56 or phoff + phentsize * phnum > len(content):
        return False
    for index in range(phnum):
        offset = phoff + phentsize * index
        if struct.unpack_from("<I", content, offset)[0] == header_type:
            return True
    return False


def _max_glibc_version(content: bytes) -> tuple[int, ...] | None:
    versions = []
    for match in re.finditer(rb"GLIBC_([0-9]+)\.([0-9]+)(?:\.([0-9]+))?", content):
        versions.append(tuple(int(part) for part in match.groups(default=b"0")))
    return max(versions) if versions else None


def _format_version(version: tuple[int, ...] | None) -> str:
    if version is None:
        return "<none>"
    parts = list(version)
    while len(parts) > 2 and parts[-1] == 0:
        parts.pop()
    return ".".join(str(part) for part in parts)


def _declared_manylinux_floor(tag: str) -> tuple[int, int] | None:
    match = re.fullmatch(r"manylinux_(?P<major>[0-9]+)_(?P<minor>[0-9]+)_.+", tag)
    if match is None:
        return None
    return (int(match.group("major")), int(match.group("minor")))


def _check_elf_binary(
    wheel_name: str,
    content: bytes,
    platform_tuple: CorePlatform,
) -> list[str]:
    errors: list[str] = []
    repair = _core_rebuild_command(platform_tuple)
    machine_name = platform_tuple[1]
    expected_machine = ELF_MACHINE[machine_name]
    if len(content) < 64:
        return [
            _failure(
                wheel_name,
                "solstone-core ELF binary is too short",
                expected="at least 64-byte ELF64 header",
                actual=f"{len(content)} bytes",
                repair=repair,
            )
        ]
    if content[:4] != ELF_MAGIC:
        return [
            _failure(
                wheel_name,
                "solstone-core binary is not ELF",
                expected="ELF magic 7f454c46",
                actual=content[:4].hex(),
                repair=repair,
            )
        ]
    if content[4] != ELF_CLASS_64:
        errors.append(
            _failure(
                wheel_name,
                "solstone-core ELF binary is not ELF64",
                expected=str(ELF_CLASS_64),
                actual=str(content[4]),
                repair=repair,
            )
        )
    if content[5] != ELF_DATA_LITTLE_ENDIAN:
        errors.append(
            _failure(
                wheel_name,
                "solstone-core ELF binary is not little-endian",
                expected=str(ELF_DATA_LITTLE_ENDIAN),
                actual=str(content[5]),
                repair=repair,
            )
        )
    actual_machine = struct.unpack_from("<H", content, 18)[0]
    if actual_machine != expected_machine:
        errors.append(
            _failure(
                wheel_name,
                "solstone-core ELF machine does not match wheel tag",
                expected=f"{machine_name} ({expected_machine:#06x})",
                actual=f"{actual_machine:#06x}",
                repair=repair,
            )
        )
    phoff = struct.unpack_from("<Q", content, 32)[0]
    phentsize = struct.unpack_from("<H", content, 54)[0]
    phnum = struct.unpack_from("<H", content, 56)[0]
    if phentsize < 56:
        return errors + [
            _failure(
                wheel_name,
                "solstone-core ELF program header size is invalid",
                expected="at least 56 bytes",
                actual=str(phentsize),
                repair=repair,
            )
        ]
    if phoff + phentsize * phnum > len(content):
        return errors + [
            _failure(
                wheel_name,
                "solstone-core ELF program headers exceed binary length",
                expected="program headers inside file",
                actual=f"offset {phoff}, size {phentsize}, count {phnum}, file {len(content)}",
                repair=repair,
            )
        ]
    for index in range(phnum):
        offset = phoff + phentsize * index
        p_type = struct.unpack_from("<I", content, offset)[0]
        if p_type == PT_INTERP:
            errors.append(
                _failure(
                    wheel_name,
                    "solstone-core ELF binary has PT_INTERP",
                    expected="no PT_INTERP program header",
                    actual="PT_INTERP present",
                    repair=repair,
                )
            )
        if p_type == PT_DYNAMIC:
            p_offset = struct.unpack_from("<Q", content, offset + 8)[0]
            p_filesz = struct.unpack_from("<Q", content, offset + 32)[0]
            errors.extend(
                _check_elf_dynamic_entries(
                    wheel_name,
                    content,
                    program_offset=p_offset,
                    program_size=p_filesz,
                    repair=repair,
                )
            )
    return errors


def _check_macho_binary(
    wheel_name: str,
    content: bytes,
    platform_tuple: CorePlatform,
    *,
    binary_label: str = "solstone-core",
) -> list[str]:
    repair = _core_rebuild_command(platform_tuple)
    if len(content) < 8:
        return [
            _failure(
                wheel_name,
                f"{binary_label} Mach-O binary is too short",
                expected="at least 8 bytes",
                actual=f"{len(content)} bytes",
                repair=repair,
            )
        ]
    magic_be = struct.unpack_from(">I", content, 0)[0]
    magic_le = struct.unpack_from("<I", content, 0)[0]
    if magic_le == MH_MAGIC_64:
        cputype = struct.unpack_from("<I", content, 4)[0]
        if cputype == CPU_TYPE_ARM64:
            return []
        return [
            _failure(
                wheel_name,
                f"{binary_label} Mach-O cputype does not match wheel tag",
                expected=f"arm64 ({CPU_TYPE_ARM64:#010x})",
                actual=f"{cputype:#010x}",
                repair=repair,
            )
        ]
    if magic_be == MH_MAGIC_64:
        cputype = struct.unpack_from(">I", content, 4)[0]
        if cputype == CPU_TYPE_ARM64:
            return []
        return [
            _failure(
                wheel_name,
                f"{binary_label} Mach-O cputype does not match wheel tag",
                expected=f"arm64 ({CPU_TYPE_ARM64:#010x})",
                actual=f"{cputype:#010x}",
                repair=repair,
            )
        ]
    if magic_be in (FAT_MAGIC, FAT_MAGIC_64, FAT_CIGAM, FAT_CIGAM_64):
        endian = ">" if magic_be in (FAT_MAGIC, FAT_MAGIC_64) else "<"
        arch_size = (
            FAT_ARCH_64_SIZE
            if magic_be in (FAT_MAGIC_64, FAT_CIGAM_64)
            else FAT_ARCH_SIZE
        )
        nfat_arch = struct.unpack_from(f"{endian}I", content, 4)[0]
        header_size = 8 + nfat_arch * arch_size
        if header_size > len(content):
            return [
                _failure(
                    wheel_name,
                    f"{binary_label} fat Mach-O header exceeds binary length",
                    expected="all fat architecture records inside file",
                    actual=f"{nfat_arch} records require {header_size} bytes; file {len(content)}",
                    repair=repair,
                )
            ]
        for index in range(nfat_arch):
            cputype = struct.unpack_from(f"{endian}I", content, 8 + arch_size * index)[
                0
            ]
            if cputype == CPU_TYPE_ARM64:
                return []
        return [
            _failure(
                wheel_name,
                f"{binary_label} fat Mach-O has no arm64 slice",
                expected=f"at least one cputype {CPU_TYPE_ARM64:#010x}",
                actual=f"{nfat_arch} slice(s), none arm64",
                repair=repair,
            )
        ]
    return [
        _failure(
            wheel_name,
            f"{binary_label} binary is not recognized as Mach-O",
            expected="Mach-O 64-bit or fat Mach-O magic",
            actual=content[:4].hex(),
            repair=repair,
        )
    ]


def _macho_thin_endian(content: bytes) -> str | None:
    if len(content) < 4:
        return None
    magic_be = struct.unpack_from(">I", content, 0)[0]
    magic_le = struct.unpack_from("<I", content, 0)[0]
    if magic_le == MH_MAGIC_64:
        return "<"
    if magic_be == MH_MAGIC_64:
        return ">"
    return None


def _macho_arm64_slice(
    wheel_name: str, content: bytes, *, repair: str
) -> tuple[bytes | None, list[str]]:
    if _macho_thin_endian(content) is not None:
        return content, []
    if len(content) < 8:
        return None, [
            _failure(
                wheel_name,
                "speakers analyze Mach-O binary is too short for load commands",
                expected="Mach-O header",
                actual=f"{len(content)} bytes",
                repair=repair,
            )
        ]
    magic_be = struct.unpack_from(">I", content, 0)[0]
    if magic_be not in (FAT_MAGIC, FAT_MAGIC_64, FAT_CIGAM, FAT_CIGAM_64):
        return None, [
            _failure(
                wheel_name,
                "speakers analyze binary is not recognized as Mach-O for load commands",
                expected="Mach-O 64-bit or fat Mach-O magic",
                actual=content[:4].hex(),
                repair=repair,
            )
        ]
    endian = ">" if magic_be in (FAT_MAGIC, FAT_MAGIC_64) else "<"
    arch_size = (
        FAT_ARCH_64_SIZE if magic_be in (FAT_MAGIC_64, FAT_CIGAM_64) else FAT_ARCH_SIZE
    )
    nfat_arch = struct.unpack_from(f"{endian}I", content, 4)[0]
    header_size = 8 + nfat_arch * arch_size
    if header_size > len(content):
        return None, [
            _failure(
                wheel_name,
                "speakers analyze fat Mach-O header exceeds binary length",
                expected="all fat architecture records inside file",
                actual=f"{nfat_arch} records require {header_size} bytes; file {len(content)}",
                repair=repair,
            )
        ]
    for index in range(nfat_arch):
        arch_offset = 8 + arch_size * index
        cputype = struct.unpack_from(f"{endian}I", content, arch_offset)[0]
        if cputype != CPU_TYPE_ARM64:
            continue
        if arch_size == FAT_ARCH_64_SIZE:
            slice_offset = struct.unpack_from(f"{endian}Q", content, arch_offset + 8)[0]
            slice_size = struct.unpack_from(f"{endian}Q", content, arch_offset + 16)[0]
        else:
            slice_offset = struct.unpack_from(f"{endian}I", content, arch_offset + 8)[0]
            slice_size = struct.unpack_from(f"{endian}I", content, arch_offset + 12)[0]
        if slice_offset + slice_size > len(content):
            return None, [
                _failure(
                    wheel_name,
                    "speakers analyze arm64 Mach-O slice exceeds binary length",
                    expected="arm64 slice fully inside fat binary",
                    actual=f"offset {slice_offset}, size {slice_size}, file {len(content)}",
                    repair=repair,
                )
            ]
        return content[slice_offset : slice_offset + slice_size], []
    return None, [
        _failure(
            wheel_name,
            "speakers analyze fat Mach-O has no arm64 slice for load commands",
            expected=f"at least one cputype {CPU_TYPE_ARM64:#010x}",
            actual=f"{nfat_arch} slice(s), none arm64",
            repair=repair,
        )
    ]


def _macho_load_command_string(
    wheel_name: str,
    content: bytes,
    *,
    command_start: int,
    command_size: int,
    string_offset: int,
    command_label: str,
    repair: str,
) -> tuple[str | None, list[str]]:
    if string_offset < 8 or string_offset >= command_size:
        return None, [
            _failure(
                wheel_name,
                f"speakers analyze Mach-O {command_label} string offset is invalid",
                expected="lc_str offset inside load command",
                actual=str(string_offset),
                repair=repair,
            )
        ]
    start = command_start + string_offset
    end = command_start + command_size
    nul = content.find(b"\0", start, end)
    if nul < 0:
        return None, [
            _failure(
                wheel_name,
                f"speakers analyze Mach-O {command_label} string is not terminated",
                expected="NUL-terminated lc_str inside load command",
                actual="missing terminator",
                repair=repair,
            )
        ]
    try:
        return content[start:nul].decode("utf-8"), []
    except UnicodeDecodeError as exc:
        return None, [
            _failure(
                wheel_name,
                f"speakers analyze Mach-O {command_label} string is not UTF-8",
                expected="UTF-8 load-command string",
                actual=str(exc),
                repair=repair,
            )
        ]


def _macho_runtime_load_commands(
    wheel_name: str, content: bytes, *, repair: str
) -> tuple[tuple[str, ...], tuple[str, ...], list[str]]:
    slice_content, failures = _macho_arm64_slice(wheel_name, content, repair=repair)
    if failures or slice_content is None:
        return (), (), failures
    endian = _macho_thin_endian(slice_content)
    if endian is None or len(slice_content) < 32:
        return (
            (),
            (),
            [
                _failure(
                    wheel_name,
                    "speakers analyze Mach-O header is invalid for load commands",
                    expected="64-bit Mach-O header with load-command fields",
                    actual=f"{len(slice_content)} bytes",
                    repair=repair,
                )
            ],
        )
    ncmds = struct.unpack_from(f"{endian}I", slice_content, 16)[0]
    sizeofcmds = struct.unpack_from(f"{endian}I", slice_content, 20)[0]
    commands_start = 32
    commands_end = commands_start + sizeofcmds
    if commands_end > len(slice_content):
        return (
            (),
            (),
            [
                _failure(
                    wheel_name,
                    "speakers analyze Mach-O load commands exceed binary length",
                    expected="load commands inside Mach-O slice",
                    actual=f"sizeofcmds {sizeofcmds}, slice {len(slice_content)}",
                    repair=repair,
                )
            ],
        )
    rpaths: list[str] = []
    dylibs: list[str] = []
    cursor = commands_start
    for _index in range(ncmds):
        if cursor + 8 > commands_end:
            failures.append(
                _failure(
                    wheel_name,
                    "speakers analyze Mach-O load command header is truncated",
                    expected="8-byte load command header",
                    actual=f"offset {cursor}, commands end {commands_end}",
                    repair=repair,
                )
            )
            break
        cmd, cmdsize = struct.unpack_from(f"{endian}II", slice_content, cursor)
        if cmdsize < 8 or cursor + cmdsize > commands_end:
            failures.append(
                _failure(
                    wheel_name,
                    "speakers analyze Mach-O load command size is invalid",
                    expected="cmdsize inside load-command region",
                    actual=f"cmd {cmd:#x}, cmdsize {cmdsize}",
                    repair=repair,
                )
            )
            break
        if cmd == LC_RPATH:
            path_offset = struct.unpack_from(f"{endian}I", slice_content, cursor + 8)[0]
            value, value_failures = _macho_load_command_string(
                wheel_name,
                slice_content,
                command_start=cursor,
                command_size=cmdsize,
                string_offset=path_offset,
                command_label="LC_RPATH",
                repair=repair,
            )
            failures.extend(value_failures)
            if value is not None:
                rpaths.append(value)
        elif cmd == LC_LOAD_DYLIB:
            name_offset = struct.unpack_from(f"{endian}I", slice_content, cursor + 8)[0]
            value, value_failures = _macho_load_command_string(
                wheel_name,
                slice_content,
                command_start=cursor,
                command_size=cmdsize,
                string_offset=name_offset,
                command_label="LC_LOAD_DYLIB",
                repair=repair,
            )
            failures.extend(value_failures)
            if value is not None:
                dylibs.append(value)
        cursor += cmdsize
    if cursor != commands_end:
        failures.append(
            _failure(
                wheel_name,
                "speakers analyze Mach-O load command walk did not consume sizeofcmds",
                expected=f"cursor {commands_end}",
                actual=f"cursor {cursor}",
                repair=repair,
            )
        )
    return tuple(rpaths), tuple(dylibs), failures


def _check_speakers_analyze_macho_runtime_paths(
    wheel_name: str, content: bytes, platform_tuple: CorePlatform
) -> list[str]:
    repair = _speakers_analyze_rebuild_command(platform_tuple)
    contract = SPEAKERS_ANALYZE_RUNTIME_LINK_CONTRACTS[platform_tuple]
    rpaths, dylibs, failures = _macho_runtime_load_commands(
        wheel_name, content, repair=repair
    )
    if failures:
        return failures
    rpath_set = set(rpaths)
    if rpath_set != {contract.rpath}:
        failures.append(
            _failure(
                wheel_name,
                "speakers analyze Mach-O RPATH is wrong",
                expected=contract.rpath,
                actual=", ".join(rpaths) or "<missing>",
                repair=repair,
            )
        )
    runtime_loads = [
        value
        for value in dylibs
        if value == contract.runtime_load or "onnxruntime" in value.lower()
    ]
    if runtime_loads != [contract.runtime_load]:
        failures.append(
            _failure(
                wheel_name,
                "speakers analyze Mach-O ONNX Runtime load command is outside bundled sibling-library contract",
                expected=f"LC_LOAD_DYLIB {contract.runtime_load}",
                actual=", ".join(runtime_loads) or "<missing>",
                repair=repair,
            )
        )
    return failures


def _check_core_binary(
    wheel_name: str,
    content: bytes,
    platform_tuple: CorePlatform,
) -> list[str]:
    if platform_tuple[0] == "linux":
        return _check_elf_binary(wheel_name, content, platform_tuple)
    return _check_macho_binary(wheel_name, content, platform_tuple)


def _check_base_platform_helper(
    path: Path,
    wheel: zipfile.ZipFile,
) -> list[str]:
    repair = "bash scripts/release.sh --candidate"
    helpers = [
        info for info in wheel.infolist() if info.filename == PARAKEET_HELPER_MEMBER
    ]
    if len(helpers) != 1:
        return [
            _failure(
                path.name,
                "wrong parakeet helper member count",
                expected=f"exactly one {PARAKEET_HELPER_MEMBER}",
                actual=str(len(helpers)),
                repair=repair,
            )
        ]
    helper = helpers[0]
    errors: list[str] = []
    if ((helper.external_attr >> 16) & 0o111) == 0:
        errors.append(
            _failure(
                path.name,
                "parakeet helper is not executable",
                expected="executable mode bit set",
                actual=oct((helper.external_attr >> 16) & 0o777),
                repair=repair,
            )
        )
    errors.extend(
        _check_macho_binary(
            path.name,
            wheel.read(helper),
            ("darwin", "arm64"),
            binary_label=f"parakeet helper {PARAKEET_HELPER_MEMBER}",
        )
    )
    errors.extend(
        _check_record_member(path, wheel, PARAKEET_HELPER_MEMBER, repair=repair)
    )
    return errors


def _check_base_root_launchers(
    path: Path,
    wheel: zipfile.ZipFile,
) -> list[str]:
    repair = "uv build --wheel"
    scripts = sorted(
        (info for info in wheel.infolist() if ".data/scripts/" in info.filename),
        key=lambda info: info.filename,
    )
    names = {Path(info.filename).name for info in scripts}
    errors: list[str] = []
    if len(scripts) != len(ROOT_LAUNCHER_NAMES) or names != set(ROOT_LAUNCHER_NAMES):
        errors.append(
            _failure(
                path.name,
                "wrong root launcher script member set",
                expected=", ".join(
                    f".data/scripts/{name}" for name in ROOT_LAUNCHER_NAMES
                ),
                actual=", ".join(info.filename for info in scripts) or "<empty>",
                repair=repair,
            )
        )
        return errors
    for script in scripts:
        script_label = Path(script.filename).name
        mode = (script.external_attr >> 16) & 0o777
        if mode != 0o755:
            errors.append(
                _failure(
                    path.name,
                    f"{script_label} launcher mode is wrong",
                    expected="0o755",
                    actual=oct(mode),
                    repair=repair,
                )
            )
        content = wheel.read(script)
        first_line = content.splitlines()[0] if content.splitlines() else b""
        if first_line != b"#!/bin/sh":
            errors.append(
                _failure(
                    path.name,
                    f"{script_label} launcher first line is wrong",
                    expected="#!/bin/sh",
                    actual=first_line.decode("utf-8", errors="replace"),
                    repair=repair,
                )
            )
        errors.extend(_check_record_member(path, wheel, script.filename, repair=repair))

    for member in wheel.namelist():
        if not member.endswith(".dist-info/entry_points.txt"):
            continue
        entry_points = wheel.read(member).decode("utf-8", errors="replace")
        if "solstone-python-compat" in entry_points:
            errors.append(
                _failure(
                    path.name,
                    "base wheel still exposes compatibility entry point",
                    expected="no solstone-python-compat entry point",
                    actual=member,
                    repair=repair,
                )
            )
    return errors


def core_wheel_script_members(wheel: zipfile.ZipFile) -> list[zipfile.ZipInfo]:
    return sorted(
        (
            info
            for info in wheel.infolist()
            if ".data/scripts/" in info.filename
            and Path(info.filename).name in CORE_SCRIPT_NAMES
        ),
        key=lambda info: info.filename,
    )


def _core_expected_members(path: Path) -> set[str]:
    version = _wheel_version_from_name(path, "solstone_core")
    data_prefix = f"solstone_core-{version}.data"
    dist_info_prefix = f"solstone_core-{version}.dist-info"
    return {
        *(f"{data_prefix}/scripts/{name}" for name in CORE_SCRIPT_NAMES),
        f"{dist_info_prefix}/METADATA",
        f"{dist_info_prefix}/WHEEL",
        f"{dist_info_prefix}/RECORD",
        f"{dist_info_prefix}/sboms/solstone-core.cyclonedx.json",
    }


def check_core_wheel(path: Path, max_bytes: int) -> list[str]:
    errors: list[str] = []
    size = path.stat().st_size
    if size > max_bytes:
        errors.append(f"{path.name}: core wheel is {size} bytes; max is {max_bytes}")

    tag = _core_wheel_tag(path)
    allowed_tags = set(SOLSTONE_CORE_PLATFORM_TAGS.values())
    if tag not in allowed_tags:
        errors.append(f"{path.name}: unsupported solstone-core wheel tag {tag}")
    if "-linux_" in path.name:
        errors.append(f"{path.name}: bare linux tag is not publishable")

    platform_tuple = CORE_TAG_PLATFORMS.get(tag)
    with zipfile.ZipFile(path) as wheel:
        expected_members = _core_expected_members(path)
        names = set(wheel.namelist())
        repair = _core_rebuild_command(platform_tuple)
        if names != expected_members:
            errors.append(
                _failure(
                    path.name,
                    "solstone-core wheel member set is wrong",
                    expected=", ".join(sorted(expected_members)),
                    actual=", ".join(sorted(names)) or "<empty>",
                    repair=repair,
                )
            )
        scripts = core_wheel_script_members(wheel)
        script_names = {Path(info.filename).name for info in scripts}
        if len(scripts) != len(CORE_SCRIPT_NAMES) or script_names != set(
            CORE_SCRIPT_NAMES
        ):
            errors.append(
                _failure(
                    path.name,
                    "wrong solstone-core script member set",
                    expected=", ".join(
                        f".data/scripts/{name}" for name in CORE_SCRIPT_NAMES
                    ),
                    actual=", ".join(info.filename for info in scripts) or "<empty>",
                    repair="bash scripts/release.sh --dry-run-linux",
                )
            )
        else:
            for script in scripts:
                script_label = Path(script.filename).name
                if ((script.external_attr >> 16) & 0o111) == 0:
                    errors.append(
                        _failure(
                            path.name,
                            f"{script_label} script is not executable",
                            expected="executable mode bit set",
                            actual=oct((script.external_attr >> 16) & 0o777),
                            repair="bash scripts/release.sh --dry-run-linux",
                        )
                    )
                    continue
                if platform_tuple is not None:
                    errors.extend(
                        _check_core_binary(
                            f"{path.name}:{script.filename}",
                            wheel.read(script),
                            platform_tuple,
                        )
                    )
        errors.extend(_check_record(path, wheel))

    return errors


def _speakers_analyze_expected_members(
    path: Path, platform_tuple: CorePlatform
) -> tuple[set[str], str, str]:
    version = _wheel_version_from_name(path, "solstone_core_speakers_analyze")
    data_prefix = f"solstone_core_speakers_analyze-{version}.data"
    dist_info_prefix = f"solstone_core_speakers_analyze-{version}.dist-info"
    target_key = SPEAKERS_ANALYZE_PLATFORM_TARGETS[platform_tuple]
    spec = SPEAKERS_ANALYZE_TARGETS[target_key]
    binary_member = f"{data_prefix}/scripts/{SPEAKERS_ANALYZE_SCRIPT_NAMES[0]}"
    library_member = (
        f"{data_prefix}/{SPEAKERS_ANALYZE_RUNTIME_INSTALL_DIR.as_posix()}/"
        f"{spec.runtime_staged_name}"
    )
    notice_members = {
        f"{data_prefix}/{SPEAKERS_ANALYZE_NOTICE_INSTALL_DIR.as_posix()}/"
        f"{notice.staged_name}"
        for notice in spec.notices
    }
    return (
        {
            binary_member,
            library_member,
            *notice_members,
            f"{dist_info_prefix}/METADATA",
            f"{dist_info_prefix}/WHEEL",
            f"{dist_info_prefix}/RECORD",
            f"{dist_info_prefix}/sboms/solstone-core-speakers-analyze.cyclonedx.json",
        },
        binary_member,
        library_member,
    )


def _check_speakers_analyze_elf_binary(
    wheel_name: str,
    content: bytes,
    library_content: bytes,
    platform_tuple: CorePlatform,
    tag: str,
) -> list[str]:
    errors: list[str] = []
    repair = _speakers_analyze_rebuild_command(platform_tuple)
    contract = SPEAKERS_ANALYZE_RUNTIME_LINK_CONTRACTS[platform_tuple]
    machine_name = platform_tuple[1]
    expected_machine = ELF_MACHINE[machine_name]
    if len(content) < 64 or content[:4] != ELF_MAGIC:
        return [
            _failure(
                wheel_name,
                "speakers analyze binary is not ELF64",
                expected="ELF64 helper binary",
                actual=content[:4].hex(),
                repair=repair,
            )
        ]
    actual_machine = struct.unpack_from("<H", content, 18)[0]
    if actual_machine != expected_machine:
        errors.append(
            _failure(
                wheel_name,
                "speakers analyze ELF machine does not match wheel tag",
                expected=f"{machine_name} ({expected_machine:#06x})",
                actual=f"{actual_machine:#06x}",
                repair=repair,
            )
        )
    if not _has_program_header(content, PT_INTERP):
        errors.append(
            _failure(
                wheel_name,
                "speakers analyze ELF binary is missing PT_INTERP",
                expected="PT_INTERP program header present",
                actual="missing",
                repair=repair,
            )
        )
    needed, runpath, rpath = _elf_dynamic_strings(content)
    if not needed:
        errors.append(
            _failure(
                wheel_name,
                "speakers analyze ELF binary is missing DT_NEEDED",
                expected="at least one DT_NEEDED entry",
                actual="<empty>",
                repair=repair,
            )
        )
    if contract.runtime_load not in needed:
        errors.append(
            _failure(
                wheel_name,
                "speakers analyze ELF binary does not need ONNX Runtime",
                expected=f"DT_NEEDED {contract.runtime_load}",
                actual=", ".join(needed) or "<empty>",
                repair=repair,
            )
        )
    if runpath != contract.rpath:
        errors.append(
            _failure(
                wheel_name,
                "speakers analyze ELF RUNPATH is wrong",
                expected=contract.rpath,
                actual=runpath or "<missing>",
                repair=repair,
            )
        )
    if rpath is not None:
        errors.append(
            _failure(
                wheel_name,
                "speakers analyze ELF uses legacy RPATH",
                expected="DT_RUNPATH only",
                actual=rpath,
                repair=repair,
            )
        )

    declared = _declared_manylinux_floor(tag)
    binary_glibc = _max_glibc_version(content)
    library_glibc = _max_glibc_version(library_content)
    measured = max(
        (version for version in (binary_glibc, library_glibc) if version is not None),
        default=None,
    )
    if declared is None:
        errors.append(
            _failure(
                wheel_name,
                "speakers analyze wheel tag does not declare a manylinux floor",
                expected="manylinux_N_M platform tag",
                actual=tag,
                repair=repair,
            )
        )
    elif measured is not None and declared < (measured[0], measured[1]):
        errors.append(
            _failure(
                wheel_name,
                "speakers analyze wheel tag understates GLIBC floor",
                expected=f"declared floor >= measured GLIBC_{_format_version(measured)}",
                actual=f"{tag} declares glibc {declared[0]}.{declared[1]}",
                repair=repair,
            )
        )
    return errors


def check_speakers_analyze_wheel(path: Path) -> list[str]:
    errors: list[str] = []
    tag = _core_wheel_tag(path)
    platform_tuple = SPEAKERS_ANALYZE_TAG_PLATFORMS.get(tag)
    repair = _speakers_analyze_rebuild_command(platform_tuple)
    size = path.stat().st_size
    if size > MAX_SPEAKERS_ANALYZE_WHEEL_BYTES:
        errors.append(
            _failure(
                path.name,
                "speakers analyze wheel is too large",
                expected=f"<= {MAX_SPEAKERS_ANALYZE_WHEEL_BYTES} bytes",
                actual=str(size),
                repair=repair,
            )
        )
    if platform_tuple is None:
        errors.append(
            _failure(
                path.name,
                "unsupported speakers analyze wheel tag",
                expected=", ".join(
                    sorted(SOLSTONE_CORE_SPEAKERS_ANALYZE_PLATFORM_TAGS.values())
                ),
                actual=tag,
                repair=repair,
            )
        )
        return errors

    expected_members, binary_member, library_member = (
        _speakers_analyze_expected_members(path, platform_tuple)
    )
    target_key = SPEAKERS_ANALYZE_PLATFORM_TARGETS[platform_tuple]
    spec = SPEAKERS_ANALYZE_TARGETS[target_key]
    with zipfile.ZipFile(path) as wheel:
        names = set(wheel.namelist())
        if names != expected_members:
            errors.append(
                _failure(
                    path.name,
                    "speakers analyze wheel member set is wrong",
                    expected=", ".join(sorted(expected_members)),
                    actual=", ".join(sorted(names)) or "<empty>",
                    repair=repair,
                )
            )
        provider_members = sorted(
            name
            for name in names
            if SPEAKERS_ANALYZE_FORBIDDEN_PROVIDER_RE.search(Path(name).name)
        )
        if provider_members:
            errors.append(
                _failure(
                    path.name,
                    "speakers analyze wheel contains unproven provider library",
                    expected="no providers_cuda, providers_tensorrt, or providers_shared libraries",
                    actual=", ".join(provider_members),
                    repair="python3 scripts/stage_speakers_analyze_runtime.py",
                )
            )
        binary_infos = [
            info for info in wheel.infolist() if info.filename == binary_member
        ]
        if len(binary_infos) != 1:
            errors.append(
                _failure(
                    path.name,
                    "speakers analyze binary member count is wrong",
                    expected=f"exactly one {binary_member}",
                    actual=str(len(binary_infos)),
                    repair=repair,
                )
            )
        else:
            mode = (binary_infos[0].external_attr >> 16) & 0o777
            if mode & 0o111 == 0:
                errors.append(
                    _failure(
                        path.name,
                        "speakers analyze binary is not executable",
                        expected="executable mode bit set",
                        actual=oct(mode),
                        repair=repair,
                    )
                )

        try:
            library_content = wheel.read(library_member)
        except KeyError:
            library_content = b""
        actual_library_sha = hashlib.sha256(library_content).hexdigest()
        # macOS signs the bundled dylib after staging, which rewrites the bytes;
        # the pre-signing digest is unrecoverable from the signed Mach-O. The
        # upstream ORT pin is therefore carried by
        # record_macos_native_wheel.validate_macos_native_record instead.
        if platform_tuple[0] == "linux" and actual_library_sha != spec.runtime_sha256:
            errors.append(
                _failure(
                    path.name,
                    "speakers analyze ONNX Runtime library digest mismatch",
                    expected=spec.runtime_sha256,
                    actual=actual_library_sha,
                    repair="python3 scripts/stage_speakers_analyze_runtime.py",
                )
            )
        for notice in spec.notices:
            notice_member = (
                f"solstone_core_speakers_analyze-{_wheel_version_from_name(path, 'solstone_core_speakers_analyze')}.data/"
                f"{SPEAKERS_ANALYZE_NOTICE_INSTALL_DIR.as_posix()}/{notice.staged_name}"
            )
            try:
                notice_content = wheel.read(notice_member)
            except KeyError:
                notice_content = b""
            actual_notice_sha = hashlib.sha256(notice_content).hexdigest()
            if actual_notice_sha != notice.sha256:
                errors.append(
                    _failure(
                        path.name,
                        f"speakers analyze notice digest mismatch for {notice.staged_name}",
                        expected=notice.sha256,
                        actual=actual_notice_sha,
                        repair="python3 scripts/stage_speakers_analyze_runtime.py",
                    )
                )
        if binary_infos:
            binary_content = wheel.read(binary_member)
            if platform_tuple[0] == "linux":
                errors.extend(
                    _check_speakers_analyze_elf_binary(
                        f"{path.name}:{binary_member}",
                        binary_content,
                        library_content,
                        platform_tuple,
                        tag,
                    )
                )
            else:
                errors.extend(
                    _check_macho_binary(
                        f"{path.name}:{binary_member}",
                        binary_content,
                        platform_tuple,
                        binary_label="solstone-core-speakers-analyze",
                    )
                )
                errors.extend(
                    _check_speakers_analyze_macho_runtime_paths(
                        f"{path.name}:{binary_member}",
                        binary_content,
                        platform_tuple,
                    )
                )
                errors.extend(
                    _check_macho_binary(
                        f"{path.name}:{library_member}",
                        library_content,
                        platform_tuple,
                        binary_label=spec.runtime_staged_name,
                    )
                )
        errors.extend(_check_record(path, wheel))
    return errors


def check_describe_wheel(path: Path) -> list[str]:
    """Validate the statically linked describe helper wheel."""
    errors: list[str] = []
    repair = "make wheel-describe-linux-x86_64"
    tag = _core_wheel_tag(path)
    size = path.stat().st_size
    if size > MAX_DESCRIBE_WHEEL_BYTES:
        errors.append(
            _failure(
                path.name,
                "describe wheel is too large",
                expected=f"<= {MAX_DESCRIBE_WHEEL_BYTES} bytes",
                actual=str(size),
                repair=repair,
            )
        )

    with zipfile.ZipFile(path) as wheel:
        binary_infos = [
            info
            for info in wheel.infolist()
            if info.filename.endswith(f".data/scripts/{DESCRIBE_SCRIPT_NAME}")
        ]
        if len(binary_infos) != 1:
            errors.append(
                _failure(
                    path.name,
                    "describe binary member count is wrong",
                    expected=(
                        f"exactly one .data/scripts/{DESCRIBE_SCRIPT_NAME} member"
                    ),
                    actual=str(len(binary_infos)),
                    repair=repair,
                )
            )
        else:
            binary_info = binary_infos[0]
            mode = (binary_info.external_attr >> 16) & 0o777
            if mode & 0o111 == 0:
                errors.append(
                    _failure(
                        path.name,
                        "describe binary is not executable",
                        expected="executable mode bit set",
                        actual=oct(mode),
                        repair=repair,
                    )
                )
            binary_content = wheel.read(binary_info)
            if not binary_content.startswith(ELF_MAGIC):
                errors.append(
                    _failure(
                        path.name,
                        "describe binary is not an ELF executable",
                        expected="ELF binary",
                        actual="non-ELF member",
                        repair=repair,
                    )
                )
            else:
                needed, _runpath, _rpath = _elf_dynamic_strings(binary_content)
                dynamic_ffmpeg = sorted(
                    library
                    for library in needed
                    if library.startswith(("libav", "libsw"))
                )
                if dynamic_ffmpeg:
                    errors.append(
                        _failure(
                            path.name,
                            "describe binary dynamically links FFmpeg",
                            expected="no libav* or libsw* DT_NEEDED entries",
                            actual=", ".join(dynamic_ffmpeg),
                            repair=repair,
                        )
                    )
                declared = _declared_manylinux_floor(tag)
                measured = _max_glibc_version(binary_content)
                if declared is None:
                    errors.append(
                        _failure(
                            path.name,
                            "describe wheel tag does not declare a manylinux floor",
                            expected="manylinux_N_M platform tag",
                            actual=tag,
                            repair=repair,
                        )
                    )
                elif measured is not None and declared < (measured[0], measured[1]):
                    errors.append(
                        _failure(
                            path.name,
                            "describe wheel tag understates GLIBC floor",
                            expected=(
                                "declared floor >= measured "
                                f"GLIBC_{_format_version(measured)}"
                            ),
                            actual=f"{tag} declares glibc {declared[0]}.{declared[1]}",
                            repair=repair,
                        )
                    )
        errors.extend(_check_record(path, wheel))
    return errors


def check_core_sdist(path: Path) -> list[str]:
    errors: list[str] = []
    with tarfile.open(path, "r:gz") as archive:
        names = archive.getnames()
    prefixes = {name.split("/", 1)[0] for name in names if "/" in name}
    if len(prefixes) != 1:
        return [
            f"{path.name}: expected one top-level sdist directory, found {sorted(prefixes)}"
        ]
    prefix = next(iter(prefixes))
    normalized = {
        name.removeprefix(prefix + "/")
        for name in names
        if name.startswith(prefix + "/")
    }
    missing = sorted(CORE_REQUIRED_SDIST_MEMBERS - normalized)
    if missing:
        errors.append(
            f"{path.name}: core sdist missing Rust workspace members: {missing}"
        )
    return errors


def _core_platforms_for_scope(scope: ReleaseScope) -> tuple[CorePlatform, ...]:
    if scope == "all-hosts":
        return SOLSTONE_CORE_COVERED_PLATFORMS
    return tuple(
        platform_tuple
        for platform_tuple in SOLSTONE_CORE_COVERED_PLATFORMS
        if platform_tuple[0] == "linux"
    )


def _speakers_analyze_platforms_for_scope(
    scope: ReleaseScope,
) -> tuple[CorePlatform, ...]:
    if scope == "all-hosts":
        return SOLSTONE_CORE_SPEAKERS_ANALYZE_COVERED_PLATFORMS
    return tuple(
        platform_tuple
        for platform_tuple in SOLSTONE_CORE_SPEAKERS_ANALYZE_COVERED_PLATFORMS
        if platform_tuple[0] == "linux"
    )


def _release_artifact_members(
    dist_dir: Path,
    *,
    release_scope: ReleaseScope,
    models_decision: ModelsDecision,
) -> list[tuple[Path, str]]:
    version = _release_version()
    models_version = _models_version()
    artifacts = [
        (dist_dir / f"solstone-{version}.tar.gz", "root sdist"),
        (dist_dir / f"solstone-{version}-py3-none-any.whl", "root any wheel"),
        (dist_dir / f"solstone_core-{version}.tar.gz", "core sdist"),
        (dist_dir / f"solstone_journal-{version}.tar.gz", "journal CPU sdist"),
        (
            dist_dir / f"solstone_journal-{version}-py3-none-any.whl",
            "journal CPU wheel",
        ),
        (dist_dir / f"solstone_journal_cuda-{version}.tar.gz", "journal CUDA sdist"),
        (
            dist_dir / f"solstone_journal_cuda-{version}-py3-none-any.whl",
            "journal CUDA wheel",
        ),
    ]
    for platform_tuple in _core_platforms_for_scope(release_scope):
        tag = SOLSTONE_CORE_PLATFORM_TAGS[platform_tuple]
        artifacts.append(
            (
                dist_dir / f"solstone_core-{version}-py3-none-{tag}.whl",
                f"core wheel for {platform_tuple[0]}/{platform_tuple[1]}",
            )
        )
    for platform_tuple in _speakers_analyze_platforms_for_scope(release_scope):
        tag = SOLSTONE_CORE_SPEAKERS_ANALYZE_PLATFORM_TAGS[platform_tuple]
        artifacts.append(
            (
                dist_dir
                / f"solstone_core_speakers_analyze-{version}-py3-none-{tag}.whl",
                "speakers analyze helper wheel for "
                f"{platform_tuple[0]}/{platform_tuple[1]}",
            )
        )
    if release_scope == "all-hosts":
        for platform_tuple in SOLSTONE_CORE_COVERED_PLATFORMS:
            if platform_tuple[0] != "darwin":
                continue
            tag = SOLSTONE_CORE_PLATFORM_TAGS[platform_tuple]
            artifacts.append(
                (
                    dist_dir / f"solstone-{version}-py3-none-{tag}.whl",
                    f"root platform wheel for {platform_tuple[0]}/{platform_tuple[1]}",
                )
            )
    # The caller owns the explicit models include/exclude decision; this helper
    # only maps that decision to the legacy publish/skip artifact vocabulary.
    if models_decision == "publish":
        artifacts.extend(
            [
                (
                    dist_dir / f"solstone_journal_models-{models_version}.tar.gz",
                    "models sdist",
                ),
                (
                    dist_dir
                    / f"solstone_journal_models-{models_version}-py3-none-any.whl",
                    "models wheel",
                ),
            ]
        )
    return artifacts


def release_artifacts(
    dist_dir: Path,
    *,
    release_scope: ReleaseScope,
    models_decision: ModelsDecision,
) -> list[Path]:
    return [
        path
        for path, _label in _release_artifact_members(
            dist_dir,
            release_scope=release_scope,
            models_decision=models_decision,
        )
    ]


def check_release_artifacts(
    dist_dir: Path,
    *,
    release_scope: ReleaseScope,
    models_decision: ModelsDecision,
) -> list[str]:
    errors: list[str] = []
    repair = (
        "bash scripts/release.sh --dry-run-linux"
        if release_scope == "linux"
        else "bash scripts/release.sh --candidate"
    )
    for path, label in _release_artifact_members(
        dist_dir,
        release_scope=release_scope,
        models_decision=models_decision,
    ):
        if path.exists():
            continue
        errors.append(
            _failure(
                str(dist_dir),
                f"missing release artifact for {label}",
                expected=str(path),
                actual="missing",
                repair=repair,
            )
        )
    return errors


def check_dist(
    dist_dir: Path,
    expected: dict[str, str],
    max_bytes: int,
    *,
    required_core_platforms: tuple[CorePlatform, ...] = (),
    release_scope: ReleaseScope | None = None,
    models_decision: ModelsDecision | None = None,
) -> list[str]:
    errors: list[str] = []
    wheels = sorted(dist_dir.glob("*.whl"))
    base_wheels = [path for path in wheels if _is_base_wheel(path)]
    models_wheels = [path for path in wheels if _is_models_wheel(path)]
    core_wheels = [path for path in wheels if _is_core_wheel(path)]
    speakers_analyze_wheels = [
        path for path in wheels if _is_speakers_analyze_wheel(path)
    ]
    describe_wheels = [path for path in wheels if _is_describe_wheel(path)]
    core_sdists = sorted(
        path for path in dist_dir.glob("*.tar.gz") if _is_core_sdist(path)
    )
    helper_only_dist = (
        (speakers_analyze_wheels or describe_wheels)
        and not base_wheels
        and not models_wheels
        and not core_wheels
        and not core_sdists
        and release_scope is None
    )

    if not helper_only_dist and not base_wheels:
        errors.append(f"{dist_dir}: no solstone base wheel found")
    require_models_wheel = release_scope is None or models_decision == "publish"
    if not helper_only_dist and require_models_wheel and not models_wheels:
        errors.append(f"{dist_dir}: no solstone_journal_models wheel found")
    system, machine = current_solstone_core_platform()
    required_tags: dict[str, str] = {}
    if is_solstone_core_covered_platform(system, machine):
        required_tags[SOLSTONE_CORE_PLATFORM_TAGS[(system, machine)]] = (
            f"{system}/{machine}"
        )
    for platform_tuple in required_core_platforms:
        required_tags[SOLSTONE_CORE_PLATFORM_TAGS[platform_tuple]] = (
            f"{platform_tuple[0]}/{platform_tuple[1]}"
        )
    found_core_tags = {_core_wheel_tag(path) for path in core_wheels}
    if not helper_only_dist:
        for tag, platform_name in sorted(required_tags.items()):
            if tag not in found_core_tags:
                errors.append(
                    f"{dist_dir}: no solstone_core wheel found for "
                    f"{platform_name} ({tag})"
                )
    if not helper_only_dist and not core_sdists:
        errors.append(f"{dist_dir}: no solstone_core sdist found")

    for path in base_wheels:
        errors.extend(check_base_wheel(path, max_bytes))
    errors.extend(_check_nvattest_authority_member(base_wheels))
    for path in models_wheels:
        errors.extend(check_models_wheel(path, expected))
    for path in core_wheels:
        errors.extend(check_core_wheel(path, MAX_CORE_WHEEL_BYTES))
    for path in speakers_analyze_wheels:
        errors.extend(check_speakers_analyze_wheel(path))
    for path in describe_wheels:
        errors.extend(check_describe_wheel(path))
    for path in core_sdists:
        errors.extend(check_core_sdist(path))
    if release_scope is not None:
        if models_decision is None:
            raise ValueError("models_decision is required with release_scope")
        errors.extend(
            check_release_artifacts(
                dist_dir,
                release_scope=release_scope,
                models_decision=models_decision,
            )
        )

    return errors


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--require-core-platform",
        action="append",
        default=[],
        type=_parse_core_platform,
        help="require a solstone-core wheel for system/machine, e.g. darwin/arm64",
    )
    parser.add_argument(
        "--release-scope",
        choices=("linux", "all-hosts"),
        help="require the exact release artifact manifest for this scope",
    )
    parser.add_argument(
        "--models-decision",
        choices=("publish", "skip"),
        help="explicit models artifact decision for solstone-journal-models",
    )
    parser.add_argument(
        "--print-artifacts",
        action="store_true",
        help="print the release artifact list after validation",
    )
    parser.add_argument("dist_dir", type=Path)
    args = parser.parse_args(argv)
    if (args.release_scope is None) != (args.models_decision is None):
        parser.error("--release-scope and --models-decision must be used together")
    if args.print_artifacts and args.release_scope is None:
        parser.error("--print-artifacts requires --release-scope")

    errors = check_dist(
        args.dist_dir,
        EXPECTED_MODEL_SHA256,
        MAX_BASE_WHEEL_BYTES,
        required_core_platforms=tuple(args.require_core_platform),
        release_scope=args.release_scope,
        models_decision=args.models_decision,
    )
    if errors:
        print("ERROR: wheel content check failed", file=sys.stderr)
        for error in errors:
            print(f"  {error}", file=sys.stderr)
        return 1
    if args.print_artifacts:
        for path in release_artifacts(
            args.dist_dir,
            release_scope=args.release_scope,
            models_decision=args.models_decision,
        ):
            print(path)
        return 0
    print("wheel contents ok")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
