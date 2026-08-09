# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Shared release-candidate fixture builders."""

from __future__ import annotations

import gzip
import hashlib
import json
import shutil
import tarfile
import zipfile
from collections.abc import Mapping, Sequence
from datetime import UTC, datetime
from io import BytesIO
from pathlib import Path
from typing import Any, Literal

import scripts.check_rust_release_manifest as checker
import scripts.check_wheel_contents as wheel_checker
import scripts.record_macos_native_wheel as native
import scripts.release_candidate_driver as driver
import scripts.release_nvattest_proof as nvattest_proof
import scripts.release_tool_pins as pins
from scripts.check_wheel_contents import (
    CORE_REQUIRED_SDIST_MEMBERS,
    CORE_SCRIPT_NAMES,
    CPU_TYPE_ARM64,
    ELF_MACHINE,
    EXPECTED_MODEL_SHA256,
    NVATTEST_AUTHORITY_MEMBER,
    SPEAKERS_ANALYZE_SCRIPT_NAMES,
    SPEAKERS_ANALYZE_TARGETS,
)
from scripts.release_advisory_policy import PolicyRun
from scripts.release_build_host import BuildHostResult, SourceBundle
from scripts.release_install_smoke import (
    CORE_SMOKE_STDOUT,
    CURRENT_PROOF_SCHEMA_VERSION,
    INSTALL_SCRIPT_NAMES,
    SCRUBBED_COMMAND_ENV,
    SPEAKERS_ANALYZE_RESPONSE_SCHEMA,
    SPEAKERS_ANALYZE_SCRIPT_NAME,
    CommandResult,
    InstallObservation,
    _expected_speakers_analyze_byte_count,
    _expected_speakers_analyze_payload_path,
    _expected_speakers_analyze_shape,
    _speakers_analyze_statement_ids,
    build_install_proof,
    expected_distribution_entries,
    target_install_paths_from_ledger,
    write_install_proof,
)
from scripts.release_nvattest_proof import SUPPORT_DISTRIBUTION_NAMES
from scripts.release_nvattest_support import read_support_lock_entries
from scripts.release_package_inventory import (
    load_release_package_inventory,
    macos_native_record_name,
    native_role,
    normalized_distribution,
)
from scripts.release_proof_host import TargetProofPaths
from scripts.release_target_policy import TARGET_POLICY
from solstone.think.probe import SOLSTONE_CORE_SPEAKERS_ANALYZE_PLATFORM_TAGS
from solstone.think.providers.nvattest_authority import (
    authority_payload,
    nvattest_target_key,
)
from solstone.think.providers.nvattest_install import SIDECAR_SCHEMA_VERSION
from solstone.think.providers.nvattest_loader import nvattest_library_env
from tests.helpers.release_wheel_fixtures import (
    NVATTEST_AUTHORITY_BYTES,
    ROOT_LAUNCHER_BYTES,
    minimal_elf,
    minimal_macho,
    record_hash,
    speakers_analyze_elf,
    speakers_analyze_macho,
    write_core_wheel,
    write_native_binary_wheel,
    write_platform_base_wheel,
    write_speakers_analyze_wheel,
    write_support_wheel,
)

SOURCE_COMMIT = "a" * 40
# PEP-440-invalid so it cannot collide with a real cut model version.
DRIFT_MODEL_VERSION = "0.0.0.drift"
_CORE_LOCK_CONTENT = "fixture lock\n"
LOCK_SHA = hashlib.sha256(_CORE_LOCK_CONTENT.encode("utf-8")).hexdigest()
_LINUX_X86_CORE = minimal_elf(ELF_MACHINE["x86_64"])
_LINUX_AARCH64_CORE = minimal_elf(ELF_MACHINE["aarch64"])
MACOS_CORE = minimal_macho(CPU_TYPE_ARM64)
MACOS_HELPER = minimal_macho(CPU_TYPE_ARM64)
MACOS_SPEAKERS_ANALYZE = speakers_analyze_macho()
MACOS_ONNXRUNTIME = minimal_macho(CPU_TYPE_ARM64)
SPEAKERS_ANALYZE_RUNTIME_BYTES = b"fixture onnxruntime GLIBC_2.27\n"
SPEAKERS_ANALYZE_LICENSE_BYTES = b"fixture onnxruntime license\n"
SPEAKERS_ANALYZE_THIRD_PARTY_NOTICE_BYTES = b"fixture onnxruntime notices\n"
_ZIP_DATE_TIME = (2026, 7, 20, 12, 0, 0)

TombstoneMutation = Literal[
    "extra-key",
    "malformed-json",
    "missing-key",
    "non-mapping",
    "wrong-status",
    "wrong-version",
]


class _GuardedEnv(dict):
    def get(self, key: str, default: Any = None) -> Any:
        if key == "SOURCE_COMMIT":
            raise AssertionError("driver must not read SOURCE_COMMIT")
        return super().get(key, default)

    def __getitem__(self, key: str) -> Any:
        if key == "SOURCE_COMMIT":
            raise AssertionError("driver must not read SOURCE_COMMIT")
        return super().__getitem__(key)


def repo(tmp_path: Path) -> Path:
    root = tmp_path / "repo"
    (root / "core").mkdir(parents=True)
    (root / "packages" / "solstone-journal-models").mkdir(parents=True)
    (root / "core" / "Cargo.lock").write_text(_CORE_LOCK_CONTENT, encoding="utf-8")
    (root / "pyproject.toml").write_text(
        f'[project]\nversion = "{checker._current_version()}"\n',
        encoding="utf-8",
    )
    (root / "packages" / "solstone-journal-models" / "pyproject.toml").write_text(
        f'[project]\nname = "solstone-journal-models"\n'
        f'version = "{wheel_checker._models_version()}"\n',
        encoding="utf-8",
    )
    _write_fixture_support_lock(root)
    return root


def env() -> _GuardedEnv:
    return _GuardedEnv(
        {
            "EXPECTED_RELEASE_COMMIT": SOURCE_COMMIT,
            "SOURCE_COMMIT": "b" * 40,
            "RELEASE_MODEL_PACKAGES": "exclude",
            "RELEASE_ADVISORY_SOURCE_NAME": "fixture",
            "RELEASE_ADVISORY_DB_URL": "ssh://example.test/db.git",
            "RELEASE_ADVISORY_DB_ROOT": "/advisory-db",
        }
    )


def _policy() -> PolicyRun:
    return PolicyRun(
        advisory_source_id="fixture",
        db_snapshot_basename="advisory-db-fixture00000000",
        db_commit="b" * 40,
        db_archive_sha256="c" * 64,
        advisory_count=1,
        advisory_acquired_at="2026-07-20T11:00:00Z",
        db_commit_timestamp="2026-07-19T12:00:00Z",
        policy_checked_at="2026-07-20T12:00:00Z",
        result="pass",
    )


def _support_version(index: int) -> str:
    return f"0.0.{index}"


def _write_fixture_support_wheels(path: Path) -> tuple[Path, ...]:
    return tuple(
        write_support_wheel(path, name=name, version=_support_version(index))
        for index, name in enumerate(sorted(SUPPORT_DISTRIBUTION_NAMES), start=1)
    )


def _write_fixture_support_lock(root: Path) -> None:
    wheels = _write_fixture_support_wheels(root / "fixture-support-lock")
    by_name = {wheel.name.split("-", 1)[0].replace("_", "-"): wheel for wheel in wheels}
    blocks = ["version = 1\n"]
    for index, name in enumerate(sorted(SUPPORT_DISTRIBUTION_NAMES), start=1):
        wheel = by_name[name]
        sha256, size = driver.file_sha256_size(wheel)
        blocks.extend(
            [
                "[[package]]\n",
                f'name = "{name}"\n',
                f'version = "{_support_version(index)}"\n',
                "wheels = [\n",
                "  { "
                f'url = "wheels/{wheel.name}", '
                f'hash = "sha256:{sha256}", '
                f"size = {size} "
                "},\n",
                "]\n\n",
            ]
        )
    (root / "uv.lock").write_text("".join(blocks), encoding="utf-8")


def _wheel_metadata(name: str) -> tuple[str, str]:
    parts = name.removesuffix(".whl").split("-")
    distribution = parts[0]
    version = parts[1]
    return (
        f"{distribution}-{version}.dist-info/METADATA",
        f"Name: {distribution.replace('_', '-')}\nVersion: {version}\n",
    )


def _write_metadata_wheel(path: Path) -> None:
    metadata_name, metadata = _wheel_metadata(path.name)
    version = path.name.removesuffix(".whl").split("-")[1]
    with zipfile.ZipFile(path, "w") as wheel:
        members = {metadata_name: metadata.encode("utf-8")}
        if path.name.startswith("solstone-"):
            members[f"solstone-{version}.dist-info/WHEEL"] = b"Wheel-Version: 1.0\n"
            for name, content in ROOT_LAUNCHER_BYTES.items():
                members[f"solstone-{version}.data/scripts/{name}"] = content
            members[NVATTEST_AUTHORITY_MEMBER] = NVATTEST_AUTHORITY_BYTES
            record_name = f"solstone-{version}.dist-info/RECORD"
            record = "\n".join(
                f"{name},{record_hash(content)},{len(content)}"
                for name, content in members.items()
            )
            members[record_name] = f"{record}\n{record_name},,".encode("utf-8")
        for name, content in members.items():
            info = zipfile.ZipInfo(name, _ZIP_DATE_TIME)
            info.create_system = 3
            info.external_attr = (
                0o755 << 16
                if Path(name).name in checker.ROOT_LAUNCHER_NAMES
                else 0o644 << 16
            )
            wheel.writestr(info, content)


def _write_linux_core_wheels(dist_dir: Path) -> None:
    content_by_lane = {
        "linux-x86_64-musl": _LINUX_X86_CORE,
        "linux-aarch64-musl": _LINUX_AARCH64_CORE,
    }
    for artifact, (lane, _target) in checker.rust_artifact_targets().items():
        if lane not in content_by_lane:
            continue
        tag = artifact.split("-py3-none-", 1)[1].removesuffix(".whl")
        write_core_wheel(
            dist_dir,
            tag=tag,
            binary=content_by_lane[lane],
            version=checker._current_version(),
        )


def _write_linux_speakers_analyze_wheels(dist_dir: Path) -> None:
    # Derived from probe so these fixtures keep matching what the release code
    # selects; a literal here would drift silently if the measured floor moved.
    for platform_tuple, tag in SOLSTONE_CORE_SPEAKERS_ANALYZE_PLATFORM_TAGS.items():
        system, machine = platform_tuple
        if system != "linux":
            continue
        write_speakers_analyze_wheel(
            dist_dir,
            tag=tag,
            binary=speakers_analyze_elf(ELF_MACHINE[machine]),
            library=SPEAKERS_ANALYZE_RUNTIME_BYTES,
            license_notice=SPEAKERS_ANALYZE_LICENSE_BYTES,
            third_party_notice=SPEAKERS_ANALYZE_THIRD_PARTY_NOTICE_BYTES,
            version=checker._current_version(),
        )


def _write_core_sdist(path: Path) -> None:
    version = checker._current_version()
    with path.open("wb") as raw:
        with gzip.GzipFile(filename="", mode="wb", fileobj=raw, mtime=0) as gzipped:
            with tarfile.open(fileobj=gzipped, mode="w") as archive:
                for member in sorted(CORE_REQUIRED_SDIST_MEMBERS):
                    content = b"x"
                    info = tarfile.TarInfo(f"solstone_core-{version}/{member}")
                    info.size = len(content)
                    info.mtime = 0
                    info.mode = 0o644
                    archive.addfile(info, BytesIO(content))


def _write_models_wheel(path: Path) -> None:
    assets_dir = (
        Path(__file__).resolve().parents[2]
        / "packages"
        / "solstone-journal-models"
        / "solstone_journal_models"
        / "assets"
    )
    with zipfile.ZipFile(path, "w") as wheel:
        metadata_name, metadata = _wheel_metadata(path.name)
        metadata_info = zipfile.ZipInfo(metadata_name, _ZIP_DATE_TIME)
        metadata_info.create_system = 3
        metadata_info.external_attr = 0o644 << 16
        wheel.writestr(metadata_info, metadata)
        for basename in sorted(EXPECTED_MODEL_SHA256):
            asset_info = zipfile.ZipInfo(
                f"solstone_journal_models/assets/{basename}", _ZIP_DATE_TIME
            )
            asset_info.create_system = 3
            asset_info.external_attr = 0o644 << 16
            wheel.writestr(asset_info, (assets_dir / basename).read_bytes())


def macos_wheel_names() -> tuple[str, str, str]:
    names = checker.expected_package_names(include_models=False)
    root = next(
        name
        for name in names
        if name.startswith("solstone-") and "macosx_14_0_arm64" in name
    )
    core = next(
        name
        for name in names
        if name.startswith("solstone_core-") and "macosx_14_0_arm64" in name
    )
    speakers_analyze = next(
        name
        for name in names
        if name.startswith("solstone_core_speakers_analyze-")
        and "macosx_14_0_arm64" in name
    )
    return root, core, speakers_analyze


def _facts(content: bytes) -> dict[str, Any]:
    digest = hashlib.sha256(content).hexdigest()
    return {
        "signed_binary_sha256": digest,
        "unsigned_binary_sha256": digest,
        "signer_pinned": True,
        "team_pinned": True,
        "hardened_runtime": True,
        "trusted_timestamp": True,
        "notarization_status": "accepted",
        "tools": {
            "xcode": pins.MACOS_XCODE_PIN,
            "swift": pins.MACOS_SWIFT_FIXTURE_BANNER,
            "codesign": pins.MACOS_CODESIGN_PUBLIC_PIN,
            "notarytool": pins.MACOS_NOTARYTOOL_PIN,
        },
    }


def _core_facts(content: bytes) -> dict[str, Any]:
    return {"members": {name: _facts(content) for name in CORE_SCRIPT_NAMES}}


def _speakers_analyze_facts(script: bytes, dylib: bytes) -> dict[str, Any]:
    return {
        "members": {
            SPEAKERS_ANALYZE_SCRIPT_NAMES[0]: _facts(script),
            SPEAKERS_ANALYZE_TARGETS["macos-arm64"].runtime_staged_name: _facts(dylib),
        }
    }


def write_macos_host_outputs(
    output_dir: Path,
    *,
    mutate: str | None = None,
) -> BuildHostResult:
    output_dir.mkdir(parents=True, exist_ok=True)
    root_name, core_name, speakers_analyze_name = macos_wheel_names()
    root_wheel = output_dir / root_name
    core_wheel = output_dir / core_name
    speakers_analyze_wheel = output_dir / speakers_analyze_name
    if mutate == "wrong_tag":
        root_wheel = output_dir / root_name.replace(
            "macosx_14_0_arm64", "manylinux2014_x86_64"
        )
    root_bytes = MACOS_HELPER
    core_bytes = MACOS_CORE
    speakers_bytes = MACOS_SPEAKERS_ANALYZE
    onnxruntime_bytes = MACOS_ONNXRUNTIME
    write_platform_base_wheel(
        root_wheel.parent,
        helper_binary=root_bytes,
        version=checker._current_version(),
    )
    if root_wheel.name != root_name:
        (output_dir / root_name).rename(root_wheel)
    write_core_wheel(
        core_wheel.parent,
        tag="macosx_14_0_arm64",
        binary=core_bytes,
        version=checker._current_version(),
    )
    write_speakers_analyze_wheel(
        speakers_analyze_wheel.parent,
        tag="macosx_14_0_arm64",
        binary=speakers_bytes,
        library=onnxruntime_bytes,
        license_notice=SPEAKERS_ANALYZE_LICENSE_BYTES,
        third_party_notice=SPEAKERS_ANALYZE_THIRD_PARTY_NOTICE_BYTES,
        version=checker._current_version(),
    )
    root_record = native.build_macos_native_record(
        role="root",
        wheel_path=root_wheel,
        signing_facts=_facts(root_bytes),
        source_commit=SOURCE_COMMIT,
        core_lock_sha256=LOCK_SHA,
    )
    core_record = native.build_macos_native_record(
        role="core",
        wheel_path=core_wheel,
        signing_facts=_core_facts(core_bytes),
        source_commit=SOURCE_COMMIT,
        core_lock_sha256=LOCK_SHA,
    )
    speakers_record = native.build_macos_native_record(
        role="speakers-analyze",
        wheel_path=speakers_analyze_wheel,
        signing_facts=_speakers_analyze_facts(speakers_bytes, onnxruntime_bytes),
        source_commit=SOURCE_COMMIT,
        core_lock_sha256=LOCK_SHA,
    )
    macos_wheels = [root_wheel, core_wheel, speakers_analyze_wheel]
    records_by_role: dict[str, dict[str, Any]] = {
        "root": root_record,
        "core": core_record,
        "speakers-analyze": speakers_record,
    }
    for package in load_release_package_inventory().macos_native_packages:
        role = native_role(package)
        if role in records_by_role:
            continue
        wheel = write_native_binary_wheel(
            output_dir,
            package=package,
            tag="macosx_14_0_arm64",
            binary=MACOS_CORE,
        )
        macos_wheels.append(wheel)
        records_by_role[role] = native.build_macos_native_record(
            role=role,
            wheel_path=wheel,
            signing_facts={"members": {package.binary: _facts(MACOS_CORE)}},
            source_commit=SOURCE_COMMIT,
            core_lock_sha256=LOCK_SHA,
        )
    if mutate == "record_role":
        root_record["role"] = "core"
    if mutate == "member":
        core_record["member"]["sha256"] = "0" * 64
    if mutate == "tool":
        core_record["tools"]["swift"] = "Apple Swift 6.3.3"
    if mutate == "signing":
        root_record["signing"]["team_pinned"] = False
    if mutate == "notary":
        root_record["notarization_status"] = "rejected"
    if mutate == "wheel_hash":
        root_record["wheel"]["sha256"] = "0" * 64
    root_record_path = output_dir / "macos-native-root.json"
    core_record_path = output_dir / "macos-native-core.json"
    if mutate == "record_paths_swapped":
        root_record_path, core_record_path = core_record_path, root_record_path
    record_paths: list[Path] = []
    for role, record in records_by_role.items():
        path = output_dir / macos_native_record_name(role)
        if role == "root":
            path = root_record_path
        elif role == "core":
            path = core_record_path
        path.write_text(json.dumps(record, sort_keys=True), encoding="utf-8")
        record_paths.append(path)
    return BuildHostResult(
        macos_wheels=tuple(macos_wheels),
        native_records=tuple(record_paths),
        tool_evidence=pins.fixture_presign_lane_tool_evidence("macos-arm64"),
    )


def _proof_observation(
    target: str,
    *,
    env_root: Path,
    candidate_dir: Path,
    install_paths: tuple[Path, ...],
    version: str,
) -> InstallObservation:
    (env_root / "bin").mkdir(parents=True, exist_ok=True)
    (env_root / "bin" / "python").write_bytes(b"python")
    for name, content in ROOT_LAUNCHER_BYTES.items():
        (env_root / "bin" / name).write_bytes(content)
    for name in CORE_SCRIPT_NAMES:
        (env_root / "bin" / name).write_bytes(b"core")
    core_sha = {
        "linux-x86_64-musl": hashlib.sha256(_LINUX_X86_CORE).hexdigest(),
        "linux-aarch64-musl": hashlib.sha256(_LINUX_AARCH64_CORE).hexdigest(),
        "macos-arm64": hashlib.sha256(MACOS_CORE).hexdigest(),
    }[target]
    members = [
        {
            "name": name,
            "path": env_root / "bin" / name,
            "sha256": hashlib.sha256(ROOT_LAUNCHER_BYTES[name]).hexdigest(),
            "symlink": False,
        }
        for name in checker.ROOT_LAUNCHER_NAMES
    ]
    members += [
        {
            "name": name,
            "path": env_root / "bin" / name,
            "sha256": core_sha,
            "symlink": False,
        }
        for name in CORE_SCRIPT_NAMES
    ]
    if target == "macos-arm64":
        (env_root / "bin" / "parakeet-helper").write_bytes(b"helper")
        members.append(
            {
                "name": "parakeet-helper",
                "path": env_root / "bin" / "parakeet-helper",
                "sha256": hashlib.sha256(MACOS_HELPER).hexdigest(),
                "symlink": False,
            }
        )
    helper_wheels = [
        path
        for path in install_paths
        if path.name.startswith("solstone_core_speakers_analyze-")
    ]
    if helper_wheels:
        with zipfile.ZipFile(helper_wheels[0]) as wheel:
            helper_member = next(
                info
                for info in wheel.infolist()
                if info.filename.endswith(
                    ".data/scripts/solstone-core-speakers-analyze"
                )
            )
            helper_bytes = wheel.read(helper_member)
        helper_path = env_root / "bin" / "solstone-core-speakers-analyze"
        helper_path.write_bytes(helper_bytes)
        members.append(
            {
                "name": "solstone-core-speakers-analyze",
                "path": helper_path,
                "sha256": hashlib.sha256(helper_bytes).hexdigest(),
                "symlink": False,
            }
        )
    smoke_results = {
        name: CommandResult(
            argv=(str(env_root / "bin" / name), "--version"),
            exit_code=0,
            stdout=f"{CORE_SMOKE_STDOUT[name]} {version}",
            env=SCRUBBED_COMMAND_ENV,
        )
        for name in INSTALL_SCRIPT_NAMES
    }
    if helper_wheels:
        payload_path = env_root / "speakers-analyze-smoke" / "statement-embedding.f32le"
        payload_path.parent.mkdir(parents=True, exist_ok=True)
        payload_path.write_bytes(b"\0" * _expected_speakers_analyze_byte_count())
        smoke_results[SPEAKERS_ANALYZE_SCRIPT_NAME] = CommandResult(
            argv=(str(env_root / "bin" / SPEAKERS_ANALYZE_SCRIPT_NAME),),
            exit_code=0,
            stdout=json.dumps(
                {
                    "schema": SPEAKERS_ANALYZE_RESPONSE_SCHEMA,
                    "inputs": {
                        "statement_embedding": {
                            "statement_ids": _speakers_analyze_statement_ids()
                        }
                    },
                    "statement_embeddings": {
                        "statement_ids": _speakers_analyze_statement_ids(),
                        "shape": _expected_speakers_analyze_shape(),
                        "byte_count": _expected_speakers_analyze_byte_count(),
                        "dtype": "float32-le",
                        "payload_format": "raw-f32le-row-major-v1",
                        "payload_path": _expected_speakers_analyze_payload_path(
                            env_root
                        ),
                    },
                },
                separators=(",", ":"),
            ),
            env=SCRUBBED_COMMAND_ENV,
        )
    return InstallObservation(
        env_root=env_root,
        preexisting_distributions=(),
        install=CommandResult(
            argv=(
                str(env_root / "bin" / "python"),
                "-m",
                "pip",
                "install",
                "--no-index",
                "--no-deps",
                *(str(path) for path in install_paths),
            ),
            exit_code=0,
            stdout="installed",
            env=SCRUBBED_COMMAND_ENV,
        ),
        installed_distributions=expected_distribution_entries(install_paths),
        installed_members=tuple(members),
        smoke=smoke_results,
    )


def _nvattest_target_key(target: str) -> str:
    policy_os, policy_arch = TARGET_POLICY[target]
    target_key = nvattest_target_key(policy_os.lower(), policy_arch)
    assert target_key is not None
    return target_key


def _nvattest_authority_target(target: str) -> Mapping[str, Any]:
    target_key = _nvattest_target_key(target)
    targets = authority_payload()["targets"]
    return targets[target_key]


def _nvattest_fixture_path(name: str) -> Path:
    return Path(__file__).resolve().parents[1] / "fixtures" / "nvattest" / name


def _nvattest_member_facts(
    authority_target: Mapping[str, Any],
) -> list[dict[str, Any]]:
    return [
        {
            "content_sha256": (
                hashlib.sha256(str(member["relpath"]).encode("utf-8")).hexdigest()
                if member["kind"] == "regular"
                else None
            ),
            "executable": member["executable"],
            "kind": member["kind"],
            "relpath": member["relpath"],
            "symlink_target": member["symlink_target"],
        }
        for member in sorted(
            authority_target["inventory"],
            key=lambda item: item["relpath"],
        )
    ]


def _nvattest_driver_payload(
    *,
    target: str,
    env_root: Path,
    journal_path: Path,
    canonical_authority_bytes: bytes,
) -> dict[str, Any]:
    target_key = _nvattest_target_key(target)
    authority_target = _nvattest_authority_target(target)
    site_root = env_root / "lib" / "python3.13" / "site-packages"
    authority_path = (
        site_root / "solstone" / "think" / "providers" / "nvattest_authority_v1.json"
    )
    cache_root = journal_path / "cache" / "providers" / "nvattest"
    sidecar_path = cache_root / ".nvattest-install.json"
    fingerprint = hashlib.sha256(target_key.encode("utf-8")).hexdigest()
    sidecar = {
        "artifact": dict(authority_target["artifact"]),
        "schema_version": SIDECAR_SCHEMA_VERSION,
        "target_key": target_key,
        "tree_fingerprint_sha256": fingerprint,
        "version": authority_target["source"]["version"],
    }
    sidecar_bytes = checker.canonical_json_bytes(sidecar)
    return {
        "authority_module_file": str(
            site_root / "solstone" / "think" / "providers" / "nvattest_authority.py"
        ),
        "authority_origin": str(
            site_root / "solstone" / "think" / "providers" / "nvattest_authority.py"
        ),
        "authority_path": str(authority_path),
        "authority_sha256": hashlib.sha256(canonical_authority_bytes).hexdigest(),
        "authority_size_bytes": len(canonical_authority_bytes),
        "cache_root": str(cache_root),
        "dist_info": [
            {
                "dist_info_path": str(
                    site_root / f"solstone-{checker._current_version()}.dist-info"
                ),
                "name": "solstone",
                "version": checker._current_version(),
            }
        ],
        "journal_path": str(journal_path),
        "members": _nvattest_member_facts(authority_target),
        "module_file": str(
            site_root / "solstone" / "think" / "providers" / "nvattest_install.py"
        ),
        "module_origin": str(
            site_root / "solstone" / "think" / "providers" / "nvattest_install.py"
        ),
        "sidecar": sidecar,
        "sidecar_path": str(sidecar_path),
        "sidecar_sha256": hashlib.sha256(sidecar_bytes).hexdigest(),
        "sidecar_size_bytes": len(sidecar_bytes),
        "site_packages": [str(site_root)],
        "solstone_journal_present": False,
        "spp_nvattest_dir_present": False,
        "tree_fingerprint_sha256": fingerprint,
    }


def _nvattest_command_result(
    argv: Sequence[str],
    *,
    stdout: str = "",
    exit_code: int = 0,
    env: Mapping[str, str] = SCRUBBED_COMMAND_ENV,
) -> CommandResult:
    return CommandResult(
        argv=tuple(argv),
        exit_code=exit_code,
        stdout=stdout,
        stderr="",
        env=env,
    )


def _nvattest_services(
    *,
    root: Path,
    target: str,
    candidate_paths: Sequence[Path],
    support_paths: Sequence[Path],
    canonical_authority_bytes: bytes,
    call_counts: dict[str, int] | None = None,
) -> nvattest_proof.NvattestProofServices:
    env_root = root / "nvattest-env" / target
    policy_os, policy_arch = TARGET_POLICY[target]
    authority_target = _nvattest_authority_target(target)
    expected_candidate_wheels = nvattest_proof.candidate_wheel_entries(candidate_paths)
    expected_support_distributions = (
        nvattest_proof.support_distribution_entries_with_metadata(support_paths)
    )

    def count(name: str) -> None:
        if call_counts is not None:
            call_counts[name] = call_counts.get(name, 0) + 1

    def create_environment(_target: str) -> Path:
        count("nvattest_create_environment")
        (env_root / "bin").mkdir(parents=True, exist_ok=True)
        python = env_root / "bin" / "python"
        python.write_text("#!/bin/sh\nexit 0\n", encoding="utf-8")
        python.chmod(0o755)
        return env_root

    def install_wheels(
        env_python: Path,
        candidate_wheels: Sequence[Path],
        support_wheels: Sequence[Path],
    ) -> CommandResult:
        count("nvattest_install_wheels")
        assert tuple(candidate_wheels) == tuple(candidate_paths)
        assert tuple(support_wheels) == tuple(support_paths)
        site_root = env_root / "lib" / "python3.13" / "site-packages"
        authority_path = (
            site_root
            / "solstone"
            / "think"
            / "providers"
            / "nvattest_authority_v1.json"
        )
        authority_path.parent.mkdir(parents=True, exist_ok=True)
        authority_path.write_bytes(canonical_authority_bytes)
        (site_root / f"solstone-{checker._current_version()}.dist-info").mkdir(
            parents=True, exist_ok=True
        )
        return _nvattest_command_result(
            (
                str(env_python),
                "-m",
                "pip",
                "install",
                "--no-index",
                "--no-deps",
                *(str(path) for path in candidate_wheels),
                *(str(path) for path in support_wheels),
            ),
            stdout="installed",
        )

    def fetch(label: str, url: str, dest: Path) -> nvattest_proof.FetchObservation:
        count("nvattest_fetch")
        dest.parent.mkdir(parents=True, exist_ok=True)
        if label == "archive":
            artifact = authority_target["artifact"]
            dest.write_bytes(b"fixture archive bytes are not retained\n")
            return nvattest_proof.FetchObservation(
                label=label,
                url=url,
                path=dest,
                sha256=str(artifact["sha256"]),
                size_bytes=int(artifact["size_bytes"]),
            )
        source = _nvattest_fixture_path(
            str(authority_target["companion_manifest"]["name"])
        )
        shutil.copy2(source, dest)
        sha256, size_bytes = driver.file_sha256_size(dest)
        return nvattest_proof.FetchObservation(
            label=label, url=url, path=dest, sha256=sha256, size_bytes=size_bytes
        )

    def run_package_install(
        env_python: Path,
        driver_path: Path,
        target_key: str,
        journal_path: Path,
    ) -> nvattest_proof.DriverObservation:
        count("nvattest_run_package_install")
        assert target_key == _nvattest_target_key(target)
        payload = _nvattest_driver_payload(
            target=target,
            env_root=env_root,
            journal_path=journal_path,
            canonical_authority_bytes=canonical_authority_bytes,
        )
        return nvattest_proof.DriverObservation(
            command=_nvattest_command_result(
                (
                    str(env_python),
                    str(driver_path),
                    "--target-key",
                    target_key,
                    "--journal-path",
                    str(journal_path),
                ),
                stdout=json.dumps(payload, sort_keys=True),
            ),
            payload=payload,
        )

    def run_smoke(nvattest_root: Path, nvattest_bin: Path) -> CommandResult:
        count("nvattest_run_smoke")
        return _nvattest_command_result(
            (str(nvattest_bin), "--help"),
            stdout="usage\n",
            env={**SCRUBBED_COMMAND_ENV, **nvattest_library_env(nvattest_root)},
        )

    def integrity_recheck(
        _journal: Path,
        _target_key: str,
        _fetches: Mapping[str, nvattest_proof.FetchObservation],
        driver_observation: nvattest_proof.DriverObservation,
    ) -> dict[str, Any]:
        count("nvattest_integrity_recheck")
        return {
            "members": driver_observation.payload["members"],
            "sidecar": driver_observation.payload["sidecar"],
            "sidecar_path": driver_observation.payload["sidecar_path"],
            "sidecar_sha256": driver_observation.payload["sidecar_sha256"],
            "sidecar_size_bytes": driver_observation.payload["sidecar_size_bytes"],
            "tree_fingerprint_sha256": driver_observation.payload[
                "tree_fingerprint_sha256"
            ],
        }

    def observe_installed_distributions(
        _env_python: Path,
    ) -> Sequence[Mapping[str, Any]]:
        count("nvattest_observe_installed_distributions")
        return [
            {
                "metadata_sha256": entry["metadata_sha256"],
                "name": entry["name"],
                "version": entry["version"],
            }
            for entry in (
                *expected_candidate_wheels,
                *expected_support_distributions,
            )
        ]

    def clock() -> datetime:
        count("nvattest_clock")
        return datetime(2026, 7, 20, 12, 35, tzinfo=UTC)

    def cleanup(path: Path) -> None:
        count("nvattest_cleanup")
        shutil.rmtree(path)

    def observe_host() -> nvattest_proof.HostObservation:
        count("nvattest_observe_host")
        return nvattest_proof.HostObservation(os=policy_os, arch=policy_arch)

    return nvattest_proof.NvattestProofServices(
        create_environment=create_environment,
        install_wheels=install_wheels,
        fetch=fetch,
        run_package_install=run_package_install,
        observe_installed_distributions=observe_installed_distributions,
        integrity_recheck=integrity_recheck,
        run_smoke=run_smoke,
        clock=clock,
        cleanup=cleanup,
        observe_host=observe_host,
    )


def services(
    root: Path, *, native_mutation: str | None = None
) -> driver.CandidateServices:
    call_counts: dict[str, int] = {}

    def count(name: str) -> None:
        call_counts[name] = call_counts.get(name, 0) + 1

    def clean_outputs(repo_root: Path, version: str) -> None:
        count("clean_outputs")
        for relative in (
            "build",
            "dist",
            f"target/release-evidence/{version}",
            f"target/release-transfer/{version}",
            f"target/release-transfer/.{version}.source.bundle",
        ):
            path = repo_root / relative
            if path.is_dir():
                shutil.rmtree(path)
            elif path.exists() or path.is_symlink():
                path.unlink()

    def build_local_dist(repo_root: Path, include_models: bool) -> None:
        count("build_local_dist")
        dist = repo_root / "dist"
        dist.mkdir(parents=True, exist_ok=True)
        for name in driver._expected_local_dist_names(include_models=include_models):
            path = dist / name
            native_package = next(
                (
                    package
                    for package in load_release_package_inventory().native_packages
                    if name.startswith(
                        f"{normalized_distribution(package.distribution)}-"
                    )
                ),
                None,
            )
            if name.startswith("solstone_journal_models-") and name.endswith(".whl"):
                _write_models_wheel(path)
            elif name.startswith("solstone_core_speakers_analyze-") and name.endswith(
                ".whl"
            ):
                continue
            elif (
                native_package is not None
                and native_package.distribution != "solstone-core"
                and name.endswith(".whl")
            ):
                write_native_binary_wheel(
                    dist,
                    package=native_package,
                    tag=name.removesuffix(".whl").split("-")[-1],
                )
            elif name.endswith(".whl"):
                _write_metadata_wheel(path)
            elif name.startswith("solstone_core-") and name.endswith(".tar.gz"):
                _write_core_sdist(path)
            else:
                path.write_bytes(b"fixture package")
        _write_linux_core_wheels(repo_root / "dist")
        _write_linux_speakers_analyze_wheels(repo_root / "dist")

    def create_source_bundle(
        _repo: Path, commit: str, output_path: Path
    ) -> SourceBundle:
        count("create_source_bundle")
        assert commit == SOURCE_COMMIT
        output_path.parent.mkdir(parents=True, exist_ok=True)
        output_path.write_bytes(b"bundle")
        return SourceBundle(
            path=output_path,
            source_commit=SOURCE_COMMIT,
            sha256=hashlib.sha256(b"bundle").hexdigest(),
            bytes=len(b"bundle"),
        )

    def build_host(
        source_bundle: SourceBundle, commit: str, output_dir: Path
    ) -> BuildHostResult:
        count("build_host")
        assert source_bundle.path.read_bytes() == b"bundle"
        assert source_bundle.source_commit == SOURCE_COMMIT
        assert source_bundle.sha256 == hashlib.sha256(b"bundle").hexdigest()
        assert source_bundle.bytes == len(b"bundle")
        assert commit == SOURCE_COMMIT
        return write_macos_host_outputs(output_dir, mutate=native_mutation)

    def materialize_support_wheels(destination: Path) -> tuple[Path, ...]:
        count("materialize_support_wheels")
        entries = read_support_lock_entries(root / "uv.lock")
        return tuple(
            write_support_wheel(destination, name=entry.name, version=entry.version)
            for entry in entries
        )

    def run_target_proofs(**kwargs: Any) -> TargetProofPaths:
        count("run_target_proofs")
        output_path = Path(kwargs["output_path"])
        target = str(kwargs["target"])
        install_paths = target_install_paths_from_ledger(
            kwargs["ledger_payload"],
            target=target,
            candidate_dir=Path(kwargs["candidate_dir"]),
            schema_version=CURRENT_PROOF_SCHEMA_VERSION,
        )
        install_excluded_keys = {
            "canonical_authority_bytes",
            "challenge",
            "nvattest_output_path",
            "output_path",
            "support_wheel_paths",
        }
        proof = build_install_proof(
            **{
                key: value
                for key, value in kwargs.items()
                if key not in install_excluded_keys
            },
            observation=_proof_observation(
                target,
                env_root=root / "env" / target,
                candidate_dir=Path(kwargs["candidate_dir"]),
                install_paths=install_paths,
                version=str(kwargs["version"]),
            ),
            recorded_at=datetime(2026, 7, 20, 12, 30, tzinfo=UTC),
        )
        install_receipt = write_install_proof(
            output_path,
            proof,
            target=target,
            version=str(kwargs["version"]),
            source_commit=str(kwargs["source_commit"]),
            core_lock_sha256=str(kwargs["core_lock_sha256"]),
            candidate_digest=str(kwargs["candidate_digest"]),
            ledger_sha256=str(kwargs["ledger_sha256"]),
            candidate_dir=Path(kwargs["candidate_dir"]),
            ledger_payload=kwargs["ledger_payload"],
        )
        evidence_staging = output_path.parents[1]
        support_paths = tuple(
            sorted((evidence_staging / "support").iterdir(), key=lambda path: path.name)
        )
        nvattest_receipt = nvattest_proof.run_nvattest_proof(
            target=target,
            version=str(kwargs["version"]),
            source_commit=str(kwargs["source_commit"]),
            core_lock_sha256=str(kwargs["core_lock_sha256"]),
            candidate_digest=str(kwargs["candidate_digest"]),
            ledger_sha256=str(kwargs["ledger_sha256"]),
            challenge=str(kwargs["ledger_payload"]["nvattest"]["challenge"]),
            candidate_dir=Path(kwargs["candidate_dir"]),
            candidate_paths=install_paths,
            support_wheel_paths=support_paths,
            output_path=Path(kwargs["nvattest_output_path"]),
            services=_nvattest_services(
                root=root,
                target=target,
                candidate_paths=install_paths,
                support_paths=support_paths,
                canonical_authority_bytes=NVATTEST_AUTHORITY_BYTES,
                call_counts=call_counts,
            ),
            canonical_authority_bytes=NVATTEST_AUTHORITY_BYTES,
        )
        return TargetProofPaths(install=install_receipt, nvattest=nvattest_receipt)

    def cleanup(paths: Sequence[Path]) -> None:
        count("cleanup_transients")
        for path in paths:
            if path.is_dir():
                shutil.rmtree(path)
            elif path.exists() or path.is_symlink():
                path.unlink()

    def git_head(_repo: Path) -> str:
        count("git_head")
        return SOURCE_COMMIT

    def git_status(_repo: Path) -> str:
        count("git_status")
        return ""

    def git_tag_commit(_repo: Path, _version: str) -> driver.TagLookup:
        count("git_tag_commit")
        return driver.TagLookup(state="absent", commit=None, detail=None)

    def core_lock_sha256(_repo: Path) -> str:
        count("core_lock_sha256")
        return LOCK_SHA

    def prepare_policy(_repo: Path, _env: Mapping[str, str]) -> PolicyRun:
        count("prepare_policy")
        return _policy()

    def coordinator_tool_evidence() -> Mapping[str, Mapping[str, str]]:
        count("coordinator_tool_evidence")
        return {
            lane: pins.fixture_lane_tool_evidence(lane)
            for lane in ("source", "linux-x86_64-musl", "linux-aarch64-musl")
        }

    def challenge_factory() -> str:
        count("challenge_factory")
        return hashlib.sha256(str(root).encode("utf-8")).hexdigest()

    def transaction_hook(_point: str) -> None:
        count("transaction_hook")

    def reset_call_counts() -> None:
        for name in tuple(call_counts):
            call_counts[name] = 0

    service = driver.CandidateServices(
        git_head=git_head,
        git_status=git_status,
        git_tag_commit=git_tag_commit,
        core_lock_sha256=core_lock_sha256,
        clean_outputs=clean_outputs,
        build_local_dist=build_local_dist,
        prepare_policy=prepare_policy,
        coordinator_tool_evidence=coordinator_tool_evidence,
        create_source_bundle=create_source_bundle,
        build_host=build_host,
        cleanup_transients=cleanup,
        challenge_factory=challenge_factory,
        materialize_support_wheels=materialize_support_wheels,
        run_target_proofs=run_target_proofs,
        transaction_hook=transaction_hook,
    )
    object.__setattr__(service, "call_counts", call_counts)
    object.__setattr__(service, "reset_call_counts", reset_call_counts)
    return service


def recover(root: Path) -> driver.CandidateReport:
    return driver.run_recover(
        root,
        version=checker._current_version(),
        source_commit=SOURCE_COMMIT,
    )


def _tombstone_payload(version: str) -> dict[str, Any]:
    return {
        "schema_version": 1,
        "kind": checker.CORE_UNSUPPORTED_TOMBSTONE_KIND,
        "project": checker.CORE_UNSUPPORTED_TOMBSTONE_PROJECT,
        "version": version,
        "status": checker.CORE_UNSUPPORTED_TOMBSTONE_STATUS,
        "supported_platform_triples": list(
            checker.CORE_UNSUPPORTED_TOMBSTONE_SUPPORTED_TRIPLES
        ),
        "resolver_checks": dict(checker.CORE_UNSUPPORTED_TOMBSTONE_RESOLVER_CHECKS),
    }


def write_core_unsupported_tombstone_record(
    evidence_dir: Path,
    version: str,
    *,
    mutation: TombstoneMutation | None = None,
) -> Path:
    path = evidence_dir / checker.CORE_UNSUPPORTED_TOMBSTONE_RECORD
    if mutation == "malformed-json":
        path.write_text("{not-json", encoding="utf-8")
        return path
    if mutation == "non-mapping":
        path.write_text("[]", encoding="utf-8")
        return path
    payload: Mapping[str, Any] | dict[str, Any] = _tombstone_payload(version)
    if mutation == "extra-key":
        payload = {**payload, "extra": "invalid"}
    elif mutation == "missing-key":
        payload = dict(payload)
        payload.pop("status")
    elif mutation == "wrong-status":
        payload = {**payload, "status": "not-verified"}
    elif mutation == "wrong-version":
        payload = {**payload, "version": "0.0.0"}
    path.write_bytes(checker.canonical_json_bytes(payload))
    return path
